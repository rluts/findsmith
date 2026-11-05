use crate::auth::CurrentUser;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::{
    csrf,
    diff::{SideBySideLine, side_by_side_lines},
    jobs::{JobConfigV1, JobRepository, JobState, JobStatus, QueueEntry, QueueEntryStatus},
    localization,
    mediawiki::{PageLoadError, PageSnapshot, edit_permissions, load_page},
    policy,
    projects::ProjectRepository,
    rules::TransformationResult,
    skip::{PageFacts, SkipMatcher, SkipOptions, SkipReason},
    structural::apply_with_wiki,
    wikis,
};

use super::AppState;
use super::preview::{RulePreviewView, protection_text};
use super::project_form::category_prefix_for_wiki;
use super::responses::{
    csrf_error, error_page, internal_error, job_error_response, job_not_found,
    project_error_response, render,
};
use super::summary::{EDIT_SUMMARY_ATTRIBUTION_LINK, suggested_edit_summary};

#[derive(Serialize)]
struct ReviewResponse<'a> {
    project_id: &'a str,
    project_name: &'a str,
    job_id: &'a str,
    state_version: i64,
    csrf_token: &'a str,
    status: &'a JobStatus,
    position: usize,
    total: usize,
    pending: usize,
    reviewed: usize,
    saved: usize,
    skipped: usize,
    unavailable: usize,
    rule_count: usize,
    suggested_edit_summary: String,
    edit_summary_attribution_link: &'static str,
    generation_message: String,
    preparse_active: bool,
    preparse_cursor: usize,
    edit_in_flight: bool,
    edit_policy_blocked: bool,
    policy_acknowledged: bool,
    queue: Vec<QueueEntry>,
    current: Option<QueueEntry>,
    page: Option<&'a ReviewPageView>,
    page_error: Option<String>,
}

#[derive(Serialize)]
struct ReviewPageView {
    full_url: String,
    revision_id: u64,
    revision_timestamp: String,
    server_timestamp: String,
    content_model: String,
    editable: bool,
    can_edit: bool,
    can_minor: bool,
    can_bot: bool,
    edit_access_message: Option<String>,
    bot_exclusion_message: Option<String>,
    skip_reason: Option<String>,
    protection: String,
    total_replacements: usize,
    changed: bool,
    rules: Vec<RulePreviewView>,
    proposed_text: String,
    side_by_side_diff: Vec<SideBySideLine>,
}

pub(super) fn localize_queue_entry(mut entry: QueueEntry) -> QueueEntry {
    entry.reason = entry.reason.map(|reason| {
        localization::message(&reason).unwrap_or_else(|| localization::from_source(&reason))
    });
    entry
}

#[derive(Deserialize)]
pub struct RecomputeForm {
    csrf_token: String,
    proposed_text: String,
}

pub async fn job_review(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response = review_response(
        &state.database,
        &user.central_id,
        &user.username,
        &user.access_token,
        &state.user_agent,
        (&id, &job_id),
        &token,
        None,
    )
    .await;
    (jar, response)
}

pub async fn job_recompute(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<RecomputeForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    review_response(
        &state.database,
        &user.central_id,
        &user.username,
        &user.access_token,
        &state.user_agent,
        (&id, &job_id),
        &form.csrf_token,
        Some(form.proposed_text),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn review_response(
    database: &DatabaseConnection,
    owner: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
    ids: (&str, &str),
    csrf_token: &str,
    proposed_text: Option<String>,
) -> Response {
    let (project_id, job_id) = ids;
    let project = match ProjectRepository::new(database)
        .get(owner, project_id)
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => return job_not_found(project_id),
        Err(error) => return project_error_response(error, "/workspace"),
    };
    let job = match JobRepository::new(database)
        .get(owner, project_id, job_id)
        .await
    {
        Ok(Some(job)) => job,
        Ok(None) => return job_not_found(project_id),
        Err(error) => return job_error_response(error, &format!("/projects/{project_id}")),
    };
    let config: JobConfigV1 = match serde_json::from_value(job.config_json) {
        Ok(config) => config,
        Err(error) => return internal_error(error),
    };
    if config.version != crate::jobs::JOB_CONFIG_VERSION {
        return error_page(
            StatusCode::CONFLICT,
            "Unsupported review configuration",
            "This review uses a configuration version that this release cannot open.",
            &format!("/projects/{project_id}"),
        );
    }
    let state: JobState = match serde_json::from_value(job.state_json) {
        Ok(state) => state,
        Err(error) => return internal_error(error),
    };
    let counters = state.counters();
    let current = (!state.preparse_active).then(|| state.current()).flatten();
    let mut page = None;
    let mut page_error = None;
    if let Some(entry) = current.filter(|entry| {
        !state.status.is_building()
            && entry.status != QueueEntryStatus::Missing
            && entry.status != QueueEntryStatus::Invalid
    }) {
        match review_page_view(
            database,
            &job.wiki_url,
            user_agent,
            access_token,
            username,
            entry,
            &config,
            proposed_text,
        )
        .await
        {
            Ok(view) => page = Some(view),
            Err(error) => page_error = Some(error),
        }
    }
    render(ReviewResponse {
        project_id,
        project_name: &project.name,
        job_id,
        state_version: job.state_version,
        csrf_token,
        status: &state.status,
        position: state.position.saturating_add(1).min(state.queue.len()),
        total: state.queue.len(),
        pending: counters.pending,
        reviewed: counters.reviewed,
        saved: counters.saved,
        skipped: counters.skipped,
        unavailable: counters.unavailable,
        rule_count: config.rules.len(),
        suggested_edit_summary: suggested_edit_summary(&config.rules),
        edit_summary_attribution_link: EDIT_SUMMARY_ATTRIBUTION_LINK,
        generation_message: localization::from_source(&state.generation_message),
        preparse_active: state.preparse_active,
        preparse_cursor: state.preparse_cursor,
        edit_in_flight: state.edit_in_flight,
        edit_policy_blocked: state.edit_policy_blocked,
        policy_acknowledged: state.policy_acknowledged,
        queue: state
            .queue
            .iter()
            .cloned()
            .map(localize_queue_entry)
            .collect(),
        current: current.cloned().map(localize_queue_entry),
        page: page.as_ref(),
        page_error: page_error.map(|message| localization::from_source(&message)),
    })
}

/// A loaded page, the rules' output for it, and the skip rule it meets, if any.
pub(super) struct PageEvaluation {
    pub(super) snapshot: PageSnapshot,
    pub(super) result: TransformationResult,
    pub(super) skip: Option<SkipReason>,
    pub(super) bot_excluded: bool,
}

pub(super) enum EvaluationError {
    Load(PageLoadError),
    Transform(String),
}

/// Loads a page and applies the skip options in a fixed order: page facts and
/// content conditions, bot exclusions, then the rules' effect on the page.
#[allow(clippy::too_many_arguments)]
pub(super) async fn evaluate_page(
    database: &DatabaseConnection,
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    username: &str,
    config: &JobConfigV1,
    matcher: &SkipMatcher,
    title: &str,
) -> Result<PageEvaluation, EvaluationError> {
    let snapshot = load_page(wiki_url, user_agent, access_token, title)
        .await
        .map_err(EvaluationError::Load)?;
    let category_prefix = category_prefix_for_wiki(database, wiki_url).await;
    let result = apply_with_wiki(
        &config.rules,
        &snapshot.content,
        wiki_url,
        user_agent,
        &category_prefix,
    )
    .await
    .map_err(EvaluationError::Transform)?;
    let skip = matcher.before_rules(&PageFacts {
        namespace: snapshot.namespace,
        redirect: snapshot.redirect,
        editable: snapshot.editable,
        content: &snapshot.content,
    });
    // Checked even for a page skipped for another reason: saving an excluded page
    // locks the review. A failed check does not skip the page; saving checks it again.
    let bot_excluded = policy::bot_edit_allowed(wiki_url, user_agent, username, &snapshot.content)
        .await
        == Ok(false);
    let skip = skip
        .or(bot_excluded.then_some(SkipReason::BotExclusion))
        .or_else(|| {
            matcher.after_rules(&snapshot.content, &result.text, result.total_replacements)
        });
    Ok(PageEvaluation {
        snapshot,
        result,
        skip,
        bot_excluded,
    })
}

pub(super) async fn skip_matcher(
    database: &DatabaseConnection,
    wiki_url: &str,
    config: &JobConfigV1,
    options: &SkipOptions,
) -> Result<SkipMatcher, String> {
    let in_use_templates = if options.in_use_tpl {
        wikis::in_use_templates(database, wiki_url).await
    } else {
        Vec::new()
    };
    SkipMatcher::compile(options, &config.namespaces, &in_use_templates)
}

#[allow(clippy::too_many_arguments)]
async fn review_page_view(
    database: &DatabaseConnection,
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    username: &str,
    entry: &QueueEntry,
    config: &JobConfigV1,
    proposed_text: Option<String>,
) -> Result<ReviewPageView, String> {
    let matcher = skip_matcher(database, wiki_url, config, &config.skip).await?;
    let PageEvaluation {
        snapshot,
        result,
        skip,
        bot_excluded,
    } = evaluate_page(
        database,
        wiki_url,
        user_agent,
        access_token,
        username,
        config,
        &matcher,
        &entry.title,
    )
    .await
    .map_err(|error| match error {
        EvaluationError::Load(error) => error.to_string(),
        EvaluationError::Transform(error) => error,
    })?;
    let proposed_text = proposed_text.unwrap_or_else(|| result.text.clone());
    if proposed_text.len() > crate::rules::MAX_OUTPUT_BYTES {
        return Err("The proposed wikitext is too large.".to_owned());
    }
    let protection = protection_text(&snapshot);
    let permissions = edit_permissions(wiki_url, user_agent, access_token).await;
    let edit_access_message = match &permissions {
        Err(_) => Some("findsmith-api-access-rights-unverified"),
        Ok(permissions) => policy::account_blocker(permissions).map(|blocker| match blocker {
            policy::AccountBlocker::Blocked => "findsmith-api-account-blocked",
            policy::AccountBlocker::NewMessages => "findsmith-api-new-messages",
            policy::AccountBlocker::MissingRights => "findsmith-api-edit-rights-missing",
        }),
    };
    let permissions = permissions.ok();
    let side_by_side_diff = side_by_side_lines(&snapshot.content, &proposed_text);
    Ok(ReviewPageView {
        full_url: snapshot.full_url,
        revision_id: snapshot.revision_id,
        revision_timestamp: snapshot.revision_timestamp,
        server_timestamp: snapshot.server_timestamp,
        content_model: snapshot.content_model,
        editable: snapshot.editable,
        can_edit: permissions
            .as_ref()
            .is_some_and(|permissions| policy::account_blocker(permissions).is_none()),
        can_minor: permissions
            .as_ref()
            .is_some_and(|permissions| permissions.can_minor),
        can_bot: permissions
            .as_ref()
            .is_some_and(|permissions| permissions.can_bot),
        edit_access_message: edit_access_message.map(localization::from_source),
        bot_exclusion_message: bot_excluded
            .then(|| localization::required("findsmith-api-save-bot-excluded")),
        skip_reason: skip.map(|reason| localization::required(reason.message_key())),
        protection,
        total_replacements: result.total_replacements,
        changed: snapshot.content != proposed_text,
        rules: result
            .counts
            .into_iter()
            .zip(result.changed)
            .map(|(count, changed)| RulePreviewView { count, changed })
            .collect(),
        proposed_text,
        side_by_side_diff,
    })
}
