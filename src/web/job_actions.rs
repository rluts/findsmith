use crate::auth::CurrentUser;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use crate::{
    csrf,
    jobs::{JobConfigV1, JobError, JobRepository, JobState, JobStatus, QueueEntryStatus},
    lists::{queue_from_normalized, source_values},
    localization,
    mediawiki::{PageLoadError, normalize_titles},
    skip::{SkipOptions, SkipReason},
    wikis::{self},
};

use super::AppState;
use super::edit::{save_current_page, verify_edit_response};
use super::project_form::split_lines;
use super::responses::{
    action_response, csrf_error, error_page, internal_error, job_error_response, job_not_found,
};
use super::review::{EvaluationError, evaluate_page, skip_matcher};

#[derive(Deserialize)]
pub struct JobActionForm {
    pub(super) csrf_token: String,
    pub(super) state_version: i64,
    pub(super) action: String,
    #[serde(default)]
    pub(super) proposed_text: Option<String>,
    #[serde(default)]
    pub(super) base_revid: Option<u64>,
    #[serde(default)]
    pub(super) start_timestamp: Option<String>,
    #[serde(default)]
    pub(super) policy_acknowledged: bool,
    #[serde(default)]
    pub(super) minor: bool,
    #[serde(default)]
    pub(super) bot: bool,
    #[serde(default)]
    pub(super) edit_summary: Option<String>,
}

#[derive(Deserialize)]
pub struct QueueActionForm {
    csrf_token: String,
    state_version: i64,
    action: String,
    #[serde(default)]
    index: usize,
    #[serde(default)]
    titles: String,
}

pub async fn job_action(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<JobActionForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    job_action_response(
        &state.database,
        &user.central_id,
        &user.username,
        &user.access_token,
        &state.user_agent,
        &id,
        &job_id,
        form,
    )
    .await
}

pub async fn job_queue_action(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<QueueActionForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    queue_action_response(
        &state.database,
        &user.central_id,
        &user.access_token,
        &state.user_agent,
        &id,
        &job_id,
        form,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn job_action_response(
    database: &DatabaseConnection,
    owner: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
    project_id: &str,
    job_id: &str,
    form: JobActionForm,
) -> Response {
    let back_href = format!("/projects/{project_id}/jobs/{job_id}");
    let repository = JobRepository::new(database);
    let job = match repository.get(owner, project_id, job_id).await {
        Ok(Some(job)) => job,
        Ok(None) => return job_not_found(project_id),
        Err(error) => return job_error_response(error, &format!("/projects/{project_id}")),
    };
    if job.state_version != form.state_version {
        return job_error_response(JobError::Conflict, &back_href);
    }
    // Refresh AWB eligibility for review actions even when the client skips
    // the project-page status request. Save checks eligibility again and enforces it.
    let _access = wikis::access_unregistered(&job.wiki_url, user_agent, username).await;
    let config: JobConfigV1 = match serde_json::from_value(job.config_json) {
        Ok(config) => config,
        Err(error) => return internal_error(error),
    };
    let mut state: JobState = match serde_json::from_value(job.state_json) {
        Ok(state) => state,
        Err(error) => return internal_error(error),
    };
    if form.action == "verify_edit" {
        return verify_edit_response(
            &repository,
            owner,
            username,
            access_token,
            user_agent,
            (project_id, job_id),
            &job.wiki_url,
            form.state_version,
            state,
            &back_href,
        )
        .await;
    }
    if state.edit_in_flight {
        return error_page(
            StatusCode::CONFLICT,
            "Edit outcome unknown",
            "An edit request may have reached Wikimedia. Start a new review after checking the page; this job cannot retry it.",
            &back_href,
        );
    }
    if state.edit_policy_blocked && form.action != "stop" {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-edit-blocked",
            "findsmith-api-this-job-encountered-a-wiki-policy-warning-start-a-new-review-after-reso",
            &back_href,
        );
    }
    if form.action == "save" {
        return save_current_page(
            &repository,
            owner,
            username,
            access_token,
            user_agent,
            project_id,
            job_id,
            &job.wiki_url,
            job.state_version,
            &mut state,
            &config,
            &form,
            &back_href,
        )
        .await;
    }
    if state.preparse_active
        && !matches!(
            form.action.as_str(),
            "preparse_step" | "preparse_stop" | "stop"
        )
    {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-pre-parse-is-running",
            "Stop pre-parse before reviewing pages.",
            &back_href,
        );
    }
    if matches!(form.action.as_str(), "next" | "skip" | "previous")
        && state.status != JobStatus::Active
    {
        return error_page(
            StatusCode::CONFLICT,
            "Review stopped",
            "Resume the review before navigating its pages.",
            &back_href,
        );
    }
    match form.action.as_str() {
        "preparse_start" => {
            if state.status != JobStatus::Active {
                return error_page(
                    StatusCode::CONFLICT,
                    "Cannot pre-parse",
                    "Start or resume a review first.",
                    &back_href,
                );
            }
            state.preparse_cursor = 0;
            state.preparse_active = true;
        }
        "preparse_stop" => {
            state.preparse_active = false;
            if !state
                .current()
                .is_some_and(|entry| entry.status.is_pending())
            {
                if let Some(index) = state
                    .queue
                    .iter()
                    .position(|entry| entry.status.is_pending())
                {
                    state.position = index;
                } else {
                    state.status = JobStatus::Complete;
                }
            }
        }
        "preparse_step" => {
            if !state.preparse_active {
                return error_page(
                    StatusCode::CONFLICT,
                    "Pre-parse stopped",
                    "Start pre-parse before scanning pages.",
                    &back_href,
                );
            }
            if let Some(entry) = state
                .queue
                .get(state.preparse_cursor)
                .filter(|entry| entry.status.is_pending())
            {
                let title = entry.title.clone();
                // Pre-parsing always removes unchanged pages; that is its purpose.
                let options = SkipOptions {
                    no_change: true,
                    ..config.skip.clone()
                };
                let evaluation = match skip_matcher(&config, &options) {
                    Ok(matcher) => {
                        evaluate_page(
                            database,
                            &job.wiki_url,
                            user_agent,
                            access_token,
                            username,
                            &config,
                            &matcher,
                            &title,
                        )
                        .await
                    }
                    Err(message) => Err(EvaluationError::Transform(message)),
                };
                let cursor = state.preparse_cursor;
                match evaluation {
                    Ok(evaluation) => {
                        let skip = evaluation.skip.map(|reason| {
                            match reason {
                                SkipReason::NoChange => "findsmith-api-preparse-no-changes",
                                reason => reason.message_key(),
                            }
                            .to_owned()
                        });
                        if let Some(entry) = state.queue.get_mut(cursor) {
                            entry.revision_id = Some(evaluation.snapshot.revision_id);
                            entry.revision_timestamp = Some(evaluation.snapshot.revision_timestamp);
                            entry.replacement_count = Some(evaluation.result.total_replacements);
                            if let Some(reason) = skip {
                                entry.status = QueueEntryStatus::Skipped;
                                entry.reason = Some(reason);
                            }
                        }
                    }
                    Err(EvaluationError::Load(PageLoadError::Missing)) => {
                        if let Some(entry) = state.queue.get_mut(cursor) {
                            entry.status = QueueEntryStatus::Missing;
                            entry.reason = Some(SkipReason::Missing.message_key().to_owned());
                        }
                    }
                    Err(EvaluationError::Load(error)) => {
                        if let Some(entry) = state.queue.get_mut(cursor) {
                            entry.reason = Some(localization::required_with_args(
                                "findsmith-api-preparse-page-load-failed",
                                &[&error.to_string()],
                            ));
                        }
                    }
                    Err(EvaluationError::Transform(error)) => {
                        if let Some(entry) = state.queue.get_mut(cursor) {
                            entry.reason = Some(
                                localization::message_with_args(
                                    "findsmith-api-preparse-transformation-failed",
                                    &[&localization::from_source(&error)],
                                )
                                .expect("English pre-parse error message exists"),
                            );
                        }
                    }
                }
            }
            state.preparse_cursor += 1;
            state.finish_preparse_if_done();
        }
        "previous" => state.previous(),
        "resume" => {
            if !state.resume() {
                return error_page(
                    StatusCode::CONFLICT,
                    "Cannot resume review",
                    "Only a stopped review with a saved queue can be resumed.",
                    &back_href,
                );
            }
        }
        "stop" => {
            state.preparse_active = false;
            state.status = if state.status == JobStatus::Building {
                JobStatus::Cancelled
            } else {
                JobStatus::Stopped
            };
            if state.status == JobStatus::Cancelled {
                state.generation_message = "findsmith-api-queue-generation-cancelled".to_owned();
            }
        }
        "skip" => state.advance(
            QueueEntryStatus::Skipped,
            Some("findsmith-api-manual-skip".to_owned()),
        ),
        "next" => {
            if let Some(entry) = state.current().filter(|entry| entry.status.is_pending()) {
                let title = entry.title.clone();
                let evaluation = match skip_matcher(&config, &config.skip) {
                    Ok(matcher) => {
                        evaluate_page(
                            database,
                            &job.wiki_url,
                            user_agent,
                            access_token,
                            username,
                            &config,
                            &matcher,
                            &title,
                        )
                        .await
                    }
                    Err(message) => Err(EvaluationError::Transform(message)),
                };
                match evaluation {
                    Ok(evaluation) => {
                        if let Some(entry) = state.queue.get_mut(state.position) {
                            entry.revision_id = Some(evaluation.snapshot.revision_id);
                            entry.revision_timestamp = Some(evaluation.snapshot.revision_timestamp);
                            entry.replacement_count = Some(evaluation.result.total_replacements);
                        }
                        match evaluation.skip {
                            Some(reason) => state.advance(
                                QueueEntryStatus::Skipped,
                                Some(reason.message_key().to_owned()),
                            ),
                            None => state.advance(QueueEntryStatus::Reviewed, None),
                        }
                    }
                    Err(EvaluationError::Load(PageLoadError::Missing)) => state.advance(
                        QueueEntryStatus::Missing,
                        Some(SkipReason::Missing.message_key().to_owned()),
                    ),
                    Err(EvaluationError::Load(error)) => state.advance(
                        QueueEntryStatus::Skipped,
                        Some(
                            localization::message_with_args(
                                "findsmith-api-page-load-failed",
                                &[&error.to_string()],
                            )
                            .expect("English page load error message exists"),
                        ),
                    ),
                    Err(EvaluationError::Transform(error)) => state.advance(
                        QueueEntryStatus::Skipped,
                        Some(
                            localization::message_with_args(
                                "findsmith-api-transformation-failed",
                                &[&localization::from_source(&error)],
                            )
                            .expect("English transformation error message exists"),
                        ),
                    ),
                }
            } else {
                state.advance(QueueEntryStatus::Reviewed, None);
            }
        }
        _ => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Invalid review action",
                "Choose Previous, Skip, Stop, Resume, Next, or Pre-parse.",
                &back_href,
            );
        }
    }
    match repository
        .update_state(owner, project_id, job_id, &state, form.state_version)
        .await
    {
        Ok(()) => action_response(back_href, Some(job_id.to_owned())),
        Err(error) => job_error_response(error, &back_href),
    }
}

#[allow(clippy::too_many_arguments)]
async fn queue_action_response(
    database: &DatabaseConnection,
    owner: &str,
    access_token: &str,
    user_agent: &str,
    project_id: &str,
    job_id: &str,
    form: QueueActionForm,
) -> Response {
    let back_href = format!("/projects/{project_id}/jobs/{job_id}");
    let repository = JobRepository::new(database);
    let job = match repository.get(owner, project_id, job_id).await {
        Ok(Some(job)) => job,
        Ok(None) => return job_not_found(project_id),
        Err(error) => return job_error_response(error, &back_href),
    };
    if job.state_version != form.state_version {
        return job_error_response(JobError::Conflict, &back_href);
    }
    let mut state: JobState = match serde_json::from_value(job.state_json) {
        Ok(state) => state,
        Err(error) => return internal_error(error),
    };
    if state.edit_in_flight || state.edit_policy_blocked {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-queue-locked",
            "findsmith-api-this-job-is-paused-after-an-edit-attempt-start-a-new-review-after-checki",
            &back_href,
        );
    }
    if state.status == JobStatus::Building {
        return error_page(
            StatusCode::CONFLICT,
            "Queue is still building",
            "Cancel generation or wait for it to finish before changing the queue.",
            &back_href,
        );
    }
    if state.preparse_active {
        return error_page(
            StatusCode::CONFLICT,
            "findsmith-api-pre-parse-is-running",
            "findsmith-api-stop-pre-parse-before-changing-the-queue",
            &back_href,
        );
    }
    match form.action.as_str() {
        "select" => {
            if !state.select(form.index) {
                return error_page(
                    StatusCode::BAD_REQUEST,
                    "Cannot select page",
                    "Choose a page in the current list after generation finishes.",
                    &back_href,
                );
            }
        }
        "move_up" => state.move_entry(form.index, -1),
        "move_down" => state.move_entry(form.index, 1),
        "remove" => state.remove_entry(form.index),
        "clear" => state.clear(),
        "sort_by_title" | "sort_remaining" => state.sort_by_title(),
        "append" => {
            let titles = match source_values(
                &split_lines(&form.titles),
                &localization::required("findsmith-api-source-appended-pages"),
            ) {
                Ok(titles) if !titles.is_empty() => titles,
                Ok(_) => {
                    return error_page(
                        StatusCode::BAD_REQUEST,
                        "No pages to append",
                        "Enter at least one page title.",
                        &back_href,
                    );
                }
                Err(message) => {
                    return error_page(
                        StatusCode::BAD_REQUEST,
                        "findsmith-api-cannot-append-pages",
                        &message,
                        &back_href,
                    );
                }
            };
            let pages =
                match normalize_titles(&job.wiki_url, user_agent, access_token, &titles).await {
                    Ok(pages) => pages,
                    Err(error) => {
                        return error_page(
                            StatusCode::BAD_GATEWAY,
                            "findsmith-api-could-not-append-pages",
                            &localization::required_with_args(
                                "findsmith-api-mediawiki-request-error",
                                &[&error.to_string()],
                            ),
                            &back_href,
                        );
                    }
                };
            state.append_unique(queue_from_normalized(pages));
            if !state.queue.is_empty()
                && matches!(
                    state.status,
                    JobStatus::Complete
                        | JobStatus::Stopped
                        | JobStatus::Cancelled
                        | JobStatus::Failed
                )
            {
                state.status = JobStatus::Active;
            }
        }
        _ => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Invalid queue action",
                "Choose select, append, move, remove, sort, or clear.",
                &back_href,
            );
        }
    }
    match repository
        .update_state(owner, project_id, job_id, &state, form.state_version)
        .await
    {
        Ok(()) => action_response(back_href, Some(job_id.to_owned())),
        Err(error) => job_error_response(error, &back_href),
    }
}
