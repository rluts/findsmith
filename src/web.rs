use std::collections::HashSet;

use axum::{
    Json,
    extract::{FromRef, Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use mwoauth::axum::{AxumOAuthState, AxumUser, Key};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::{
    auth::Authentication,
    csrf,
    diff::{DiffLine, SideBySideLine, side_by_side_lines, unified_lines},
    entities::project,
    jobs::{
        JobConfigV1, JobError, JobRepository, JobState, JobStatus, QueueEntry, QueueEntryStatus,
    },
    lists::{queue_from_normalized, source_values, validate_namespaces, validate_queue_limit},
    mediawiki::{
        ApiPageListKind, PageListKind, PageSnapshot, api_page_list, load_page, normalize_titles,
        page_list, page_list_bot, parse_preview,
    },
    projects::{
        PageSource, PageSourceKind, ProjectError, ProjectRepository, ProjectState, ReplacementRule,
        RuleMask, RuleOperation,
    },
    rules::{RuleEngine, TransformationResult},
    saved_rules,
    structural::apply_with_wiki,
    wikis::{self, AccessView, WikiView},
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

#[derive(Serialize)]
struct SessionResponse<'a> {
    username: Option<&'a str>,
    groups: Vec<String>,
    groups_error: Option<String>,
    can_logout: bool,
}

#[derive(Default, Deserialize)]
pub struct SessionQuery {
    #[serde(default)]
    include_groups: bool,
    wiki_id: Option<String>,
}

#[derive(Serialize)]
struct WorkspaceResponse<'a> {
    username: &'a str,
    projects: Vec<WorkspaceProjectView>,
    wikis: Vec<WikiView>,
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
struct ProjectResponse<'a> {
    project_id: &'a str,
    project_name: &'a str,
    state_version: i64,
    sources: &'a [PageSourceView],
    namespace_options: &'a [NamespaceOption],
    queue_limit: usize,
    wiki_id: &'a str,
    awb_access: Option<&'a AccessView>,
    rules: &'a [ReplacementRule],
    csrf_token: &'a str,
    page_title: &'a str,
    page_preview: Option<&'a PagePreviewView>,
    page_preview_error: Option<&'a str>,
    latest_job_id: &'a str,
    latest_job_status: &'a str,
    latest_job_message: &'a str,
    latest_job_total: usize,
    latest_job_found: usize,
    latest_job_duplicates: usize,
    latest_job_limit_reached: bool,
    latest_job_building: bool,
    latest_queue_preview: &'a [QueueEntry],
}

#[derive(Serialize)]
struct PageSourceView {
    kind: &'static str,
    label: &'static str,
    value: String,
    summary: String,
}

#[derive(Serialize)]
struct NamespaceOption {
    id: i64,
    label: String,
    selected: bool,
}

struct LegacySourceLists {
    pages: Vec<String>,
    categories: Vec<String>,
    backlinks: Vec<String>,
    transclusions: Vec<String>,
    searches: Vec<String>,
}

#[derive(Serialize)]
struct RulePreviewView {
    count: usize,
    changed: bool,
}

#[derive(Serialize)]
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
    proposed_text: String,
    rules: Vec<RulePreviewView>,
    unified_diff: Vec<DiffLine>,
    side_by_side_diff: Vec<SideBySideLine>,
}

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
    skipped: usize,
    unavailable: usize,
    rule_count: usize,
    generation_message: &'a str,
    preparse_active: bool,
    preparse_cursor: usize,
    preparse_render_equivalent: bool,
    queue: &'a [QueueEntry],
    current: Option<&'a QueueEntry>,
    page: Option<&'a ReviewPageView>,
    page_error: Option<&'a str>,
}

#[derive(Serialize)]
struct ReviewPageView {
    full_url: String,
    revision_id: u64,
    revision_timestamp: String,
    server_timestamp: String,
    content_model: String,
    editable: bool,
    protection: String,
    total_replacements: usize,
    changed: bool,
    rules: Vec<RulePreviewView>,
    proposed_text: String,
    unified_diff: Vec<DiffLine>,
    side_by_side_diff: Vec<SideBySideLine>,
}

#[derive(Serialize)]
struct ErrorResponse<'a> {
    title: &'a str,
    message: &'a str,
    back_href: &'a str,
}

#[derive(Serialize)]
struct ActionResponse {
    ok: bool,
    redirect: String,
    id: Option<String>,
}

fn action_response(redirect: String, id: Option<String>) -> Response {
    Json(ActionResponse {
        ok: true,
        redirect,
        id,
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct CreateProjectForm {
    name: String,
    wiki_id: String,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct AddWikiForm {
    domain: String,
    csrf_token: String,
}

#[derive(Serialize)]
struct WikisResponse {
    wikis: Vec<WikiView>,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct UpdateProjectForm {
    name: String,
    wiki_id: String,
    #[serde(default)]
    source_kind: Vec<String>,
    #[serde(default)]
    source_value: Vec<String>,
    #[serde(default)]
    namespaces: Vec<String>,
    queue_limit: usize,
    #[serde(default)]
    rule_name: Vec<String>,
    #[serde(default)]
    rule_operation: Vec<String>,
    #[serde(default)]
    rule_group: Vec<String>,
    #[serde(default)]
    rule_group_contains: Vec<String>,
    #[serde(default)]
    rule_parameter: Vec<String>,
    #[serde(default)]
    rule_mask_comments: Vec<String>,
    #[serde(default)]
    rule_mask_tags: Vec<String>,
    #[serde(default)]
    rule_mask_references: Vec<String>,
    #[serde(default)]
    rule_mask_links: Vec<String>,
    #[serde(default)]
    rule_mask_headings: Vec<String>,
    #[serde(default)]
    rule_mask_templates: Vec<String>,
    #[serde(default)]
    rule_mask_link_targets: Vec<String>,
    #[serde(default)]
    rule_group_invert: Vec<String>,
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
    page_title: String,
    state_version: i64,
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct DeleteProjectForm {
    csrf_token: String,
    confirmation: Option<String>,
}

#[derive(Deserialize)]
pub struct StartJobForm {
    csrf_token: String,
}

#[derive(Deserialize)]
pub struct JobActionForm {
    csrf_token: String,
    state_version: i64,
    action: String,
    #[serde(default)]
    render_equivalent: bool,
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

#[derive(Deserialize)]
pub struct RecomputeForm {
    csrf_token: String,
    proposed_text: String,
}

#[derive(Deserialize)]
pub struct RenderPreviewForm {
    csrf_token: String,
    title: String,
    proposed_text: String,
}

#[derive(Serialize)]
struct RenderedPreviewResponse {
    title: String,
    wiki_origin: String,
    html: String,
    module_styles: Vec<String>,
}

pub async fn web_render_preview(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<RenderPreviewForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    render_preview_response(
        &state.database,
        user.profile().central_id(),
        user.oauth2_token(),
        &state.user_agent,
        &id,
        form,
    )
    .await
}

pub async fn owner_render_preview(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<RenderPreviewForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    render_preview_response(
        &state.database,
        &state.central_id,
        &state.access_token,
        &state.user_agent,
        &id,
        form,
    )
    .await
}

async fn render_preview_response(
    database: &DatabaseConnection,
    owner: &str,
    access_token: &str,
    user_agent: &str,
    project_id: &str,
    form: RenderPreviewForm,
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
                "Project not found",
                "This project does not exist or you do not have access to it.",
                "/workspace",
            );
        }
        Err(error) => return project_error_response(error, "/workspace"),
    };
    if form.title.trim().is_empty()
        || form.title.chars().count() > 255
        || form.proposed_text.len() > crate::rules::MAX_OUTPUT_BYTES
    {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Invalid preview",
            "The title, wiki, or proposed wikitext is invalid.",
            &back_href,
        );
    }
    let parsed = match parse_preview(
        &project.wiki_url,
        user_agent,
        access_token,
        form.title.trim(),
        &form.proposed_text,
    )
    .await
    {
        Ok(parsed) => parsed,
        Err(error) => {
            return error_page(
                StatusCode::BAD_GATEWAY,
                "Could not render preview",
                &error.to_string(),
                &back_href,
            );
        }
    };
    let origin = match url::Url::parse(&project.wiki_url) {
        Ok(url) => url.origin().ascii_serialization(),
        Err(error) => return internal_error(error),
    };
    Json(RenderedPreviewResponse {
        title: parsed.title,
        wiki_origin: origin,
        html: parsed.html,
        module_styles: parsed.module_styles,
    })
    .into_response()
}

#[derive(Deserialize)]
pub struct SaveRuleForm {
    csrf_token: String,
    rule_json: String,
}

#[derive(Deserialize)]
pub struct DeleteRuleForm {
    csrf_token: String,
}

pub async fn web_saved_rules(State(state): State<WebState>, user: AxumUser) -> Response {
    saved_rules_response(&state.database, user.profile().central_id()).await
}

pub async fn owner_saved_rules(State(state): State<OwnerState>) -> Response {
    saved_rules_response(&state.database, &state.central_id).await
}

async fn saved_rules_response(database: &DatabaseConnection, owner: &str) -> Response {
    match saved_rules::list(database, owner).await {
        Ok(rules) => Json(rules).into_response(),
        Err(message) => error_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Saved rules unavailable",
            &message,
            "/workspace",
        ),
    }
}

pub async fn web_save_rule(
    State(state): State<WebState>,
    user: AxumUser,
    jar: PrivateCookieJar,
    Form(form): Form<SaveRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    save_rule_response(&state.database, user.profile().central_id(), None, form).await
}

pub async fn owner_save_rule(
    State(state): State<OwnerState>,
    jar: PrivateCookieJar,
    Form(form): Form<SaveRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    save_rule_response(&state.database, &state.central_id, None, form).await
}

pub async fn web_update_saved_rule(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<SaveRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    save_rule_response(
        &state.database,
        user.profile().central_id(),
        Some(&id),
        form,
    )
    .await
}

pub async fn owner_update_saved_rule(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<SaveRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    save_rule_response(&state.database, &state.central_id, Some(&id), form).await
}

async fn save_rule_response(
    database: &DatabaseConnection,
    owner: &str,
    id: Option<&str>,
    form: SaveRuleForm,
) -> Response {
    let rule: ReplacementRule = match serde_json::from_str(&form.rule_json) {
        Ok(rule) => rule,
        Err(_) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Invalid rule",
                "Rule data is invalid.",
                "/workspace",
            );
        }
    };
    match saved_rules::save(database, owner, id, rule).await {
        Ok(saved) => Json(saved).into_response(),
        Err(message) => error_page(
            if message == "Saved rule not found." {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            },
            "Could not save rule",
            &message,
            "/workspace",
        ),
    }
}

pub async fn web_delete_saved_rule(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<DeleteRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    delete_saved_rule_response(&state.database, user.profile().central_id(), &id).await
}

pub async fn owner_delete_saved_rule(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<DeleteRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    delete_saved_rule_response(&state.database, &state.central_id, &id).await
}

async fn delete_saved_rule_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
) -> Response {
    match saved_rules::delete(database, owner, id).await {
        Ok(true) => Json(serde_json::json!({"ok": true})).into_response(),
        Ok(false) => error_page(
            StatusCode::NOT_FOUND,
            "Saved rule not found",
            "Saved rule not found.",
            "/workspace",
        ),
        Err(message) => error_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not delete rule",
            &message,
            "/workspace",
        ),
    }
}

pub async fn web_oauth_index(
    State(state): State<WebState>,
    user: Option<AxumUser>,
    Query(query): Query<SessionQuery>,
) -> Response {
    let owner = user
        .as_ref()
        .map(|user| user.profile().central_id().to_owned());
    let authentication = user.map(|user| Authentication::WebOAuth(Box::new(user)));
    render_index(
        authentication.as_ref(),
        owner.as_deref(),
        &state.database,
        &state.user_agent,
        query.include_groups,
        query.wiki_id.as_deref(),
    )
    .await
}

pub async fn owner_index(
    State(state): State<OwnerState>,
    Query(query): Query<SessionQuery>,
) -> Response {
    let authentication = Authentication::OwnerToken {
        username: state.username.clone(),
        access_token: state.access_token.clone(),
    };
    render_index(
        Some(&authentication),
        Some(&state.central_id),
        &state.database,
        &state.user_agent,
        query.include_groups,
        query.wiki_id.as_deref(),
    )
    .await
}

pub async fn web_wikis(
    State(state): State<WebState>,
    _user: AxumUser,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    (jar, wikis_response(&state.database, token).await)
}

pub async fn owner_wikis(
    State(state): State<OwnerState>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    (jar, wikis_response(&state.database, token).await)
}

async fn wikis_response(database: &DatabaseConnection, csrf_token: String) -> Response {
    match wikis::list(database).await {
        Ok(items) => render(WikisResponse {
            wikis: items.into_iter().map(WikiView::from).collect(),
            csrf_token,
        }),
        Err(error) => internal_error(error),
    }
}

pub async fn web_add_wiki(
    State(state): State<WebState>,
    _user: AxumUser,
    jar: PrivateCookieJar,
    Form(form): Form<AddWikiForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/add-wiki");
    }
    add_wiki_response(&state.database, &state.user_agent, &form.domain).await
}

pub async fn owner_add_wiki(
    State(state): State<OwnerState>,
    jar: PrivateCookieJar,
    Form(form): Form<AddWikiForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/add-wiki");
    }
    add_wiki_response(&state.database, &state.user_agent, &form.domain).await
}

async fn add_wiki_response(
    database: &DatabaseConnection,
    user_agent: &str,
    domain: &str,
) -> Response {
    match wikis::register(database, user_agent, domain).await {
        Ok(wiki) => action_response("/workspace".into(), Some(wiki.id)),
        Err(message) => error_page(
            StatusCode::BAD_REQUEST,
            "Could not add wiki",
            &message,
            "/add-wiki",
        ),
    }
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
    let response = project_response(
        &state.database,
        user.profile().central_id(),
        user.profile().username(),
        &state.user_agent,
        &id,
        &token,
    )
    .await;
    (jar, response)
}

pub async fn owner_project(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response = project_response(
        &state.database,
        &state.central_id,
        &state.username,
        &state.user_agent,
        &id,
        &token,
    )
    .await;
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

pub async fn web_build_project(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    if let Err(response) =
        save_project(&state.database, user.profile().central_id(), &id, &form).await
    {
        return *response;
    }
    start_job_response(
        &state.database,
        user.profile().central_id(),
        user.profile().username(),
        user.oauth2_token(),
        &state.user_agent,
        &id,
        true,
    )
    .await
}

pub async fn owner_build_project(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    if let Err(response) = save_project(&state.database, &state.central_id, &id, &form).await {
        return *response;
    }
    start_job_response(
        &state.database,
        &state.central_id,
        &state.username,
        &state.access_token,
        &state.user_agent,
        &id,
        true,
    )
    .await
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

pub async fn web_start_job(
    State(state): State<WebState>,
    user: AxumUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<StartJobForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    start_job_response(
        &state.database,
        user.profile().central_id(),
        user.profile().username(),
        user.oauth2_token(),
        &state.user_agent,
        &id,
        false,
    )
    .await
}

pub async fn owner_start_job(
    State(state): State<OwnerState>,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<StartJobForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    start_job_response(
        &state.database,
        &state.central_id,
        &state.username,
        &state.access_token,
        &state.user_agent,
        &id,
        false,
    )
    .await
}

pub async fn web_job_review(
    State(state): State<WebState>,
    user: AxumUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response = review_response(
        &state.database,
        user.profile().central_id(),
        user.oauth2_token(),
        &state.user_agent,
        (&id, &job_id),
        &token,
        None,
    )
    .await;
    (jar, response)
}

pub async fn owner_job_review(
    State(state): State<OwnerState>,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    let response = review_response(
        &state.database,
        &state.central_id,
        &state.access_token,
        &state.user_agent,
        (&id, &job_id),
        &token,
        None,
    )
    .await;
    (jar, response)
}

pub async fn web_job_recompute(
    State(state): State<WebState>,
    user: AxumUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<RecomputeForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    review_response(
        &state.database,
        user.profile().central_id(),
        user.oauth2_token(),
        &state.user_agent,
        (&id, &job_id),
        &form.csrf_token,
        Some(form.proposed_text),
    )
    .await
}

pub async fn owner_job_recompute(
    State(state): State<OwnerState>,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<RecomputeForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    review_response(
        &state.database,
        &state.central_id,
        &state.access_token,
        &state.user_agent,
        (&id, &job_id),
        &form.csrf_token,
        Some(form.proposed_text),
    )
    .await
}

pub async fn web_job_action(
    State(state): State<WebState>,
    user: AxumUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<JobActionForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    job_action_response(
        &state.database,
        user.profile().central_id(),
        user.profile().username(),
        user.oauth2_token(),
        &state.user_agent,
        &id,
        &job_id,
        form,
    )
    .await
}

pub async fn owner_job_action(
    State(state): State<OwnerState>,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<JobActionForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    job_action_response(
        &state.database,
        &state.central_id,
        &state.username,
        &state.access_token,
        &state.user_agent,
        &id,
        &job_id,
        form,
    )
    .await
}

pub async fn web_job_queue_action(
    State(state): State<WebState>,
    user: AxumUser,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<QueueActionForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    queue_action_response(
        &state.database,
        user.profile().central_id(),
        user.oauth2_token(),
        &state.user_agent,
        &id,
        &job_id,
        form,
    )
    .await
}

pub async fn owner_job_queue_action(
    State(state): State<OwnerState>,
    Path((id, job_id)): Path<(String, String)>,
    jar: PrivateCookieJar,
    Form(form): Form<QueueActionForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}/jobs/{job_id}"));
    }
    queue_action_response(
        &state.database,
        &state.central_id,
        &state.access_token,
        &state.user_agent,
        &id,
        &job_id,
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
    let wikis = match wikis::list(database).await {
        Ok(items) => items,
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
        Ok(project) => action_response(format!("/projects/{}", project.id), Some(project.id)),
        Err(error) => project_error_response(error, "/workspace"),
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
                        latest_queue_preview,
                    ) = match latest_job {
                        Some(job) => {
                            let job_state: JobState = match serde_json::from_value(job.state_json) {
                                Ok(state) => state,
                                Err(error) => return internal_error(error),
                            };
                            let preview = job_state.queue.iter().take(10).cloned().collect();
                            (
                                job.id,
                                job_state.status.label().to_owned(),
                                job_state.generation_message,
                                job_state.queue.len(),
                                job_state.generation_found,
                                job_state.generation_duplicates,
                                job_state.generation_limit_reached,
                                job_state.status.is_building(),
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
                            Vec::new(),
                        ),
                    };
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
                        latest_queue_preview: &latest_queue_preview,
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
            return render_page_preview_error(database, id, &form, &[], &message).await;
        }
    };
    let wiki_url = match resolve_project_wiki_url(database, id, owner, &form.wiki_id).await {
        Ok(Some(url)) => url,
        Ok(None) => {
            return render_page_preview_error(
                database,
                id,
                &form,
                &rules,
                "Select one of the available wikis.",
            )
            .await;
        }
        Err(error) => return internal_error(error),
    };
    let category_prefix = category_prefix_for_wiki(database, &wiki_url).await;
    let snapshot = match load_page(&wiki_url, user_agent, access_token, &form.page_title).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return render_page_preview_error(database, id, &form, &rules, &error.to_string())
                .await;
        }
    };
    let result = match apply_with_wiki(
        &rules,
        &snapshot.content,
        &wiki_url,
        user_agent,
        &category_prefix,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            return render_page_preview_error(database, id, &form, &rules, &error).await;
        }
    };
    let page_preview = page_preview_view(snapshot, result);
    let sources = page_sources_from_form(&form).unwrap_or_default();
    let source_views = page_source_views(&sources);
    let selected_namespaces = parse_namespaces(&form.namespaces).unwrap_or_default();
    let namespace_options =
        namespace_options_for_form(database, &form.wiki_id, &selected_namespaces).await;
    render_project(ProjectResponse {
        project_id: id,
        project_name: &form.name,
        state_version: form.state_version,
        sources: &source_views,
        namespace_options: &namespace_options,
        queue_limit: form.queue_limit,
        wiki_id: &form.wiki_id,
        awb_access: None,
        rules: &rules,
        csrf_token: &form.csrf_token,
        page_title: &form.page_title,
        page_preview: Some(&page_preview),
        page_preview_error: None,
        latest_job_id: "",
        latest_job_status: "",
        latest_job_message: "",
        latest_job_total: 0,
        latest_job_found: 0,
        latest_job_duplicates: 0,
        latest_job_limit_reached: false,
        latest_job_building: false,
        latest_queue_preview: &[],
    })
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
                "Project not found",
                "This project does not exist or you do not have access to it.",
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
            "Invalid replacement rules",
            &error.to_string(),
            &back_href,
        );
    }
    if let Err(message) = validate_project_sources(&state) {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Cannot start review",
            &message,
            &back_href,
        );
    }
    if !state.has_sources() {
        return error_page(
            StatusCode::BAD_REQUEST,
            "Cannot start review",
            "Add at least one page source.",
            &back_href,
        );
    }
    let queue_limit = match validate_queue_limit(state.queue_limit) {
        Ok(limit) => limit,
        Err(message) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Cannot start review",
                &message,
                &back_href,
            );
        }
    };
    let config = JobConfigV1::new(state.rules.clone());
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
            tokio::spawn(async move {
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
            });
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
                &format!("Could not initialize the MediaWiki page generator: {error}"),
            )
            .await;
            return;
        }
    };
    for source in state.ordered_sources() {
        if source.kind == PageSourceKind::Manual {
            if update_generation(
                &database,
                &owner,
                &project_id,
                &job_id,
                Vec::new(),
                "Loading manual titles…",
                false,
                false,
            )
            .await
            .ok()
            .flatten()
            .is_none()
            {
                return;
            }
            match normalize_titles(&wiki_url, &user_agent, &access_token, &source.values).await {
                Ok(mut pages) => {
                    filter_namespaces(&mut pages, &state.namespaces);
                    let message = format!("Loaded {} manual title(s).", pages.len());
                    if update_generation(
                        &database,
                        &owner,
                        &project_id,
                        &job_id,
                        queue_from_normalized(pages),
                        &message,
                        false,
                        false,
                    )
                    .await
                    .ok()
                    .flatten()
                    .is_none()
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
                        &format!("Manual title lookup failed: {error}"),
                    )
                    .await;
                    return;
                }
            }
            continue;
        }

        let (kind, label) = match source.kind {
            PageSourceKind::Category => (Some(PageListKind::CategoryMembers), "category members"),
            PageSourceKind::Backlinks => (Some(PageListKind::Backlinks), "backlinks"),
            PageSourceKind::Transclusions => (Some(PageListKind::Transclusions), "transclusions"),
            PageSourceKind::Search => (Some(PageListKind::Search), "search results"),
            PageSourceKind::Watchlist => (None, "watchlist pages"),
            PageSourceKind::Contributions => (None, "contribution pages"),
            PageSourceKind::LinksOnPage => (None, "linked pages"),
            PageSourceKind::CategoriesOnPage => (None, "categories on page"),
            PageSourceKind::RedirectsToPage => (None, "redirect pages"),
            PageSourceKind::Manual => unreachable!(),
        };
        for value in &source.values {
            let current =
                match current_generation_size(&database, &owner, &project_id, &job_id).await {
                    Some(size) => size,
                    None => return,
                };
            if current >= state.queue_limit {
                break;
            }
            let loading_message = format!("Loading {label} for “{value}”…");
            if update_generation(
                &database,
                &owner,
                &project_id,
                &job_id,
                Vec::new(),
                &loading_message,
                false,
                false,
            )
            .await
            .ok()
            .flatten()
            .is_none()
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
                    let message = format!("Loaded {label} for “{value}”.");
                    if update_generation(
                        &database,
                        &owner,
                        &project_id,
                        &job_id,
                        queue_from_normalized(pages),
                        &message,
                        false,
                        false,
                    )
                    .await
                    .ok()
                    .flatten()
                    .is_none()
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
                        &format!("Could not load {label}: {error}"),
                    )
                    .await;
                    return;
                }
            }
        }
    }
    let _ = update_generation(
        &database,
        &owner,
        &project_id,
        &job_id,
        Vec::new(),
        "Queue ready.",
        true,
        false,
    )
    .await;
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
) -> Option<usize> {
    let job = JobRepository::new(database)
        .get(owner, project_id, job_id)
        .await
        .ok()??;
    let state: JobState = serde_json::from_value(job.state_json).ok()?;
    (state.status == JobStatus::Building).then_some(state.queue.len())
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
    let _ = update_generation(
        database,
        owner,
        project_id,
        job_id,
        Vec::new(),
        message,
        false,
        true,
    )
    .await;
}

async fn review_response(
    database: &DatabaseConnection,
    owner: &str,
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
    let current = (state.status.is_active() && !state.preparse_active)
        .then(|| state.current())
        .flatten();
    let mut page = None;
    let mut page_error = None;
    if let Some(entry) = current.filter(|entry| {
        entry.status != QueueEntryStatus::Missing && entry.status != QueueEntryStatus::Invalid
    }) {
        match review_page_view(
            database,
            &job.wiki_url,
            user_agent,
            access_token,
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
        skipped: counters.skipped,
        unavailable: counters.unavailable,
        rule_count: config.rules.len(),
        generation_message: &state.generation_message,
        preparse_active: state.preparse_active,
        preparse_cursor: state.preparse_cursor,
        preparse_render_equivalent: state.preparse_render_equivalent,
        queue: &state.queue,
        current,
        page: page.as_ref(),
        page_error: page_error.as_deref(),
    })
}

async fn review_page_view(
    database: &DatabaseConnection,
    wiki_url: &str,
    user_agent: &str,
    access_token: &str,
    entry: &QueueEntry,
    config: &JobConfigV1,
    proposed_text: Option<String>,
) -> Result<ReviewPageView, String> {
    let snapshot = load_page(wiki_url, user_agent, access_token, &entry.title)
        .await
        .map_err(|error| error.to_string())?;
    let category_prefix = category_prefix_for_wiki(database, wiki_url).await;
    let result = apply_with_wiki(
        &config.rules,
        &snapshot.content,
        wiki_url,
        user_agent,
        &category_prefix,
    )
    .await?;
    let proposed_text = proposed_text.unwrap_or_else(|| result.text.clone());
    if proposed_text.len() > crate::rules::MAX_OUTPUT_BYTES {
        return Err("The proposed wikitext is too large.".to_owned());
    }
    let protection = protection_text(&snapshot);
    let unified_diff = unified_lines(&snapshot.content, &proposed_text);
    let side_by_side_diff = side_by_side_lines(&snapshot.content, &proposed_text);
    Ok(ReviewPageView {
        full_url: snapshot.full_url,
        revision_id: snapshot.revision_id,
        revision_timestamp: snapshot.revision_timestamp,
        server_timestamp: snapshot.server_timestamp,
        content_model: snapshot.content_model,
        editable: snapshot.editable,
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
        unified_diff,
        side_by_side_diff,
    })
}

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
    // Review actions remain read-only, but the server refreshes AWB eligibility
    // even when a client skips the project-page status request.
    let _access = wikis::access_unregistered(&job.wiki_url, user_agent, username).await;
    let config: JobConfigV1 = match serde_json::from_value(job.config_json) {
        Ok(config) => config,
        Err(error) => return internal_error(error),
    };
    let mut state: JobState = match serde_json::from_value(job.state_json) {
        Ok(state) => state,
        Err(error) => return internal_error(error),
    };
    if state.preparse_active
        && !matches!(
            form.action.as_str(),
            "preparse_step" | "preparse_stop" | "stop"
        )
    {
        return error_page(
            StatusCode::CONFLICT,
            "Pre-parse is running",
            "Stop pre-parse before reviewing pages.",
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
            state.preparse_render_equivalent = form.render_equivalent;
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
                match load_page(&job.wiki_url, user_agent, access_token, &title).await {
                    Ok(snapshot) => {
                        let prefix = category_prefix_for_wiki(database, &job.wiki_url).await;
                        match apply_with_wiki(
                            &config.rules,
                            &snapshot.content,
                            &job.wiki_url,
                            user_agent,
                            &prefix,
                        )
                        .await
                        {
                            Ok(result) => {
                                if let Some(entry) = state.queue.get_mut(state.preparse_cursor) {
                                    entry.revision_id = Some(snapshot.revision_id);
                                    entry.revision_timestamp = Some(snapshot.revision_timestamp);
                                    entry.replacement_count = Some(result.total_replacements);
                                    if result.text == snapshot.content {
                                        entry.status = QueueEntryStatus::Skipped;
                                        entry.reason =
                                            Some("No changes after rules (pre-parse).".to_owned());
                                    }
                                }
                                if result.text != snapshot.content
                                    && state.preparse_render_equivalent
                                {
                                    let before = parse_preview(
                                        &job.wiki_url,
                                        user_agent,
                                        access_token,
                                        &snapshot.title,
                                        &snapshot.content,
                                    )
                                    .await;
                                    let after = parse_preview(
                                        &job.wiki_url,
                                        user_agent,
                                        access_token,
                                        &snapshot.title,
                                        &result.text,
                                    )
                                    .await;
                                    if let (Ok(before), Ok(after)) = (before, after)
                                        && before.html == after.html
                                        && before.module_styles == after.module_styles
                                        && let Some(entry) =
                                            state.queue.get_mut(state.preparse_cursor)
                                    {
                                        entry.status = QueueEntryStatus::Skipped;
                                        entry.reason = Some(
                                            "Rendered output is unchanged (pre-parse).".to_owned(),
                                        );
                                    }
                                }
                            }
                            Err(error) => {
                                if let Some(entry) = state.queue.get_mut(state.preparse_cursor) {
                                    entry.reason =
                                        Some(format!("Pre-parse transformation failed: {error}"));
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(entry) = state.queue.get_mut(state.preparse_cursor) {
                            entry.reason = Some(format!("Pre-parse could not load page: {error}"));
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
                state.generation_message = "Queue generation cancelled.".to_owned();
            }
        }
        "skip" => state.advance(
            QueueEntryStatus::Skipped,
            Some("Skipped during manual review.".to_owned()),
        ),
        "next" => {
            if let Some(entry) = state.current().filter(|entry| entry.status.is_pending()) {
                let title = entry.title.clone();
                let loaded = load_page(&job.wiki_url, user_agent, access_token, &title).await;
                match loaded {
                    Ok(snapshot) => match apply_with_wiki(
                        &config.rules,
                        &snapshot.content,
                        &job.wiki_url,
                        user_agent,
                        &category_prefix_for_wiki(database, &job.wiki_url).await,
                    )
                    .await
                    {
                        Ok(result) => {
                            if let Some(entry) = state.queue.get_mut(state.position) {
                                entry.revision_id = Some(snapshot.revision_id);
                                entry.revision_timestamp = Some(snapshot.revision_timestamp);
                                entry.replacement_count = Some(result.total_replacements);
                            }
                            state.advance(QueueEntryStatus::Reviewed, None);
                        }
                        Err(error) => state.advance(
                            QueueEntryStatus::Skipped,
                            Some(format!("Transformation failed: {error}")),
                        ),
                    },
                    Err(error) => state.advance(
                        QueueEntryStatus::Skipped,
                        Some(format!("Page load failed: {error}")),
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
            "Pre-parse is running",
            "Stop pre-parse before changing the queue.",
            &back_href,
        );
    }
    match form.action.as_str() {
        "move_up" => state.move_entry(form.index, -1),
        "move_down" => state.move_entry(form.index, 1),
        "remove" => state.remove_entry(form.index),
        "clear" => state.clear(),
        "sort_remaining" => state.sort_remaining_by_title(),
        "append" => {
            let titles = match source_values(&split_lines(&form.titles), "Appended pages") {
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
                        "Cannot append pages",
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
                            "Could not append pages",
                            &format!("MediaWiki request failed: {error}"),
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
                "Choose append, move, remove, sort, or clear.",
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

fn job_not_found(project_id: &str) -> Response {
    error_page(
        StatusCode::NOT_FOUND,
        "Review not found",
        "This review does not exist or you do not have access to it.",
        &format!("/projects/{project_id}"),
    )
}

fn job_error_response(error: JobError, back_href: &str) -> Response {
    match error {
        JobError::Conflict => error_page(
            StatusCode::CONFLICT,
            "Review changed elsewhere",
            "Reload the review to get the latest progress, then try again.",
            back_href,
        ),
        JobError::Database(error) => internal_error(error),
        JobError::Serialization(error) => internal_error(error),
    }
}

async fn render_page_preview_error(
    database: &DatabaseConnection,
    id: &str,
    form: &UpdateProjectForm,
    rules: &[ReplacementRule],
    message: &str,
) -> Response {
    let sources = page_sources_from_form(form).unwrap_or_default();
    let source_views = page_source_views(&sources);
    let selected_namespaces = parse_namespaces(&form.namespaces).unwrap_or_default();
    let namespace_options =
        namespace_options_for_form(database, &form.wiki_id, &selected_namespaces).await;
    render_project(ProjectResponse {
        project_id: id,
        project_name: &form.name,
        state_version: form.state_version,
        sources: &source_views,
        namespace_options: &namespace_options,
        queue_limit: form.queue_limit,
        wiki_id: &form.wiki_id,
        awb_access: None,
        rules,
        csrf_token: &form.csrf_token,
        page_title: &form.page_title,
        page_preview: None,
        page_preview_error: Some(message),
        latest_job_id: "",
        latest_job_status: "",
        latest_job_message: "",
        latest_job_total: 0,
        latest_job_found: 0,
        latest_job_duplicates: 0,
        latest_job_limit_reached: false,
        latest_job_building: false,
        latest_queue_preview: &[],
    })
}

fn page_preview_view(snapshot: PageSnapshot, result: TransformationResult) -> PagePreviewView {
    let protection = protection_text(&snapshot);
    let proposed_text = result.text.clone();
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
        proposed_text,
        rules: result
            .counts
            .into_iter()
            .zip(result.changed)
            .map(|(count, changed)| RulePreviewView { count, changed })
            .collect(),
        unified_diff: unified_lines(&snapshot.content, &result.text),
        side_by_side_diff: side_by_side_lines(&snapshot.content, &result.text),
    }
}

fn protection_text(snapshot: &PageSnapshot) -> String {
    if snapshot.protection.is_empty() {
        "None".to_owned()
    } else {
        snapshot
            .protection
            .iter()
            .map(|item| format!("{}: {} until {}", item.kind, item.level, item.expiry))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn render_project(response: ProjectResponse<'_>) -> Response {
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
        && state.preparse_active
    {
        return error_page(
            StatusCode::CONFLICT,
            "Pre-parse is running",
            "Stop pre-parse before changing the rules.",
            &format!("/projects/{id}"),
        );
    }
    match save_project(database, owner, id, &form).await {
        Ok((project, rules)) => {
            let jobs = JobRepository::new(database);
            let latest_job = match jobs.latest(owner, id).await {
                Ok(job) => job,
                Err(error) => return job_error_response(error, &format!("/projects/{id}")),
            };
            if let Some(job) = latest_job
                && let Err(error) = jobs
                    .update_config(owner, id, &job.id, &JobConfigV1::new(rules))
                    .await
            {
                return job_error_response(error, &format!("/projects/{id}"));
            }
            action_response(format!("/projects/{id}"), Some(project.id))
        }
        Err(response) => *response,
    }
}

async fn save_project(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
    form: &UpdateProjectForm,
) -> Result<(project::Model, Vec<ReplacementRule>), Box<Response>> {
    let back_href = format!("/projects/{id}");
    let rules = match replacement_rules(form) {
        Ok(rules) => rules,
        Err(message) => {
            return Err(Box::new(error_page(
                StatusCode::BAD_REQUEST,
                "Invalid replacement rules",
                &message,
                &back_href,
            )));
        }
    };
    if let Err(error) = RuleEngine::compile(&rules) {
        return Err(Box::new(error_page(
            StatusCode::BAD_REQUEST,
            "Invalid replacement rules",
            &error.to_string(),
            &back_href,
        )));
    }
    let wiki_url = match resolve_project_wiki_url(database, id, owner, &form.wiki_id).await {
        Ok(Some(url)) => url,
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
                "Invalid queue limit",
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
                    "Invalid namespaces",
                    &message,
                    &back_href,
                )));
            }
        };
    if let Ok(Some(wiki)) = wikis::by_id(database, &form.wiki_id).await {
        if let Ok(available) = wikis::namespaces(&wiki) {
            if namespaces
                .iter()
                .any(|id| !available.iter().any(|entry| entry.id == *id))
            {
                return Err(Box::new(error_page(
                    StatusCode::BAD_REQUEST,
                    "Invalid namespaces",
                    "Select namespaces available on this wiki.",
                    &back_href,
                )));
            }
        }
    }
    let sources = match page_sources_from_form(form) {
        Ok(sources) => sources,
        Err(message) => {
            return Err(Box::new(error_page(
                StatusCode::BAD_REQUEST,
                "Invalid page sources",
                &message,
                &back_href,
            )));
        }
    };
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
    };
    if let Err(message) = validate_project_sources(&state) {
        return Err(Box::new(error_page(
            StatusCode::BAD_REQUEST,
            "Invalid page sources",
            &message,
            &back_href,
        )));
    }
    let saved_rules = state.rules.clone();
    let repository = ProjectRepository::new(database);
    match repository
        .update(
            owner,
            id,
            form.name.clone(),
            wiki_url,
            state,
            form.state_version,
        )
        .await
    {
        Ok(Some(project)) => Ok((project, saved_rules)),
        Ok(None) => Err(Box::new(error_page(
            StatusCode::NOT_FOUND,
            "Project not found",
            "This project does not exist or you do not have access to it.",
            "/workspace",
        ))),
        Err(error) => Err(Box::new(project_error_response(error, &back_href))),
    }
}

fn split_lines(value: &str) -> Vec<String> {
    value
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn page_sources_from_form(form: &UpdateProjectForm) -> Result<Vec<PageSource>, String> {
    if form.source_kind.len() != form.source_value.len() {
        return Err("The submitted page sources are incomplete. Reload and try again.".to_owned());
    }
    if form.source_kind.len() > 100 {
        return Err("A project can contain at most 100 page sources.".to_owned());
    }

    form.source_kind
        .iter()
        .zip(&form.source_value)
        .filter_map(|(kind, value)| {
            let values = split_lines(value);
            if values.is_empty() {
                return None;
            }
            Some(parse_page_source(kind, values))
        })
        .collect()
}

fn parse_page_source(kind: &str, mut values: Vec<String>) -> Result<PageSource, String> {
    let kind = match kind {
        "manual" => PageSourceKind::Manual,
        "category" => PageSourceKind::Category,
        "backlinks" => PageSourceKind::Backlinks,
        "transclusions" => PageSourceKind::Transclusions,
        "search" => PageSourceKind::Search,
        "watchlist" => PageSourceKind::Watchlist,
        "contributions" => PageSourceKind::Contributions,
        "links_on_page" => PageSourceKind::LinksOnPage,
        "categories_on_page" => PageSourceKind::CategoriesOnPage,
        "redirects_to_page" => PageSourceKind::RedirectsToPage,
        _ => return Err("A page source has an unsupported type.".to_owned()),
    };
    if kind == PageSourceKind::Watchlist {
        values = vec!["My watchlist".to_owned()];
    }
    if matches!(
        kind,
        PageSourceKind::Category | PageSourceKind::Transclusions
    ) {
        let prefix = if kind == PageSourceKind::Category {
            "Category:"
        } else {
            "Template:"
        };
        for value in &mut values {
            if !value.contains(':') {
                *value = format!("{prefix}{value}");
            }
        }
    }
    source_values(&values, kind.label())?;
    Ok(PageSource { kind, values })
}

fn legacy_sources(sources: &[PageSource]) -> LegacySourceLists {
    let (mut pages, mut categories, mut backlinks, mut transclusions, mut searches) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for source in sources {
        match source.kind {
            PageSourceKind::Manual => pages.extend(source.values.clone()),
            PageSourceKind::Category => categories.extend(source.values.clone()),
            PageSourceKind::Backlinks => backlinks.extend(source.values.clone()),
            PageSourceKind::Transclusions => transclusions.extend(source.values.clone()),
            PageSourceKind::Search => searches.extend(source.values.clone()),
            PageSourceKind::Watchlist
            | PageSourceKind::Contributions
            | PageSourceKind::LinksOnPage
            | PageSourceKind::CategoriesOnPage
            | PageSourceKind::RedirectsToPage => {}
        }
    }
    LegacySourceLists {
        pages,
        categories,
        backlinks,
        transclusions,
        searches,
    }
}

fn page_source_views(sources: &[PageSource]) -> Vec<PageSourceView> {
    sources
        .iter()
        .map(|source| {
            let first = source
                .values
                .first()
                .map(String::as_str)
                .unwrap_or_default();
            let summary = if source.values.len() > 1 {
                format!("{first} and {} more", source.values.len() - 1)
            } else {
                first.to_owned()
            };
            PageSourceView {
                kind: source.kind.form_value(),
                label: source.kind.label(),
                value: source.values.join("\n"),
                summary,
            }
        })
        .collect()
}

fn namespace_options(available: &[wikis::Namespace], selected: &[i64]) -> Vec<NamespaceOption> {
    available
        .iter()
        .map(|namespace| NamespaceOption {
            id: namespace.id,
            label: namespace.name.clone(),
            selected: selected.contains(&namespace.id),
        })
        .collect()
}

fn legacy_namespaces() -> Vec<wikis::Namespace> {
    [
        (0, "Main"),
        (1, "Talk"),
        (2, "User"),
        (3, "User talk"),
        (4, "Project"),
        (5, "Project talk"),
        (6, "File"),
        (7, "File talk"),
        (8, "MediaWiki"),
        (9, "MediaWiki talk"),
        (10, "Template"),
        (11, "Template talk"),
        (12, "Help"),
        (13, "Help talk"),
        (14, "Category"),
        (15, "Category talk"),
        (100, "Portal"),
        (101, "Portal talk"),
        (828, "Module"),
        (829, "Module talk"),
    ]
    .into_iter()
    .map(|(id, name)| wikis::Namespace {
        id,
        name: name.into(),
    })
    .collect()
}

async fn namespace_options_for_form(
    database: &DatabaseConnection,
    wiki_id: &str,
    selected: &[i64],
) -> Vec<NamespaceOption> {
    let available = match wikis::by_id(database, wiki_id).await {
        Ok(Some(wiki)) => wikis::namespaces(&wiki).unwrap_or_else(|_| legacy_namespaces()),
        _ => legacy_namespaces(),
    };
    namespace_options(&available, selected)
}

async fn category_prefix_for_wiki(database: &DatabaseConnection, wiki_url: &str) -> String {
    if let Ok(Some(wiki)) = wikis::by_url(database, wiki_url).await
        && let Ok(namespaces) = wikis::namespaces(&wiki)
        && let Some(category) = namespaces.into_iter().find(|namespace| namespace.id == 14)
        && !category.name.trim().is_empty()
    {
        return category.name;
    }
    "Category".to_owned()
}

async fn resolve_project_wiki_url(
    database: &DatabaseConnection,
    project_id: &str,
    owner: &str,
    wiki_id: &str,
) -> Result<Option<String>, sea_orm::DbErr> {
    if let Some(wiki) = wikis::by_id(database, wiki_id).await? {
        return Ok(Some(wiki.api_url));
    }
    if let Ok(Some(project)) = ProjectRepository::new(database)
        .get(owner, project_id)
        .await
    {
        if wiki_id == project.wiki_url {
            return Ok(Some(project.wiki_url));
        }
    }
    Ok(None)
}

fn validate_project_sources(state: &ProjectState) -> Result<(), String> {
    for source in state.ordered_sources() {
        source_values(&source.values, source.kind.label())?;
    }
    Ok(())
}

fn parse_namespaces(values: &[String]) -> Result<Vec<i64>, String> {
    values
        .iter()
        .map(String::as_str)
        .map(|item| {
            item.parse::<i64>()
                .map_err(|_| format!("“{item}” is not a namespace ID."))
        })
        .collect()
}

fn replacement_rules(form: &UpdateProjectForm) -> Result<Vec<ReplacementRule>, String> {
    let count = form.rule_find.len();
    if form.rule_replace.len() != count
        || form.rule_type.len() != count
        || form.rule_case.len() != count
        || form.rule_scope.len() != count
        || (!form.rule_operation.is_empty() && form.rule_operation.len() != count)
        || (!form.rule_group.is_empty() && form.rule_group.len() != count)
        || (!form.rule_group_contains.is_empty() && form.rule_group_contains.len() != count)
        || (!form.rule_parameter.is_empty() && form.rule_parameter.len() != count)
        || (!form.rule_name.is_empty() && form.rule_name.len() != count)
    {
        return Err(
            "The submitted rule fields are incomplete. Reload the page and try again.".to_owned(),
        );
    }

    let enabled = rule_indexes(&form.rule_enabled, count)?;
    let multiline = rule_indexes(&form.rule_multiline, count)?;
    let dot_newline = rule_indexes(&form.rule_dot_newline, count)?;
    let group_invert = rule_indexes(&form.rule_group_invert, count)?;
    let mask_comments = rule_indexes(&form.rule_mask_comments, count)?;
    let mask_tags = rule_indexes(&form.rule_mask_tags, count)?;
    let mask_references = rule_indexes(&form.rule_mask_references, count)?;
    let mask_links = rule_indexes(&form.rule_mask_links, count)?;
    let mask_headings = rule_indexes(&form.rule_mask_headings, count)?;
    let mask_templates = rule_indexes(&form.rule_mask_templates, count)?;
    let mask_link_targets = rule_indexes(&form.rule_mask_link_targets, count)?;
    let mut rules = Vec::with_capacity(count);
    for index in 0..count {
        let name = form.rule_name.get(index).map(String::as_str).unwrap_or("");
        if name.chars().count() > 255 {
            return Err(format!("Rule {} name is too long.", index + 1));
        }
        if form.rule_find[index].is_empty() && form.rule_replace[index].is_empty() {
            continue;
        }
        let operation = match form
            .rule_operation
            .get(index)
            .map(String::as_str)
            .unwrap_or("replace")
        {
            "replace" => RuleOperation::Replace,
            "append" => RuleOperation::Append,
            "prepend" => RuleOperation::Prepend,
            "category_add" => RuleOperation::CategoryAdd,
            "category_remove" => RuleOperation::CategoryRemove,
            "category_replace" => RuleOperation::CategoryReplace,
            "template_rename" => RuleOperation::TemplateRename,
            "template_parameter_set" => RuleOperation::TemplateParameterSet,
            "template_parameter_remove" => RuleOperation::TemplateParameterRemove,
            _ => return Err(format!("Rule {} has an invalid operation.", index + 1)),
        };
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
            name: name.to_owned(),
            enabled: enabled.contains(&index),
            operation,
            group_name: form.rule_group.get(index).cloned().unwrap_or_default(),
            group_contains: form
                .rule_group_contains
                .get(index)
                .cloned()
                .unwrap_or_default(),
            group_invert: group_invert.contains(&index),
            parameter: form.rule_parameter.get(index).cloned().unwrap_or_default(),
            mask: RuleMask {
                comments: mask_comments.contains(&index),
                tags: mask_tags.contains(&index),
                references: mask_references.contains(&index),
                links: mask_links.contains(&index),
                headings: mask_headings.contains(&index),
                templates: mask_templates.contains(&index),
                link_targets: mask_link_targets.contains(&index),
            },
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
        Ok(true) => action_response("/workspace".to_owned(), Some(id.to_owned())),
        Ok(false) => error_page(
            StatusCode::NOT_FOUND,
            "Project not found",
            "This project does not exist or you do not have access to it.",
            "/workspace",
        ),
        Err(error) => project_error_response(error, &back_href),
    }
}

async fn render_index(
    authentication: Option<&Authentication>,
    owner: Option<&str>,
    database: &DatabaseConnection,
    user_agent: &str,
    include_groups: bool,
    requested_wiki_id: Option<&str>,
) -> Response {
    if let Some(authentication) = authentication {
        debug_assert!(!authentication.access_token().is_empty());
    }
    let (groups, groups_error) = match (authentication, include_groups, requested_wiki_id) {
        (Some(authentication), true, Some(requested_wiki_id)) => {
            let wiki = match wikis::by_id(database, requested_wiki_id).await {
                Ok(wiki) => wiki,
                Err(error) => return internal_error(error),
            };
            let wiki_url = if let Some(wiki) = wiki {
                Some(wiki.api_url)
            } else if let Some(owner) = owner {
                match ProjectRepository::new(database).list(owner).await {
                    Ok(projects) => projects
                        .into_iter()
                        .find(|project| project.wiki_url == requested_wiki_id)
                        .map(|project| project.wiki_url),
                    Err(error) => return project_error_response(error, "/workspace"),
                }
            } else {
                None
            };
            let Some(wiki_url) = wiki_url else {
                return render(SessionResponse {
                    username: Some(authentication.username()),
                    groups: Vec::new(),
                    groups_error: Some(format!(
                        "Could not load groups: unsupported wiki {requested_wiki_id}."
                    )),
                    can_logout: authentication.can_logout(),
                });
            };
            match crate::mediawiki::user_groups(
                &wiki_url,
                user_agent,
                authentication.access_token(),
            )
            .await
            {
                Ok(groups) => (groups, None),
                Err(error) => (
                    Vec::new(),
                    Some(format!(
                        "Could not load groups for {requested_wiki_id}: {error}"
                    )),
                ),
            }
        }
        _ => (Vec::new(), None),
    };
    render(SessionResponse {
        username: authentication.map(Authentication::username),
        groups,
        groups_error,
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
        ErrorResponse {
            title,
            message,
            back_href,
        },
    )
}

fn render(template: impl Serialize) -> Response {
    render_with_status(StatusCode::OK, template)
}

fn render_with_status(status: StatusCode, template: impl Serialize) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "same-origin"),
        ],
        Json(template),
    )
        .into_response()
}

fn internal_error(error: impl std::error::Error) -> Response {
    eprintln!("request failed: {error}");
    error_page_without_rendering()
}

fn error_page_without_rendering() -> Response {
    render_with_status(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorResponse {
            title: "Unexpected error",
            message: "An unexpected error occurred.",
            back_href: "/",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_parser_adds_missing_mediawiki_prefixes() {
        let category = parse_page_source("category", vec!["Scientists".to_owned()]).unwrap();
        let template =
            parse_page_source("transclusions", vec!["Infobox person".to_owned()]).unwrap();

        assert_eq!(category.values, ["Category:Scientists"]);
        assert_eq!(template.values, ["Template:Infobox person"]);
    }

    #[test]
    fn source_parser_keeps_localized_prefixes() {
        let category =
            parse_page_source("category", vec!["Категорія:Науковиці".to_owned()]).unwrap();
        assert_eq!(category.values, ["Категорія:Науковиці"]);
    }

    #[test]
    fn submitted_rule_keeps_empty_replacement() {
        let form = UpdateProjectForm {
            name: "Example".to_owned(),
            wiki_id: "examplewiki".to_owned(),
            source_kind: Vec::new(),
            source_value: Vec::new(),
            namespaces: Vec::new(),
            queue_limit: 100,
            rule_name: vec!["Remove obsolete".to_owned()],
            rule_operation: vec!["replace".to_owned()],
            rule_group: Vec::new(),
            rule_group_contains: Vec::new(),
            rule_parameter: Vec::new(),
            rule_mask_comments: Vec::new(),
            rule_mask_tags: Vec::new(),
            rule_mask_references: Vec::new(),
            rule_mask_links: Vec::new(),
            rule_mask_headings: Vec::new(),
            rule_mask_templates: Vec::new(),
            rule_mask_link_targets: Vec::new(),
            rule_group_invert: Vec::new(),
            rule_find: vec!["obsolete".to_owned()],
            rule_replace: vec![String::new()],
            rule_enabled: vec!["0".to_owned()],
            rule_type: vec!["plain".to_owned()],
            rule_case: vec!["sensitive".to_owned()],
            rule_scope: vec!["all".to_owned()],
            rule_multiline: Vec::new(),
            rule_dot_newline: Vec::new(),
            page_title: String::new(),
            state_version: 1,
            csrf_token: "test".to_owned(),
        };

        let rules = replacement_rules(&form).unwrap();
        assert_eq!(rules.len(), 1);
        assert!(rules[0].enabled);
        assert_eq!(rules[0].find, "obsolete");
        assert_eq!(rules[0].name, "Remove obsolete");
        assert_eq!(rules[0].replace, "");
    }
}
