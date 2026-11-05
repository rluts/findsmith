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
    entities::project,
    jobs::{JobConfigV1, JobRepository, JobState, QueueEntry},
    lists::{validate_namespaces, validate_queue_limit},
    localization,
    projects::{ProjectRepository, ProjectState, ReplacementRule},
    rules::RuleEngine,
    skip::{self, SkipOptions},
    wikis::{self, AccessView, WikiView},
};

use super::AppState;
use super::preview::PagePreviewView;
use super::project_form::{
    legacy_namespaces, legacy_sources, namespace_options, page_source_views,
    page_sources_from_form, parse_namespaces, replacement_rules, resolve_project_wiki_url,
    validate_project_sources,
};
use super::queue_build::StartJobForm;
use super::responses::{
    action_response, csrf_error, error_page, internal_error, job_error_response,
    project_error_response, render,
};
use super::review::localize_queue_entry;

#[derive(Serialize)]
struct WorkspaceResponse<'a> {
    username: &'a str,
    projects: Vec<WorkspaceProjectView>,
    wikis: Vec<WikiView>,
    default_wiki_id: Option<String>,
    csrf_token: &'a str,
}

#[derive(Serialize)]
struct WorkspaceProjectView {
    id: String,
    name: String,
    state_version: i64,
    updated_at: String,
    wiki_id: String,
}

#[derive(Serialize)]
pub(super) struct ProjectResponse<'a> {
    pub(super) project_id: &'a str,
    pub(super) project_name: &'a str,
    pub(super) state_version: i64,
    pub(super) sources: &'a [PageSourceView],
    pub(super) namespace_options: &'a [NamespaceOption],
    pub(super) queue_limit: usize,
    pub(super) wiki_id: &'a str,
    pub(super) awb_access: Option<&'a AccessView>,
    pub(super) rules: &'a [ReplacementRule],
    pub(super) skip: &'a SkipOptions,
    pub(super) csrf_token: &'a str,
    pub(super) page_title: &'a str,
    pub(super) page_preview: Option<&'a PagePreviewView>,
    pub(super) page_preview_error: Option<String>,
    pub(super) latest_job_id: &'a str,
    pub(super) latest_job_status: &'a str,
    pub(super) latest_job_message: &'a str,
    pub(super) latest_job_total: usize,
    pub(super) latest_job_found: usize,
    pub(super) latest_job_duplicates: usize,
    pub(super) latest_job_limit_reached: bool,
    pub(super) latest_job_building: bool,
    pub(super) latest_job_outdated: bool,
    pub(super) latest_queue_preview: &'a [QueueEntry],
}

#[derive(Serialize)]
pub(super) struct PageSourceView {
    pub(super) kind: &'static str,
    pub(super) label: String,
    pub(super) value: String,
    pub(super) summary: String,
}

#[derive(Serialize)]
pub(super) struct NamespaceOption {
    pub(super) id: i64,
    pub(super) label: String,
    pub(super) selected: bool,
}

pub(super) struct LegacySourceLists {
    pub(super) pages: Vec<String>,
    pub(super) categories: Vec<String>,
    pub(super) backlinks: Vec<String>,
    pub(super) transclusions: Vec<String>,
    pub(super) searches: Vec<String>,
}

#[derive(Deserialize)]
pub struct CreateProjectForm {
    name: String,
    wiki_id: String,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct UpdateProjectForm {
    pub(super) name: String,
    pub(super) wiki_id: String,
    #[serde(default)]
    pub(super) source_kind: Vec<String>,
    #[serde(default)]
    pub(super) source_value: Vec<String>,
    #[serde(default)]
    pub(super) namespaces: Vec<String>,
    pub(super) queue_limit: usize,
    #[serde(default)]
    pub(super) rule_name: Vec<String>,
    #[serde(default)]
    pub(super) rule_operation: Vec<String>,
    #[serde(default)]
    pub(super) rule_parameter: Vec<String>,
    #[serde(default)]
    pub(super) rule_mask_comments: Vec<String>,
    #[serde(default)]
    pub(super) rule_mask_tags: Vec<String>,
    #[serde(default)]
    pub(super) rule_mask_references: Vec<String>,
    #[serde(default)]
    pub(super) rule_mask_links: Vec<String>,
    #[serde(default)]
    pub(super) rule_mask_headings: Vec<String>,
    #[serde(default)]
    pub(super) rule_mask_templates: Vec<String>,
    #[serde(default)]
    pub(super) rule_mask_link_targets: Vec<String>,
    #[serde(default)]
    pub(super) rule_find: Vec<String>,
    #[serde(default)]
    pub(super) rule_replace: Vec<String>,
    #[serde(default)]
    pub(super) rule_enabled: Vec<String>,
    #[serde(default)]
    pub(super) rule_type: Vec<String>,
    #[serde(default)]
    pub(super) rule_case: Vec<String>,
    #[serde(default)]
    pub(super) rule_scope: Vec<String>,
    #[serde(default)]
    pub(super) rule_multiline: Vec<String>,
    #[serde(default)]
    pub(super) rule_dot_newline: Vec<String>,
    #[serde(default)]
    pub(super) skip_no_change: bool,
    #[serde(default)]
    pub(super) skip_in_use: bool,
    #[serde(default)]
    pub(super) skip_redirects: bool,
    #[serde(default)]
    pub(super) skip_protected: bool,
    #[serde(default)]
    pub(super) skip_outside_namespaces: bool,
    #[serde(default)]
    pub(super) skip_contains: String,
    #[serde(default)]
    pub(super) skip_not_contains: String,
    #[serde(default)]
    pub(super) skip_regex: bool,
    #[serde(default)]
    pub(super) skip_case_sensitive: bool,
    #[serde(default)]
    pub(super) skip_min_replacements: usize,
    #[serde(default)]
    pub(super) skip_max_replacements: usize,
    #[serde(default)]
    pub(super) page_title: String,
    pub(super) state_version: i64,
    pub(super) csrf_token: String,
}

impl UpdateProjectForm {
    pub(super) fn skip_options(&self) -> SkipOptions {
        SkipOptions {
            no_change: self.skip_no_change,
            in_use_tpl: self.skip_in_use,
            redirects: self.skip_redirects,
            protected: self.skip_protected,
            outside_namespaces: self.skip_outside_namespaces,
            contains: self.skip_contains.clone(),
            not_contains: self.skip_not_contains.clone(),
            regex: self.skip_regex,
            case_sensitive: self.skip_case_sensitive,
            min_replacements: self.skip_min_replacements,
            max_replacements: self.skip_max_replacements,
        }
    }
}

#[derive(Serialize)]
struct AutosaveResponse {
    state_version: i64,
}

#[derive(Deserialize)]
pub struct DeleteProjectForm {
    csrf_token: String,
    confirmation: Option<String>,
}

pub async fn workspace(
    State(state): State<AppState>,
    user: CurrentUser,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response =
        workspace_response(&state.database, &user.central_id, &user.username, &token).await;
    (jar, response)
}

pub async fn create_project(
    State(state): State<AppState>,
    user: CurrentUser,
    jar: PrivateCookieJar,
    Form(form): Form<CreateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    create_project_response(&state.database, &user.central_id, form).await
}

pub async fn project(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response = project_response(
        &state.database,
        &user.central_id,
        &user.username,
        &state.user_agent,
        &id,
        &token,
    )
    .await;
    (jar, response)
}

pub async fn update_project(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    update_project_response(&state.database, &user.central_id, &id, form).await
}

pub async fn delete_project(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<DeleteProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    delete_project_response(&state.database, &user.central_id, &id, form).await
}

pub async fn autosave_project(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    autosave_project_response(&state.database, &user.central_id, &id, form).await
}

pub async fn duplicate_project(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<StartJobForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    duplicate_project_response(&state.database, &user.central_id, &id).await
}

async fn workspace_response(
    database: &DatabaseConnection,
    owner: &str,
    username: &str,
    csrf_token: &str,
) -> Response {
    let wikis = match wikis::list(database).await {
        Ok(items) => items,
        Err(error) => return internal_error(error),
    };
    let default_wiki_id = match wikis::last_used(database, owner).await {
        Ok(id) => id.filter(|id| wikis.iter().any(|wiki| &wiki.id == id)),
        Err(error) => return internal_error(error),
    };
    let repository = ProjectRepository::new(database);
    match repository.list(owner).await {
        Ok(projects) => render(WorkspaceResponse {
            username,
            projects: projects
                .into_iter()
                .map(|project| WorkspaceProjectView {
                    id: project.id,
                    name: project.name,
                    state_version: project.state_version,
                    updated_at: project.updated_at.to_rfc3339(),
                    wiki_id: wikis
                        .iter()
                        .find(|wiki| wiki.api_url == project.wiki_url)
                        .map_or_else(
                            || {
                                url::Url::parse(&project.wiki_url)
                                    .ok()
                                    .and_then(|url| url.host_str().map(str::to_owned))
                                    .unwrap_or(project.wiki_url.clone())
                            },
                            |wiki| wiki.domain.clone(),
                        ),
                })
                .collect(),
            wikis: wikis.into_iter().map(WikiView::from).collect(),
            default_wiki_id,
            csrf_token,
        }),
        Err(error) => project_error_response(error, "/workspace"),
    }
}

async fn create_project_response(
    database: &DatabaseConnection,
    owner: &str,
    form: CreateProjectForm,
) -> Response {
    let Some(wiki) = (match wikis::by_id(database, &form.wiki_id).await {
        Ok(wiki) => wiki,
        Err(error) => return internal_error(error),
    }) else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Unsupported wiki",
            "Select one of the available wikis.",
            "/workspace",
        );
    };
    let repository = ProjectRepository::new(database);
    match repository.create(owner, form.name, wiki.api_url).await {
        Ok(project) => {
            if let Err(error) = wikis::save_last_used(database, owner, &wiki.id).await {
                eprintln!("failed to remember last used wiki: {error}");
            }
            action_response(format!("/projects/{}", project.id), Some(project.id))
        }
        Err(error) => project_error_response(error, "/workspace"),
    }
}

/// Saves the project configuration only. Unlike an explicit save it never touches
/// the review job, so typing in the form cannot restart a review.
async fn autosave_project_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    form: UpdateProjectForm,
) -> Response {
    match save_project(database, owner, id, &form).await {
        Ok((project, _)) => render(AutosaveResponse {
            state_version: project.state_version,
        }),
        Err(response) => *response,
    }
}

async fn duplicate_project_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
) -> Response {
    match ProjectRepository::new(database).duplicate(owner, id).await {
        Ok(Some(project)) => action_response(format!("/projects/{}", project.id), Some(project.id)),
        Ok(None) => error_page(
            StatusCode::NOT_FOUND,
            "findsmith-api-project-not-found",
            "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
            "/workspace",
        ),
        Err(error) => project_error_response(error, &format!("/projects/{id}")),
    }
}

async fn project_response(
    database: &DatabaseConnection,
    owner: &str,
    username: &str,
    user_agent: &str,
    id: &str,
    csrf_token: &str,
) -> Response {
    let repository = ProjectRepository::new(database);
    match repository.get(owner, id).await {
        Ok(Some(project)) => {
            match serde_json::from_value::<ProjectState>(project.state_json.clone()) {
                Ok(state) => {
                    let sources = page_source_views(&state.ordered_sources());
                    let registered = match wikis::by_url(database, &project.wiki_url).await {
                        Ok(wiki) => wiki,
                        Err(error) => return internal_error(error),
                    };
                    let available_namespaces = registered
                        .as_ref()
                        .and_then(|wiki| wikis::namespaces(wiki).ok())
                        .unwrap_or_else(legacy_namespaces);
                    let namespace_options =
                        namespace_options(&available_namespaces, &state.namespaces);
                    let wiki_id = registered
                        .as_ref()
                        .map_or(project.wiki_url.as_str(), |wiki| wiki.id.as_str());
                    let awb_access = match registered.as_ref() {
                        Some(wiki) => Some(wikis::access(wiki, user_agent, username).await),
                        None => {
                            wikis::access_unregistered(&project.wiki_url, user_agent, username)
                                .await
                        }
                    };
                    let latest_job = match JobRepository::new(database).latest(owner, id).await {
                        Ok(job) => job,
                        Err(error) => return job_error_response(error, &format!("/projects/{id}")),
                    };
                    let (
                        latest_job_id,
                        latest_job_status,
                        latest_job_message,
                        latest_job_total,
                        latest_job_found,
                        latest_job_duplicates,
                        latest_job_limit_reached,
                        latest_job_building,
                        latest_job_outdated,
                        latest_queue_preview,
                    ) = match latest_job {
                        Some(job) => {
                            // Autosave updates the project, not the review: report when the
                            // review still runs with an older configuration.
                            let outdated = serde_json::from_value::<JobConfigV1>(job.config_json)
                                .is_ok_and(|config| {
                                    config.rules != state.rules
                                        || config.skip != state.skip
                                        || config.namespaces != state.namespaces
                                });
                            let job_state: JobState = match serde_json::from_value(job.state_json) {
                                Ok(state) => state,
                                Err(error) => return internal_error(error),
                            };
                            let preview = job_state
                                .queue
                                .iter()
                                .take(10)
                                .cloned()
                                .map(localize_queue_entry)
                                .collect();
                            (
                                job.id,
                                job_state.status.label().to_owned(),
                                job_state.generation_message,
                                job_state.queue.len(),
                                job_state.generation_found,
                                job_state.generation_duplicates,
                                job_state.generation_limit_reached,
                                job_state.status.is_building(),
                                outdated,
                                preview,
                            )
                        }
                        None => (
                            String::new(),
                            String::new(),
                            String::new(),
                            0,
                            0,
                            0,
                            false,
                            false,
                            false,
                            Vec::new(),
                        ),
                    };
                    let latest_job_status = localization::from_source(&latest_job_status);
                    let latest_job_message = localization::from_source(&latest_job_message);
                    render_project(ProjectResponse {
                        project_id: &project.id,
                        project_name: &project.name,
                        state_version: project.state_version,
                        sources: &sources,
                        namespace_options: &namespace_options,
                        queue_limit: state.queue_limit,
                        wiki_id,
                        awb_access: awb_access.as_ref(),
                        rules: &state.rules,
                        skip: &state.skip,
                        csrf_token,
                        page_title: "",
                        page_preview: None,
                        page_preview_error: None,
                        latest_job_id: &latest_job_id,
                        latest_job_status: &latest_job_status,
                        latest_job_message: &latest_job_message,
                        latest_job_total,
                        latest_job_found,
                        latest_job_duplicates,
                        latest_job_limit_reached,
                        latest_job_building,
                        latest_job_outdated,
                        latest_queue_preview: &latest_queue_preview,
                    })
                }
                Err(error) => internal_error(error),
            }
        }
        Ok(None) => error_page(
            StatusCode::NOT_FOUND,
            "findsmith-api-project-not-found",
            "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
            "/workspace",
        ),
        Err(error) => project_error_response(error, "/workspace"),
    }
}

pub(super) fn render_project(response: ProjectResponse<'_>) -> Response {
    render(response)
}

async fn update_project_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    form: UpdateProjectForm,
) -> Response {
    if let Ok(Some(job)) = JobRepository::new(database).latest(owner, id).await
        && let Ok(state) = serde_json::from_value::<JobState>(job.state_json)
    {
        if state.edit_in_flight || state.edit_policy_blocked {
            return error_page(
                StatusCode::CONFLICT,
                "findsmith-api-review-is-paused",
                "findsmith-api-check-the-wiki-page-and-build-a-new-list-before-changing-rules-after-an",
                &format!("/projects/{id}"),
            );
        }
        if state.preparse_active || state.status.is_building() {
            return error_page(
                StatusCode::CONFLICT,
                "findsmith-api-review-is-busy",
                "findsmith-api-wait-for-list-generation-or-stop-pre-parse-before-changing-the-rules",
                &format!("/projects/{id}"),
            );
        }
    }
    match save_project(database, owner, id, &form).await {
        Ok((project, state)) => {
            let rules = state.rules.clone();
            let config = JobConfigV1::new(state.rules, state.skip, state.namespaces);
            let jobs = JobRepository::new(database);
            let latest_job = match jobs.latest(owner, id).await {
                Ok(job) => job,
                Err(error) => return job_error_response(error, &format!("/projects/{id}")),
            };
            if let Some(job) = latest_job {
                let old_config: JobConfigV1 = match serde_json::from_value(job.config_json) {
                    Ok(config) => config,
                    Err(error) => return internal_error(error),
                };
                let old_state: JobState = match serde_json::from_value(job.state_json) {
                    Ok(state) => state,
                    Err(error) => return internal_error(error),
                };
                if old_config.rules != rules && !old_state.queue.is_empty() {
                    let next = match jobs
                        .create(
                            owner,
                            id,
                            &project.wiki_url,
                            config,
                            JobState::repeat_with_new_rules(&old_state),
                        )
                        .await
                    {
                        Ok(next) => next,
                        Err(error) => return job_error_response(error, &format!("/projects/{id}")),
                    };
                    return action_response(
                        format!("/projects/{id}/jobs/{}", next.id),
                        Some(next.id),
                    );
                }
                if let Err(error) = jobs.update_config(owner, id, &job.id, &config).await {
                    return job_error_response(error, &format!("/projects/{id}"));
                }
            }
            action_response(format!("/projects/{id}"), Some(project.id))
        }
        Err(response) => *response,
    }
}

pub(super) async fn save_project(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    form: &UpdateProjectForm,
) -> Result<(project::Model, ProjectState), Box<Response>> {
    let back_href = format!("/projects/{id}");
    let rules = match replacement_rules(form) {
        Ok(rules) => rules,
        Err(message) => {
            return Err(Box::new(error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-invalid-replacement-rules",
                &message,
                &back_href,
            )));
        }
    };
    if let Err(error) = RuleEngine::compile(&rules) {
        return Err(Box::new(error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-invalid-replacement-rules",
            &error.to_string(),
            &back_href,
        )));
    }
    let project = match ProjectRepository::new(database).get(owner, id).await {
        Ok(Some(project)) => project,
        Ok(None) => {
            return Err(Box::new(error_page(
                StatusCode::NOT_FOUND,
                "findsmith-api-project-not-found",
                "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
                "/workspace",
            )));
        }
        Err(error) => return Err(Box::new(project_error_response(error, &back_href))),
    };
    match resolve_project_wiki_url(database, id, owner, &form.wiki_id).await {
        Ok(Some(url)) if url == project.wiki_url => {}
        Ok(Some(_)) => {
            return Err(Box::new(error_page(
                StatusCode::BAD_REQUEST,
                "Wiki cannot be changed",
                "A project always edits the wiki it was created for. Create a new project to work on another wiki.",
                &back_href,
            )));
        }
        Ok(None) => {
            return Err(Box::new(error_page(
                StatusCode::BAD_REQUEST,
                "Unsupported wiki",
                "Select one of the available wikis.",
                &back_href,
            )));
        }
        Err(error) => return Err(Box::new(internal_error(error))),
    };
    let queue_limit = match validate_queue_limit(form.queue_limit) {
        Ok(limit) => limit,
        Err(message) => {
            return Err(Box::new(error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-invalid-queue-limit",
                &message,
                &back_href,
            )));
        }
    };
    let namespaces =
        match parse_namespaces(&form.namespaces).and_then(|items| validate_namespaces(&items)) {
            Ok(namespaces) => namespaces,
            Err(message) => {
                return Err(Box::new(error_page(
                    StatusCode::BAD_REQUEST,
                    "findsmith-api-invalid-namespaces",
                    &message,
                    &back_href,
                )));
            }
        };
    if let Ok(Some(wiki)) = wikis::by_id(database, &form.wiki_id).await
        && let Ok(available) = wikis::namespaces(&wiki)
        && namespaces
            .iter()
            .any(|id| !available.iter().any(|entry| entry.id == *id))
    {
        return Err(Box::new(error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-invalid-namespaces",
            "findsmith-api-select-namespaces-available-on-this-wiki",
            &back_href,
        )));
    }
    let sources = match page_sources_from_form(form) {
        Ok(sources) => sources,
        Err(message) => {
            return Err(Box::new(error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-invalid-page-sources",
                &message,
                &back_href,
            )));
        }
    };
    let skip = form.skip_options();
    if let Err(message) = skip::validate(&skip) {
        return Err(Box::new(error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-invalid-skip-options",
            &message,
            &back_href,
        )));
    }
    let legacy = legacy_sources(&sources);
    let state = ProjectState {
        sources,
        pages: legacy.pages,
        categories: legacy.categories,
        backlinks: legacy.backlinks,
        transclusions: legacy.transclusions,
        searches: legacy.searches,
        namespaces,
        queue_limit,
        rules,
        skip,
    };
    if let Err(message) = validate_project_sources(&state) {
        return Err(Box::new(error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-invalid-page-sources",
            &message,
            &back_href,
        )));
    }
    let saved_state = state.clone();
    let repository = ProjectRepository::new(database);
    match repository
        .update(owner, id, form.name.clone(), state, form.state_version)
        .await
    {
        Ok(Some(project)) => Ok((project, saved_state)),
        Ok(None) => Err(Box::new(error_page(
            StatusCode::NOT_FOUND,
            "findsmith-api-project-not-found",
            "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
            "/workspace",
        ))),
        Err(error) => Err(Box::new(project_error_response(error, &back_href))),
    }
}

async fn delete_project_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    form: DeleteProjectForm,
) -> Response {
    let back_href = format!("/projects/{id}");
    if form.confirmation.as_deref() != Some("delete") {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Deletion not confirmed",
            "Confirm that you want to permanently delete this project.",
            &back_href,
        );
    }
    let repository = ProjectRepository::new(database);
    match repository.delete(owner, id).await {
        Ok(true) => action_response("/workspace".to_owned(), Some(id.to_owned())),
        Ok(false) => error_page(
            StatusCode::NOT_FOUND,
            "findsmith-api-project-not-found",
            "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
            "/workspace",
        ),
        Err(error) => project_error_response(error, &back_href),
    }
}
