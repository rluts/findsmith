use axum::{http::StatusCode, response::Response};

use crate::{
    error::{self, ApiFailureKind},
    jobs::{JobConfigV1, JobRepository, JobState, JobStatus, QueueEntryStatus},
    localization,
    mediawiki::{
        RecentRevision, edit_existing_page, edit_permissions, load_page, recent_revisions,
    },
    policy,
    wikis::{self},
};

use super::job_actions::JobActionForm;
use super::responses::{action_response, error_page, job_error_response};
use super::summary::{
    EDIT_SUMMARY_ATTRIBUTION_LINK, complete_edit_summary, suggested_edit_summary,
};

#[allow(clippy::too_many_arguments)]
pub(super) async fn save_current_page(
    repository: &JobRepository<'_>,
    owner: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
    project_id: &str,
    job_id: &str,
    wiki_url: &str,
    state_version: i64,
    state: &mut JobState,
    config: &JobConfigV1,
    form: &JobActionForm,
    back_href: &str,
) -> Response {
    if !state.can_save_current() {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-cannot-save",
            "findsmith-api-select-a-pending-or-reviewed-page-in-an-active-or-completed-review",
            back_href,
        );
    }
    if state.edit_policy_blocked {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-edit-blocked",
            "findsmith-api-this-job-encountered-a-wiki-policy-warning-start-a-new-review-after-reso",
            back_href,
        );
    }
    let (Some(text), Some(base_revid), Some(start_timestamp)) =
        (&form.proposed_text, form.base_revid, &form.start_timestamp)
    else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-cannot-save",
            "findsmith-api-the-reviewed-text-base-revision-and-start-timestamp-are-required",
            back_href,
        );
    };
    if !form.policy_acknowledged && !state.policy_acknowledged {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-policy-acknowledgement-required",
            "findsmith-api-acknowledge-the-selected-wiki-s-editing-policies-before-saving",
            back_href,
        );
    }
    if text.len() > crate::rules::MAX_OUTPUT_BYTES {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-cannot-save",
            "findsmith-api-the-proposed-wikitext-is-too-large",
            back_href,
        );
    }
    let summary_prefix = form
        .edit_summary
        .as_deref()
        .map(str::to_owned)
        .unwrap_or_else(|| suggested_edit_summary(&config.rules));
    let summary = match complete_edit_summary(&summary_prefix) {
        Ok(summary) => summary,
        Err(message) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-cannot-save",
                message,
                back_href,
            );
        }
    };
    let title = state
        .current()
        .map(|entry| entry.title.clone())
        .unwrap_or_default();
    let access = wikis::access_unregistered(wiki_url, user_agent, username).await;
    if !access.is_some_and(|access| access.allowed == Some(true)) {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "permission",
            "findsmith-api-save-awb-access",
            back_href,
        )
        .await;
    }
    let permissions = match edit_permissions(wiki_url, user_agent, access_token).await {
        Ok(permissions) => permissions,
        Err(error) => {
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                error::classify(&error).label(),
                "findsmith-api-save-rights-unverified",
                back_href,
            )
            .await;
        }
    };
    if let Some(blocker) = policy::account_blocker(&permissions) {
        let kind = blocker.kind().label();
        let message = match blocker {
            policy::AccountBlocker::Blocked => "findsmith-api-save-account-blocked",
            policy::AccountBlocker::NewMessages => "findsmith-api-save-new-messages",
            policy::AccountBlocker::MissingRights => "findsmith-api-save-rights-missing",
        };
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            kind,
            message,
            back_href,
        )
        .await;
    }
    if (form.minor && !permissions.can_minor) || (form.bot && !permissions.can_bot) {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "permission",
            "findsmith-api-save-flags-denied",
            back_href,
        )
        .await;
    }
    let snapshot = match load_page(wiki_url, user_agent, access_token, &title).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                "conflict",
                &localization::required_with_args(
                    "findsmith-api-save-reload-page",
                    &[&error.to_string()],
                ),
                back_href,
            )
            .await;
        }
    };
    if snapshot.revision_id != base_revid || snapshot.title != title {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "conflict",
            "findsmith-api-save-revision-changed",
            back_href,
        )
        .await;
    }
    if !snapshot.editable {
        return pause_edit(
            repository,
            owner,
            project_id,
            job_id,
            state_version,
            state,
            "protection",
            "findsmith-api-save-page-protected",
            back_href,
        )
        .await;
    }
    match policy::bot_edit_allowed(wiki_url, user_agent, username, &snapshot.content).await {
        Ok(true) => {}
        Ok(false) => {
            state.edit_policy_blocked = true;
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                "bot exclusion",
                "findsmith-api-save-bot-excluded",
                back_href,
            )
            .await;
        }
        Err(_) => {
            return pause_edit(
                repository,
                owner,
                project_id,
                job_id,
                state_version,
                state,
                "bot exclusion",
                "findsmith-api-save-bot-check-failed",
                back_href,
            )
            .await;
        }
    }
    if snapshot.content == *text {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-no-changes",
            "findsmith-api-the-proposed-wikitext-matches-the-current-revision",
            back_href,
        );
    }
    if let Err(seconds) = policy::reserve_edit_slot(owner) {
        return error_page(
            StatusCode::TOO_MANY_REQUESTS,
            "findsmith-api-cannot-save",
            &localization::required_with_args(
                "findsmith-api-save-too-fast",
                &[&seconds.to_string()],
            ),
            back_href,
        );
    }
    state.policy_acknowledged = true;
    state.edit_in_flight = true;
    state.status = JobStatus::Stopped;
    if let Some(entry) = state.queue.get_mut(state.position) {
        // The base revision lets an interrupted request be checked later.
        entry.revision_id = Some(snapshot.revision_id);
        entry.revision_timestamp = Some(snapshot.revision_timestamp.clone());
        entry.edit_result = Some("in_flight".to_owned());
        entry.reason = Some("findsmith-api-edit-outcome-unknown-reason".to_owned());
    }
    if let Err(error) = repository
        .update_state(owner, project_id, job_id, state, state_version)
        .await
    {
        return job_error_response(error, back_href);
    }
    let outcome = edit_existing_page(
        wiki_url,
        user_agent,
        access_token,
        username,
        &snapshot.title,
        text,
        snapshot.revision_id,
        &snapshot.revision_timestamp,
        start_timestamp,
        &summary,
        form.minor,
        form.bot,
    )
    .await;
    match outcome {
        Ok(new_revid) => {
            state.edit_in_flight = false;
            state.consecutive_edit_failures = 0;
            state.status = JobStatus::Active;
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_revision_id = Some(new_revid);
                entry.edit_result = Some("saved".to_owned());
            }
            state.advance(QueueEntryStatus::Saved, None);
        }
        Err(error) => {
            let kind = error::classify(&error);
            // Transport failures and unrecognized responses can happen after MediaWiki committed the edit.
            let ambiguous = ApiFailureKind::is_ambiguous(&error);
            state.edit_in_flight = ambiguous;
            state.consecutive_edit_failures += 1;
            // An ambiguous outcome stays locked as in flight until it is checked instead.
            state.edit_policy_blocked =
                !ambiguous && policy::halts_editing(kind, state.consecutive_edit_failures);
            let repeated = state.edit_policy_blocked && !kind.is_policy_sensitive();
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_result = Some(kind.label().to_owned());
                entry.reason = Some(if ambiguous {
                    "findsmith-api-save-outcome-ambiguous".to_owned()
                } else if repeated {
                    "findsmith-api-save-repeated-failures".to_owned()
                } else if kind == ApiFailureKind::Conflict {
                    "findsmith-api-save-conflict".to_owned()
                } else if kind == ApiFailureKind::AbuseFilter {
                    "findsmith-api-save-abuse-filter".to_owned()
                } else {
                    localization::required_with_args("findsmith-api-save-rejected", &[kind.label()])
                });
            }
        }
    }
    match repository
        .update_state(owner, project_id, job_id, state, state_version + 1)
        .await
    {
        Ok(()) => action_response(back_href.to_owned(), Some(job_id.to_owned())),
        Err(error) => job_error_response(error, back_href),
    }
}

#[allow(clippy::too_many_arguments)]
async fn pause_edit(
    repository: &JobRepository<'_>,
    owner: &str,
    project_id: &str,
    job_id: &str,
    state_version: i64,
    state: &mut JobState,
    kind: &str,
    message: &str,
    back_href: &str,
) -> Response {
    state.status = JobStatus::Stopped;
    if let Some(entry) = state.queue.get_mut(state.position) {
        entry.edit_result = Some(kind.to_owned());
        entry.reason = Some(message.to_owned());
    }
    match repository
        .update_state(owner, project_id, job_id, state, state_version)
        .await
    {
        Ok(()) => action_response(back_href.to_owned(), Some(job_id.to_owned())),
        Err(error) => job_error_response(error, back_href),
    }
}

/// Resolves an edit whose outcome is unknown, for example after a timeout or a
/// server restart, by looking for the FindSmith revision in the page history.
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_edit_response(
    repository: &JobRepository<'_>,
    owner: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
    ids: (&str, &str),
    wiki_url: &str,
    state_version: i64,
    mut state: JobState,
    back_href: &str,
) -> Response {
    let (project_id, job_id) = ids;
    let Some(entry) = state.current().filter(|_| state.edit_in_flight) else {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-no-edit-to-verify",
            "findsmith-api-no-edit-to-verify-details",
            back_href,
        );
    };
    let base_revision = entry.revision_id;
    let revisions = match recent_revisions(wiki_url, user_agent, access_token, &entry.title).await {
        Ok(revisions) => revisions,
        Err(error) => {
            return error_page(
                StatusCode::BAD_GATEWAY,
                "findsmith-api-could-not-verify-edit",
                &localization::required_with_args(
                    "findsmith-api-mediawiki-request-error",
                    &[error::classify(&error).label()],
                ),
                back_href,
            );
        }
    };
    state.edit_in_flight = false;
    match find_own_revision(&revisions, base_revision, username) {
        Some(revision_id) => {
            state.consecutive_edit_failures = 0;
            state.status = JobStatus::Active;
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_revision_id = Some(revision_id);
                entry.edit_result = Some("saved".to_owned());
                entry.reason = None;
            }
            state.advance(QueueEntryStatus::Saved, None);
        }
        None => {
            state.status = JobStatus::Stopped;
            if let Some(entry) = state.queue.get_mut(state.position) {
                entry.edit_result = Some("not_saved".to_owned());
                entry.reason = Some("findsmith-api-edit-verified-not-saved".to_owned());
            }
        }
    }
    match repository
        .update_state(owner, project_id, job_id, &state, state_version)
        .await
    {
        Ok(()) => action_response(back_href.to_owned(), Some(job_id.to_owned())),
        Err(error) => job_error_response(error, back_href),
    }
}

/// Finds a revision by this account with the FindSmith attribution, newer than
/// the base revision. Without a recorded base only the latest revision counts.
fn find_own_revision(
    revisions: &[RecentRevision],
    base_revision: Option<u64>,
    username: &str,
) -> Option<u64> {
    let normalize = |name: &str| name.replace('_', " ");
    revisions
        .iter()
        .take(if base_revision.is_some() {
            revisions.len()
        } else {
            1
        })
        .take_while(|revision| base_revision.is_none_or(|base| revision.revision_id > base))
        .find(|revision| {
            normalize(&revision.user) == normalize(username)
                && revision.comment.contains(EDIT_SUMMARY_ATTRIBUTION_LINK)
        })
        .map(|revision| revision.revision_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_edit_is_found_only_after_the_base_revision() {
        let revision = |revision_id: u64, user: &str, comment: &str| RecentRevision {
            revision_id,
            user: user.to_owned(),
            comment: comment.to_owned(),
        };
        let attributed = format!("Fix typo ({EDIT_SUMMARY_ATTRIBUTION_LINK})");
        let history = [
            revision(12, "Other", "unrelated"),
            revision(11, "Some_User", &attributed),
            revision(10, "Some User", &attributed),
        ];
        assert_eq!(find_own_revision(&history, Some(10), "Some User"), Some(11));
        assert_eq!(find_own_revision(&history, Some(11), "Some User"), None);
        assert_eq!(
            find_own_revision(&history[1..], None, "Some User"),
            Some(11)
        );
        assert_eq!(find_own_revision(&history, None, "Some User"), None);
        let manual = [revision(11, "Some User", "manual edit")];
        assert_eq!(find_own_revision(&manual, Some(10), "Some User"), None);
    }
}
