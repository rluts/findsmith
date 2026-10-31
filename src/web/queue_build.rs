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
    jobs::{JobConfigV1, JobError, JobRepository, JobState, JobStatus, QueueEntry},
    lists::{queue_from_normalized, validate_queue_limit},
    localization,
    mediawiki::{
        ApiPageListKind, PageListKind, api_page_list, edit_permissions, normalize_titles,
        page_list, page_list_bot,
    },
    policy,
    projects::{PageSourceKind, ProjectRepository, ProjectState},
    rules::RuleEngine,
};

use super::AppState;
use super::project_form::validate_project_sources;
use super::projects::{UpdateProjectForm, save_project};
use super::responses::{
    action_response, csrf_error, error_page, internal_error, job_error_response,
    project_error_response,
};

#[derive(Deserialize)]
pub struct StartJobForm {
    pub(super) csrf_token: String,
}

pub async fn build_project(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    if let Err(response) = save_project(&state.database, &user.central_id, &id, &form).await {
        return *response;
    }
    start_job_response(
        &state.database,
        &user.central_id,
        &user.username,
        &user.access_token,
        &state.user_agent,
        &id,
        true,
    )
    .await
}

pub async fn start_job(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<StartJobForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    start_job_response(
        &state.database,
        &user.central_id,
        &user.username,
        &user.access_token,
        &state.user_agent,
        &id,
        false,
    )
    .await
}

async fn start_job_response(
    database: &DatabaseConnection,
    owner: &str,
    username: &str,
    access_token: &str,
    user_agent: &str,
    project_id: &str,
    return_to_project: bool,
) -> Response {
    let back_href = format!("/projects/{project_id}");
    let project = match ProjectRepository::new(database)
        .get(owner, project_id)
        .await
    {
        Ok(Some(project)) => project,
        Ok(None) => {
            return error_page(
                StatusCode::NOT_FOUND,
                "findsmith-api-project-not-found",
                "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
                "/workspace",
            );
        }
        Err(error) => return project_error_response(error, "/workspace"),
    };
    let state: ProjectState = match serde_json::from_value(project.state_json) {
        Ok(state) => state,
        Err(error) => return internal_error(error),
    };
    if let Err(error) = RuleEngine::compile(&state.rules) {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-invalid-replacement-rules",
            &error.to_string(),
            &back_href,
        );
    }
    if let Err(message) = validate_project_sources(&state) {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-cannot-start-review",
            &message,
            &back_href,
        );
    }
    if !state.has_sources() {
        return error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-cannot-start-review",
            "findsmith-api-add-at-least-one-page-source",
            &back_href,
        );
    }
    let queue_limit = match validate_queue_limit(state.queue_limit) {
        Ok(limit) => limit,
        Err(message) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-cannot-start-review",
                &message,
                &back_href,
            );
        }
    };
    match edit_permissions(&project.wiki_url, user_agent, access_token).await {
        Ok(permissions) => {
            if let Some(message) = policy::job_start_blocker(&permissions) {
                return error_page(
                    StatusCode::FORBIDDEN,
                    "findsmith-api-cannot-start-review",
                    message,
                    &back_href,
                );
            }
        }
        Err(_) => {
            return error_page(
                StatusCode::BAD_GATEWAY,
                "findsmith-api-cannot-start-review",
                "findsmith-api-start-rights-unverified",
                &back_href,
            );
        }
    }
    let config = JobConfigV1::new(
        state.rules.clone(),
        state.skip.clone(),
        state.namespaces.clone(),
    );
    let job_state = JobState::building(queue_limit);
    match JobRepository::new(database)
        .create(owner, project_id, &project.wiki_url, config, job_state)
        .await
    {
        Ok(job) => {
            let redirect = if return_to_project {
                format!("/projects/{project_id}")
            } else {
                format!("/projects/{project_id}/jobs/{}", job.id)
            };
            let database = database.clone();
            let owner = owner.to_owned();
            let username = username.to_owned();
            let project_id = project_id.to_owned();
            let wiki_url = project.wiki_url;
            let job_id = job.id.clone();
            let access_token = access_token.to_owned();
            let user_agent = user_agent.to_owned();
            let language = localization::current_language();
            tokio::spawn(localization::in_language(language, async move {
                build_job_queue(
                    database,
                    owner,
                    username,
                    project_id,
                    job_id,
                    wiki_url,
                    access_token,
                    user_agent,
                    state,
                )
                .await;
            }));
            action_response(redirect, Some(job.id))
        }
        Err(error) => job_error_response(error, &back_href),
    }
}

#[allow(clippy::too_many_arguments)]
async fn build_job_queue(
    database: DatabaseConnection,
    owner: String,
    username: String,
    project_id: String,
    job_id: String,
    wiki_url: String,
    access_token: String,
    user_agent: String,
    state: ProjectState,
) {
    let bot = match page_list_bot(&wiki_url, &username, &access_token, &user_agent).await {
        Ok(bot) => bot,
        Err(error) => {
            fail_generation(
                &database,
                &owner,
                &project_id,
                &job_id,
                &localization::required_with_args(
                    "findsmith-api-generation-init-failed",
                    &[&error.to_string()],
                ),
            )
            .await;
            return;
        }
    };
    for source in state.ordered_sources() {
        if source.kind == PageSourceKind::Manual {
            if !record_step(
                &database,
                &owner,
                &project_id,
                &job_id,
                Vec::new(),
                "findsmith-api-generation-loading-manual",
                false,
            )
            .await
            {
                return;
            }
            match normalize_titles(&wiki_url, &user_agent, &access_token, &source.values).await {
                Ok(mut pages) => {
                    filter_namespaces(&mut pages, &state.namespaces);
                    let message = localization::required_with_args(
                        "findsmith-api-generation-loaded-manual",
                        &[&pages.len().to_string()],
                    );
                    if !record_step(
                        &database,
                        &owner,
                        &project_id,
                        &job_id,
                        queue_from_normalized(pages),
                        &message,
                        false,
                    )
                    .await
                    {
                        return;
                    }
                }
                Err(error) => {
                    fail_generation(
                        &database,
                        &owner,
                        &project_id,
                        &job_id,
                        &localization::required_with_args(
                            "findsmith-api-generation-manual-failed",
                            &[&error.to_string()],
                        ),
                    )
                    .await;
                    return;
                }
            }
            continue;
        }

        let (kind, label) = match source.kind {
            PageSourceKind::Category => (
                Some(PageListKind::CategoryMembers),
                "findsmith-api-source-category-members",
            ),
            PageSourceKind::Backlinks => (
                Some(PageListKind::Backlinks),
                "findsmith-api-source-backlinks",
            ),
            PageSourceKind::Transclusions => (
                Some(PageListKind::Transclusions),
                "findsmith-api-source-transclusions",
            ),
            PageSourceKind::Search => (
                Some(PageListKind::Search),
                "findsmith-api-source-search-results",
            ),
            PageSourceKind::Watchlist => (None, "findsmith-api-source-watchlist-pages"),
            PageSourceKind::Contributions => (None, "findsmith-api-source-contribution-pages"),
            PageSourceKind::LinksOnPage => (None, "findsmith-api-source-linked-pages"),
            PageSourceKind::CategoriesOnPage => (None, "findsmith-api-source-categories-on-page"),
            PageSourceKind::RedirectsToPage => (None, "findsmith-api-source-redirect-pages"),
            PageSourceKind::Manual => unreachable!(),
        };
        let label = localization::required(label);
        for value in &source.values {
            let current =
                match current_generation_size(&database, &owner, &project_id, &job_id).await {
                    Ok(Some(size)) => size,
                    Ok(None) => return,
                    Err(error) => {
                        abandon_build(&database, &owner, &project_id, &job_id, &error).await;
                        return;
                    }
                };
            if current >= state.queue_limit {
                break;
            }
            let loading_message = localization::required_with_args(
                "findsmith-api-generation-loading-source",
                &[&label, value],
            );
            if !record_step(
                &database,
                &owner,
                &project_id,
                &job_id,
                Vec::new(),
                &loading_message,
                false,
            )
            .await
            {
                return;
            }
            let pages = if let Some(kind) = kind {
                page_list(
                    &bot,
                    kind,
                    value,
                    &state.namespaces,
                    state.queue_limit - current,
                )
                .await
                .map_err(|error| error.to_string())
            } else {
                let kind = match source.kind {
                    PageSourceKind::Watchlist => ApiPageListKind::Watchlist,
                    PageSourceKind::Contributions => ApiPageListKind::Contributions,
                    PageSourceKind::LinksOnPage => ApiPageListKind::LinksOnPage,
                    PageSourceKind::CategoriesOnPage => ApiPageListKind::CategoriesOnPage,
                    PageSourceKind::RedirectsToPage => ApiPageListKind::RedirectsToPage,
                    _ => unreachable!(),
                };
                api_page_list(
                    &wiki_url,
                    &user_agent,
                    &access_token,
                    kind,
                    value,
                    &state.namespaces,
                    state.queue_limit - current,
                )
                .await
            };
            match pages {
                Ok(pages) => {
                    let message = localization::required_with_args(
                        "findsmith-api-generation-loaded-source",
                        &[&label, value],
                    );
                    if !record_step(
                        &database,
                        &owner,
                        &project_id,
                        &job_id,
                        queue_from_normalized(pages),
                        &message,
                        false,
                    )
                    .await
                    {
                        return;
                    }
                }
                Err(error) => {
                    fail_generation(
                        &database,
                        &owner,
                        &project_id,
                        &job_id,
                        &localization::required_with_args(
                            "findsmith-api-generation-source-failed",
                            &[&label, &error],
                        ),
                    )
                    .await;
                    return;
                }
            }
        }
    }
    record_step(
        &database,
        &owner,
        &project_id,
        &job_id,
        Vec::new(),
        "findsmith-api-queue-ready",
        true,
    )
    .await;
}

/// Records a build step. Returns false when the build must stop: the job is
/// no longer building, or its progress could not be saved, in which case the
/// build is failed rather than left building.
#[allow(clippy::too_many_arguments)]
async fn record_step(
    database: &DatabaseConnection,
    owner: &str,
    project_id: &str,
    job_id: &str,
    entries: Vec<QueueEntry>,
    message: &str,
    finish: bool,
) -> bool {
    match update_generation(
        database, owner, project_id, job_id, entries, message, finish, false,
    )
    .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => {
            abandon_build(database, owner, project_id, job_id, &error).await;
            false
        }
    }
}

async fn abandon_build(
    database: &DatabaseConnection,
    owner: &str,
    project_id: &str,
    job_id: &str,
    error: &JobError,
) {
    eprintln!("FindSmith: queue build {job_id} could not save its progress: {error}");
    fail_generation(
        database,
        owner,
        project_id,
        job_id,
        "findsmith-api-generation-update-failed",
    )
    .await;
}

/// How long a build may go without saving progress before it is treated as
/// abandoned. Each source saves before and after it is fetched.
const ABANDONED_BUILD_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(10);
const BUILD_RECOVERY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Builds run inside the server process, so a restart or a crashed build task
/// leaves its job building forever. At startup this fails every build that
/// was not updated after the process started; afterwards it periodically
/// fails builds that stopped saving progress.
pub fn spawn_build_recovery(database: DatabaseConnection) {
    let started = chrono::Utc::now();
    tokio::spawn(async move {
        recover_builds(&database, started).await;
        let mut interval = tokio::time::interval(BUILD_RECOVERY_INTERVAL);
        interval.tick().await;
        loop {
            interval.tick().await;
            recover_builds(&database, chrono::Utc::now() - ABANDONED_BUILD_AFTER).await;
        }
    });
}

async fn recover_builds(database: &DatabaseConnection, cutoff: chrono::DateTime<chrono::Utc>) {
    match JobRepository::new(database)
        .fail_abandoned_builds(cutoff, "findsmith-api-generation-interrupted")
        .await
    {
        Ok(0) => {}
        Ok(count) => println!("FindSmith: failed {count} interrupted queue build(s)"),
        Err(error) => eprintln!("FindSmith: could not recover interrupted queue builds: {error}"),
    }
}

fn filter_namespaces(pages: &mut Vec<crate::mediawiki::NormalizedTitle>, namespaces: &[i64]) {
    if !namespaces.is_empty() {
        pages.retain(|page| {
            page.namespace
                .is_some_and(|namespace| namespaces.contains(&namespace))
        });
    }
}

async fn current_generation_size(
    database: &DatabaseConnection,
    owner: &str,
    project_id: &str,
    job_id: &str,
) -> Result<Option<usize>, JobError> {
    let Some(job) = JobRepository::new(database)
        .get(owner, project_id, job_id)
        .await?
    else {
        return Ok(None);
    };
    let state: JobState = serde_json::from_value(job.state_json)?;
    Ok((state.status == JobStatus::Building).then_some(state.queue.len()))
}

#[allow(clippy::too_many_arguments)]
async fn update_generation(
    database: &DatabaseConnection,
    owner: &str,
    project_id: &str,
    job_id: &str,
    entries: Vec<QueueEntry>,
    message: &str,
    finish: bool,
    failed: bool,
) -> Result<Option<usize>, JobError> {
    for _ in 0..4 {
        let repository = JobRepository::new(database);
        let Some(job) = repository.get(owner, project_id, job_id).await? else {
            return Ok(None);
        };
        let mut state: JobState = serde_json::from_value(job.state_json)?;
        if state.status != JobStatus::Building {
            return Ok(None);
        }
        state.append_unique(entries.clone());
        state.generation_message = message.to_owned();
        if failed {
            state.status = JobStatus::Failed;
        }
        if finish {
            state.finish_build();
        }
        let size = state.queue.len();
        match repository
            .update_state(owner, project_id, job_id, &state, job.state_version)
            .await
        {
            Ok(()) => return Ok(Some(size)),
            Err(JobError::Conflict) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(JobError::Conflict)
}

async fn fail_generation(
    database: &DatabaseConnection,
    owner: &str,
    project_id: &str,
    job_id: &str,
    message: &str,
) {
    // If even this fails, build recovery fails the job once it goes stale.
    if let Err(error) = update_generation(
        database,
        owner,
        project_id,
        job_id,
        Vec::new(),
        message,
        false,
        true,
    )
    .await
    {
        eprintln!("FindSmith: could not mark queue build {job_id} as failed: {error}");
    }
}
