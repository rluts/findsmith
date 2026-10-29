use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use parsoid::prelude::*;

use crate::{error::ApiFailureKind, mediawiki::EditPermissions};

/// Minimum time between two saves by the same account. Every edit is reviewed by
/// a person, but this keeps bursts of quick saves at a conservative pace.
pub const MIN_EDIT_INTERVAL: Duration = Duration::from_secs(5);
/// Consecutive failed saves after which a job stops until a new review starts.
pub const MAX_CONSECUTIVE_EDIT_FAILURES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountBlocker {
    Blocked,
    NewMessages,
    MissingRights,
}

impl AccountBlocker {
    pub fn kind(self) -> ApiFailureKind {
        match self {
            Self::Blocked => ApiFailureKind::Block,
            Self::NewMessages | Self::MissingRights => ApiFailureKind::Permission,
        }
    }
}

/// Why the account cannot edit right now. OAuth 2 limits `meta=userinfo` rights to
/// the granted ones, so a missing `read` or `edit` right also covers missing grants.
pub fn account_blocker(permissions: &EditPermissions) -> Option<AccountBlocker> {
    if permissions.blocked {
        Some(AccountBlocker::Blocked)
    } else if !permissions.can_read || !permissions.can_edit {
        Some(AccountBlocker::MissingRights)
    } else if permissions.new_messages {
        Some(AccountBlocker::NewMessages)
    } else {
        None
    }
}

/// Message explaining why a new review cannot start. New talk-page messages do not
/// stop a review, which never edits; saving checks them again.
pub fn job_start_blocker(permissions: &EditPermissions) -> Option<&'static str> {
    match account_blocker(permissions)? {
        AccountBlocker::Blocked => Some("findsmith-api-start-account-blocked"),
        AccountBlocker::MissingRights => Some("findsmith-api-start-rights-missing"),
        AccountBlocker::NewMessages => None,
    }
}

/// Whether a failed save should lock the job instead of letting the editor retry.
pub fn halts_editing(kind: ApiFailureKind, consecutive_failures: u32) -> bool {
    kind.is_policy_sensitive() || consecutive_failures >= MAX_CONSECUTIVE_EDIT_FAILURES
}

fn last_edits() -> &'static Mutex<HashMap<String, Instant>> {
    static LAST_EDITS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LAST_EDITS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Reserves the next edit slot for an account, or returns the whole seconds to wait.
pub fn reserve_edit_slot(owner: &str) -> Result<(), u64> {
    reserve_edit_slot_at(
        &mut last_edits()
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        owner,
        Instant::now(),
    )
}

fn reserve_edit_slot_at(
    last_edits: &mut HashMap<String, Instant>,
    owner: &str,
    now: Instant,
) -> Result<(), u64> {
    if let Some(previous) = last_edits.get(owner) {
        let elapsed = now.saturating_duration_since(*previous);
        if elapsed < MIN_EDIT_INTERVAL {
            let remaining = MIN_EDIT_INTERVAL - elapsed;
            return Err(remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0));
        }
    }
    if last_edits.len() >= 10_000 {
        last_edits
            .retain(|_, previous| now.saturating_duration_since(*previous) < MIN_EDIT_INTERVAL);
    }
    last_edits.insert(owner.to_owned(), now);
    Ok(())
}

/// Cheap pre-check before asking Parsoid: `{{bots}}` and `{{nobots}}` cannot
/// appear in text that never mentions "bots".
pub fn may_exclude_bots(text: &str) -> bool {
    text.to_lowercase().contains("bots")
}

/// Applies `{{nobots}}` and `{{bots|allow=…|deny=…}}` to the signed-in account.
pub async fn bot_edit_allowed(
    wiki_url: &str,
    user_agent: &str,
    username: &str,
    text: &str,
) -> Result<bool, String> {
    if !may_exclude_bots(text) {
        return Ok(true);
    }
    let rest_url = wiki_url
        .strip_suffix("api.php")
        .ok_or("The wiki has no supported Parsoid endpoint.")?;
    let client = ParsoidClient::new(&format!("{rest_url}rest.php"), user_agent)
        .map_err(|error| format!("Could not connect to Parsoid: {error}"))?;
    let code = client
        .transform_to_html(text)
        .await
        .map_err(|error| format!("Could not inspect bot exclusions: {error}"))?
        .into_mutable();
    let templates = code
        .filter_templates()
        .map_err(|error| format!("Could not inspect bot exclusions: {error}"))?;
    Ok(templates_allow_user(
        templates.iter().map(|template| {
            (
                template.name_in_wikitext(),
                template.param("allow"),
                template.param("deny"),
            )
        }),
        username,
    ))
}

fn templates_allow_user(
    templates: impl IntoIterator<Item = (String, Option<String>, Option<String>)>,
    username: &str,
) -> bool {
    let normalize = |name: &str| name.trim().replace('_', " ").to_lowercase();
    let user = normalize(username);
    let lists = |names: &str| {
        names
            .split(',')
            .map(normalize)
            .any(|name| name == "all" || name == user)
    };
    for (name, allow, deny) in templates {
        let name = normalize(&name);
        let name = name.strip_prefix("template:").unwrap_or(&name).trim();
        if name == "nobots" {
            return false;
        }
        if name != "bots" {
            continue;
        }
        if allow.is_some_and(|allow| !lists(&allow)) || deny.is_some_and(|deny| lists(&deny)) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissions() -> EditPermissions {
        EditPermissions {
            can_read: true,
            can_edit: true,
            can_minor: true,
            can_bot: false,
            blocked: false,
            new_messages: false,
        }
    }

    #[test]
    fn account_checks_report_the_most_serious_problem() {
        assert_eq!(account_blocker(&permissions()), None);
        let blocked = EditPermissions {
            blocked: true,
            can_edit: false,
            ..permissions()
        };
        assert_eq!(account_blocker(&blocked), Some(AccountBlocker::Blocked));
        let no_grant = EditPermissions {
            can_edit: false,
            ..permissions()
        };
        assert_eq!(
            job_start_blocker(&no_grant),
            Some("findsmith-api-start-rights-missing")
        );
        let messages = EditPermissions {
            new_messages: true,
            ..permissions()
        };
        assert_eq!(
            account_blocker(&messages),
            Some(AccountBlocker::NewMessages)
        );
        assert_eq!(job_start_blocker(&messages), None);
    }

    #[test]
    fn saves_are_paced_per_account() {
        let mut edits = HashMap::new();
        let start = Instant::now();
        assert_eq!(reserve_edit_slot_at(&mut edits, "1", start), Ok(()));
        assert_eq!(
            reserve_edit_slot_at(&mut edits, "1", start + Duration::from_millis(1500)),
            Err(4)
        );
        assert_eq!(reserve_edit_slot_at(&mut edits, "2", start), Ok(()));
        assert_eq!(
            reserve_edit_slot_at(&mut edits, "1", start + MIN_EDIT_INTERVAL),
            Ok(())
        );
    }

    #[test]
    fn repeated_failures_halt_editing() {
        assert!(!halts_editing(ApiFailureKind::Maxlag, 1));
        assert!(halts_editing(
            ApiFailureKind::Maxlag,
            MAX_CONSECUTIVE_EDIT_FAILURES
        ));
        assert!(halts_editing(ApiFailureKind::AbuseFilter, 1));
    }

    #[test]
    fn bot_templates_follow_allow_and_deny_lists() {
        let template = |name: &str, allow: Option<&str>, deny: Option<&str>| {
            (
                name.to_owned(),
                allow.map(str::to_owned),
                deny.map(str::to_owned),
            )
        };
        assert!(templates_allow_user(
            [template("Infobox", None, None)],
            "Some_User"
        ));
        assert!(!templates_allow_user(
            [template("Nobots", None, None)],
            "Some User"
        ));
        assert!(!templates_allow_user(
            [template("Template:Bots", None, Some("Other, Some_User"))],
            "Some User"
        ));
        assert!(templates_allow_user(
            [template("bots", Some("some user"), None)],
            "Some_User"
        ));
        assert!(!templates_allow_user(
            [template("bots", Some("Other"), None)],
            "Some User"
        ));
        assert!(!may_exclude_bots("Plain article text."));
        assert!(may_exclude_bots("{{NoBots}}"));
    }
}
