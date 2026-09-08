use std::collections::HashSet;

use askama::Template;
use axum::{
    extract::{FromRef, Path, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use mwoauth::axum::{AxumOAuthState, AxumUser, Key};
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use crate::{
    auth::Authentication,
    csrf,
    diff::{DiffLine, unified_lines},
    entities::project,
    mediawiki::{PageSnapshot, load_page, wiki_api_url, wiki_id},
    projects::{ProjectError, ProjectRepository, ProjectState, ReplacementRule},
    rules::{RuleEngine, TransformationResult},
};

#[derive(Clone)]
pub struct WebState {
    pub oauth: AxumOAuthState,
    pub database: DatabaseConnection,
    pub user_agent: String,
    pub csrf_secure: bool,
}

impl FromRef<WebState> for AxumOAuthState {
    fn from_ref(state: &WebState) -> Self {
        state.oauth.clone()
    }
}

impl FromRef<WebState> for Key {
    fn from_ref(state: &WebState) -> Self {
        Key::from_ref(&state.oauth)
    }
}

#[derive(Clone)]
pub struct OwnerState {
    pub central_id: String,
    pub username: String,
    pub access_token: String,
    pub database: DatabaseConnection,
    pub user_agent: String,
    pub cookie_key: Key,
    pub csrf_secure: bool,
}

impl FromRef<OwnerState> for Key {
    fn from_ref(state: &OwnerState) -> Self {
        state.cookie_key.clone()
    }
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    username: Option<&'a str>,
    role: Option<&'a str>,
    can_logout: bool,
}

#[derive(Template)]
#[template(path = "workspace.html")]
struct WorkspaceTemplate<'a> {
    username: &'a str,
    projects: &'a [project::Model],
    csrf_token: &'a str,
}

#[derive(Template)]
#[template(path = "project.html")]
struct ProjectTemplate<'a> {
    project_id: &'a str,
    project_name: &'a str,
    state_version: i64,
    pages: &'a str,
    wiki_id: &'a str,
    rules: &'a [ReplacementRule],
    csrf_token: &'a str,
    preview_text: &'a str,
    preview: Option<&'a PreviewView>,
    preview_error: Option<&'a str>,
    page_title: &'a str,
    page_preview: Option<&'a PagePreviewView>,
    page_preview_error: Option<&'a str>,
}

struct PreviewView {
    output: String,
    total_replacements: usize,
    changed: bool,
    rules: Vec<RulePreviewView>,
    diff: Vec<DiffLine>,
}

struct RulePreviewView {
    count: usize,
    changed: bool,
}

struct PagePreviewView {
    title: String,
    full_url: String,
    page_id: u64,
    namespace: i64,
    revision_id: u64,
    revision_timestamp: String,
    server_timestamp: String,
    content_model: String,
    redirect: bool,
    editable: bool,
    protection: String,
    total_replacements: usize,
    changed: bool,
    rules: Vec<RulePreviewView>,
    diff: Vec<DiffLine>,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate<'a> {
    title: &'a str,
    message: &'a str,
    back_href: &'a str,
}

#[derive(Deserialize)]
pub struct CreateProjectForm {
    name: String,
    wiki_id: String,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct UpdateProjectForm {
    name: String,
    wiki_id: String,
    pages: String,
    #[serde(default)]
    rule_find: Vec<String>,
    #[serde(default)]
    rule_replace: Vec<String>,
    #[serde(default)]
    rule_enabled: Vec<String>,
    #[serde(default)]
    rule_type: Vec<String>,
    #[serde(default)]
    rule_case: Vec<String>,
    #[serde(default)]
    rule_scope: Vec<String>,
    #[serde(default)]
    rule_multiline: Vec<String>,
    #[serde(default)]
    rule_dot_newline: Vec<String>,
    #[serde(default)]
    preview_text: String,
    #[serde(default)]
    page_title: String,
    state_version: i64,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct DeleteProjectForm {
    csrf_token: String,
    confirmation: Option<String>,
}

pub async fn web_oauth_index(State(_state): State<WebState>, user: Option<AxumUser>) -> Response {
    let authentication = user.map(|user| Authentication::WebOAuth(Box::new(user)));
    render_index(authentication.as_ref())
}

pub async fn owner_index(State(state): State<OwnerState>) -> Response {
    let authentication = Authentication::OwnerToken {
        username: state.username.clone(),
        access_token: state.access_token.clone(),
    };
    render_index(Some(&authentication))
}

pub async fn web_workspace(
    State(state): State<WebState>,
    user: AxumUser,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response = workspace_response(
        &state.database,
        user.profile().central_id(),
        user.profile().username(),
        &token,
    )
    .await;
    (jar, response)
}

pub async fn owner_workspace(
    State(state): State<OwnerState>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response =
        workspace_response(&state.database, &state.central_id, &state.username, &token).await;
    (jar, response)
}

pub async fn web_create_project(
    State(state): State<WebState>,
    user: AxumUser,
    jar: PrivateCookieJar,
    Form(form): Form<CreateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    create_project_response(&state.database, user.profile().central_id(), form).await
}

pub async fn owner_create_project(
    State(state): State<OwnerState>,
    jar: PrivateCookieJar,
    Form(form): Form<CreateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    create_project_response(&state.database, &state.central_id, form).await
}

pub async fn web_project(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response =
        project_response(&state.database, user.profile().central_id(), &id, &token).await;
    (jar, response)
}

pub async fn owner_project(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response = project_response(&state.database, &state.central_id, &id, &token).await;
    (jar, response)
}

pub async fn web_update_project(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    update_project_response(&state.database, user.profile().central_id(), &id, form).await
}

pub async fn owner_update_project(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    update_project_response(&state.database, &state.central_id, &id, form).await
}

pub async fn web_preview_project(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    preview_project_response(&state.database, user.profile().central_id(), &id, form).await
}

pub async fn owner_preview_project(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    preview_project_response(&state.database, &state.central_id, &id, form).await
}

pub async fn web_page_preview(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    page_preview_response(
        &state.database,
        user.profile().central_id(),
        user.oauth2_token(),
        &state.user_agent,
        &id,
        form,
    )
    .await
}

pub async fn owner_page_preview(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    page_preview_response(
        &state.database,
        &state.central_id,
        &state.access_token,
        &state.user_agent,
        &id,
        form,
    )
    .await
}

pub async fn web_delete_project(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<DeleteProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    delete_project_response(&state.database, user.profile().central_id(), &id, form).await
}

pub async fn owner_delete_project(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<DeleteProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    delete_project_response(&state.database, &state.central_id, &id, form).await
}

pub async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn workspace_response(
    database: &DatabaseConnection,
    owner: &str,
    username: &str,
    csrf_token: &str,
) -> Response {
    let repository = ProjectRepository::new(database);
    match repository.list(owner).await {
        Ok(projects) => render(WorkspaceTemplate {
            username,
            projects: &projects,
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
    let Some(wiki_url) = wiki_api_url(&form.wiki_id) else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Unsupported wiki",
            "Select one of the available wikis.",
            "/workspace",
        );
    };
    let repository = ProjectRepository::new(database);
    match repository
        .create(owner, form.name, wiki_url.to_owned())
        .await
    {
        Ok(project) => Redirect::to(&format!("/projects/{}", project.id)).into_response(),
        Err(error) => project_error_response(error, "/workspace"),
    }
}

async fn project_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    csrf_token: &str,
) -> Response {
    let repository = ProjectRepository::new(database);
    match repository.get(owner, id).await {
        Ok(Some(project)) => {
            match serde_json::from_value::<ProjectState>(project.state_json.clone()) {
                Ok(state) => {
                    let pages = state.pages.join("\n");
                    render_project(ProjectTemplate {
                        project_id: &project.id,
                        project_name: &project.name,
                        state_version: project.state_version,
                        pages: &pages,
                        wiki_id: wiki_id(&project.wiki_url).unwrap_or("ukwiki"),
                        rules: &state.rules,
                        csrf_token,
                        preview_text: "",
                        preview: None,
                        preview_error: None,
                        page_title: "",
                        page_preview: None,
                        page_preview_error: None,
                    })
                }
                Err(error) => internal_error(error),
            }
        }
        Ok(None) => error_page(
            StatusCode::NOT_FOUND,
            "Project not found",
            "This project does not exist or you do not have access to it.",
            "/workspace",
        ),
        Err(error) => project_error_response(error, "/workspace"),
    }
}

async fn preview_project_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    form: UpdateProjectForm,
) -> Response {
    let repository = ProjectRepository::new(database);
    match repository.get(owner, id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_page(
                StatusCode::NOT_FOUND,
                "Project not found",
                "This project does not exist or you do not have access to it.",
                "/workspace",
            );
        }
        Err(error) => return project_error_response(error, "/workspace"),
    };
    let rules = match replacement_rules(&form) {
        Ok(rules) => rules,
        Err(message) => {
            return render_project(ProjectTemplate {
                project_id: id,
                project_name: &form.name,
                state_version: form.state_version,
                pages: &form.pages,
                wiki_id: &form.wiki_id,
                rules: &[],
                csrf_token: &form.csrf_token,
                preview_text: &form.preview_text,
                preview: None,
                preview_error: Some(&message),
                page_title: &form.page_title,
                page_preview: None,
                page_preview_error: None,
            });
        }
    };
    let result = RuleEngine::compile(&rules).and_then(|engine| engine.apply(&form.preview_text));
    match result {
        Ok(result) => {
            let preview = preview_view(&form.preview_text, result);
            render_project(ProjectTemplate {
                project_id: id,
                project_name: &form.name,
                state_version: form.state_version,
                pages: &form.pages,
                wiki_id: &form.wiki_id,
                rules: &rules,
                csrf_token: &form.csrf_token,
                preview_text: &form.preview_text,
                preview: Some(&preview),
                preview_error: None,
                page_title: &form.page_title,
                page_preview: None,
                page_preview_error: None,
            })
        }
        Err(error) => {
            let message = error.to_string();
            render_project(ProjectTemplate {
                project_id: id,
                project_name: &form.name,
                state_version: form.state_version,
                pages: &form.pages,
                wiki_id: &form.wiki_id,
                rules: &rules,
                csrf_token: &form.csrf_token,
                preview_text: &form.preview_text,
                preview: None,
                preview_error: Some(&message),
                page_title: &form.page_title,
                page_preview: None,
                page_preview_error: None,
            })
        }
    }
}

async fn page_preview_response(
    database: &DatabaseConnection,
    owner: &str,
    access_token: &str,
    user_agent: &str,
    id: &str,
    form: UpdateProjectForm,
) -> Response {
    let repository = ProjectRepository::new(database);
    match repository.get(owner, id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return error_page(
                StatusCode::NOT_FOUND,
                "Project not found",
                "This project does not exist or you do not have access to it.",
                "/workspace",
            );
        }
        Err(error) => return project_error_response(error, "/workspace"),
    };
    let rules = match replacement_rules(&form) {
        Ok(rules) => rules,
        Err(message) => {
            return render_page_preview_error(id, &form, &[], &message);
        }
    };
    let engine = match RuleEngine::compile(&rules) {
        Ok(engine) => engine,
        Err(error) => return render_page_preview_error(id, &form, &rules, &error.to_string()),
    };
    let Some(wiki_url) = wiki_api_url(&form.wiki_id) else {
        return render_page_preview_error(id, &form, &rules, "Select one of the available wikis.");
    };
    let snapshot = match load_page(wiki_url, user_agent, access_token, &form.page_title).await {
        Ok(snapshot) => snapshot,
        Err(error) => return render_page_preview_error(id, &form, &rules, &error.to_string()),
    };
    let result = match engine.apply(&snapshot.content) {
        Ok(result) => result,
        Err(error) => return render_page_preview_error(id, &form, &rules, &error.to_string()),
    };
    let page_preview = page_preview_view(snapshot, result);
    render_project(ProjectTemplate {
        project_id: id,
        project_name: &form.name,
        state_version: form.state_version,
        pages: &form.pages,
        wiki_id: &form.wiki_id,
        rules: &rules,
        csrf_token: &form.csrf_token,
        preview_text: &form.preview_text,
        preview: None,
        preview_error: None,
        page_title: &form.page_title,
        page_preview: Some(&page_preview),
        page_preview_error: None,
    })
}

fn render_page_preview_error(
    id: &str,
    form: &UpdateProjectForm,
    rules: &[ReplacementRule],
    message: &str,
) -> Response {
    render_project(ProjectTemplate {
        project_id: id,
        project_name: &form.name,
        state_version: form.state_version,
        pages: &form.pages,
        wiki_id: &form.wiki_id,
        rules,
        csrf_token: &form.csrf_token,
        preview_text: &form.preview_text,
        preview: None,
        preview_error: None,
        page_title: &form.page_title,
        page_preview: None,
        page_preview_error: Some(message),
    })
}

fn page_preview_view(snapshot: PageSnapshot, result: TransformationResult) -> PagePreviewView {
    let protection = if snapshot.protection.is_empty() {
        "None".to_owned()
    } else {
        snapshot
            .protection
            .iter()
            .map(|item| format!("{}: {} until {}", item.kind, item.level, item.expiry))
            .collect::<Vec<_>>()
            .join(", ")
    };
    PagePreviewView {
        title: snapshot.title,
        full_url: snapshot.full_url,
        page_id: snapshot.page_id,
        namespace: snapshot.namespace,
        revision_id: snapshot.revision_id,
        revision_timestamp: snapshot.revision_timestamp,
        server_timestamp: snapshot.server_timestamp,
        content_model: snapshot.content_model,
        redirect: snapshot.redirect,
        editable: snapshot.editable,
        protection,
        total_replacements: result.total_replacements,
        changed: result.total_replacements > 0,
        rules: result
            .counts
            .into_iter()
            .zip(result.changed)
            .map(|(count, changed)| RulePreviewView { count, changed })
            .collect(),
        diff: unified_lines(&snapshot.content, &result.text),
    }
}

fn preview_view(input: &str, result: TransformationResult) -> PreviewView {
    let diff = unified_lines(input, &result.text);
    PreviewView {
        output: result.text,
        total_replacements: result.total_replacements,
        changed: result.total_replacements > 0,
        rules: result
            .counts
            .into_iter()
            .zip(result.changed)
            .map(|(count, changed)| RulePreviewView { count, changed })
            .collect(),
        diff,
    }
}

fn render_project(template: ProjectTemplate<'_>) -> Response {
    render(template)
}

async fn update_project_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    form: UpdateProjectForm,
) -> Response {
    let back_href = format!("/projects/{id}");
    let rules = match replacement_rules(&form) {
        Ok(rules) => rules,
        Err(message) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Invalid replacement rules",
                &message,
                &back_href,
            );
        }
    };
    if let Err(error) = RuleEngine::compile(&rules).and_then(|engine| engine.apply("")) {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Invalid replacement rules",
            &error.to_string(),
            &back_href,
        );
    }
    let Some(wiki_url) = wiki_api_url(&form.wiki_id) else {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Unsupported wiki",
            "Select one of the available wikis.",
            &back_href,
        );
    };
    let state = ProjectState {
        pages: form
            .pages
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        rules,
    };
    let repository = ProjectRepository::new(database);
    match repository
        .update(
            owner,
            id,
            form.name,
            wiki_url.to_owned(),
            state,
            form.state_version,
        )
        .await
    {
        Ok(Some(_)) => Redirect::to(&back_href).into_response(),
        Ok(None) => error_page(
            StatusCode::NOT_FOUND,
            "Project not found",
            "This project does not exist or you do not have access to it.",
            "/workspace",
        ),
        Err(error) => project_error_response(error, &back_href),
    }
}

fn replacement_rules(form: &UpdateProjectForm) -> Result<Vec<ReplacementRule>, String> {
    let count = form.rule_find.len();
    if form.rule_replace.len() != count
        || form.rule_type.len() != count
        || form.rule_case.len() != count
        || form.rule_scope.len() != count
    {
        return Err(
            "The submitted rule fields are incomplete. Reload the page and try again.".to_owned(),
        );
    }

    let enabled = rule_indexes(&form.rule_enabled, count)?;
    let multiline = rule_indexes(&form.rule_multiline, count)?;
    let dot_newline = rule_indexes(&form.rule_dot_newline, count)?;
    let mut rules = Vec::with_capacity(count);
    for index in 0..count {
        if form.rule_find[index].is_empty() && form.rule_replace[index].is_empty() {
            continue;
        }
        let regex = match form.rule_type[index].as_str() {
            "plain" => false,
            "regex" => true,
            _ => return Err(format!("Rule {} has an invalid type.", index + 1)),
        };
        let case_sensitive = match form.rule_case[index].as_str() {
            "sensitive" => true,
            "insensitive" => false,
            _ => return Err(format!("Rule {} has an invalid case option.", index + 1)),
        };
        let replace_all = match form.rule_scope[index].as_str() {
            "all" => true,
            "first" => false,
            _ => {
                return Err(format!(
                    "Rule {} has an invalid replacement scope.",
                    index + 1
                ));
            }
        };
        rules.push(ReplacementRule {
            enabled: enabled.contains(&index),
            find: form.rule_find[index].clone(),
            replace: form.rule_replace[index].clone(),
            regex,
            case_sensitive,
            replace_all,
            multiline: multiline.contains(&index),
            dot_matches_new_line: dot_newline.contains(&index),
        });
    }
    Ok(rules)
}

fn rule_indexes(values: &[String], count: usize) -> Result<HashSet<usize>, String> {
    values
        .iter()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "A submitted rule index is invalid.".to_owned())
                .and_then(|index| {
                    if index < count {
                        Ok(index)
                    } else {
                        Err("A submitted rule index is out of range.".to_owned())
                    }
                })
        })
        .collect()
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
        Ok(true) => Redirect::to("/workspace").into_response(),
        Ok(false) => error_page(
            StatusCode::NOT_FOUND,
            "Project not found",
            "This project does not exist or you do not have access to it.",
            "/workspace",
        ),
        Err(error) => project_error_response(error, &back_href),
    }
}

fn render_index(authentication: Option<&Authentication>) -> Response {
    if let Some(authentication) = authentication {
        debug_assert!(!authentication.access_token().is_empty());
    }
    render(IndexTemplate {
        username: authentication.map(Authentication::username),
        role: authentication.map(|authentication| authentication.role().as_str()),
        can_logout: authentication.is_some_and(Authentication::can_logout),
    })
}

fn project_error_response(error: ProjectError, back_href: &str) -> Response {
    match error {
        ProjectError::InvalidName => error_page(
            StatusCode::BAD_REQUEST,
            "Invalid project name",
            "Project name must contain between 1 and 255 characters.",
            back_href,
        ),
        ProjectError::DuplicateName => error_page(
            StatusCode::CONFLICT,
            "Project name already exists",
            "Choose a different name for this project.",
            back_href,
        ),
        ProjectError::Conflict => error_page(
            StatusCode::CONFLICT,
            "Project changed elsewhere",
            "Reload the project to get the latest version, then apply your changes again.",
            back_href,
        ),
        ProjectError::Database(error) => internal_error(error),
        ProjectError::Serialization(error) => internal_error(error),
    }
}

fn csrf_error(back_href: &str) -> Response {
    error_page(
        StatusCode::FORBIDDEN,
        "Request could not be verified",
        "Reload the page and try again. No changes were made.",
        back_href,
    )
}

fn error_page(status: StatusCode, title: &str, message: &str, back_href: &str) -> Response {
    render_with_status(
        status,
        ErrorTemplate {
            title,
            message,
            back_href,
        },
    )
}

fn render(template: impl Template) -> Response {
    render_with_status(StatusCode::OK, template)
}

fn render_with_status(status: StatusCode, template: impl Template) -> Response {
    match template.render() {
        Ok(page) => (
            status,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::REFERRER_POLICY, "same-origin"),
            ],
            Html(page),
        )
            .into_response(),
        Err(error) => internal_error(error),
    }
}

fn internal_error(error: impl std::error::Error) -> Response {
    eprintln!("request failed: {error}");
    error_page_without_rendering()
}

fn error_page_without_rendering() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        "An unexpected error occurred.",
    )
        .into_response()
}
