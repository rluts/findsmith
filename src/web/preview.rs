use crate::auth::CurrentUser;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::{
    csrf,
    diff::{DiffLine, SideBySideLine, side_by_side_lines, unified_lines},
    localization,
    mediawiki::{PageSnapshot, load_page, parse_preview},
    projects::{ProjectRepository, ReplacementRule},
    rules::TransformationResult,
    structural::apply_with_wiki,
};

use super::AppState;
use super::project_form::{
    category_prefix_for_wiki, namespace_options_for_form, page_source_views,
    page_sources_from_form, parse_namespaces, replacement_rules, resolve_project_wiki_url,
};
use super::projects::{ProjectResponse, UpdateProjectForm, render_project};
use super::responses::{csrf_error, error_page, internal_error, project_error_response};

#[derive(Serialize)]
pub(super) struct RulePreviewView {
    pub(super) count: usize,
    pub(super) changed: bool,
}

#[derive(Serialize)]
pub(super) struct PagePreviewView {
    pub(super) title: String,
    pub(super) full_url: String,
    pub(super) page_id: u64,
    pub(super) namespace: i64,
    pub(super) revision_id: u64,
    pub(super) revision_timestamp: String,
    pub(super) server_timestamp: String,
    pub(super) content_model: String,
    pub(super) redirect: bool,
    pub(super) editable: bool,
    pub(super) protection: String,
    pub(super) total_replacements: usize,
    pub(super) changed: bool,
    pub(super) proposed_text: String,
    pub(super) rules: Vec<RulePreviewView>,
    pub(super) unified_diff: Vec<DiffLine>,
    pub(super) side_by_side_diff: Vec<SideBySideLine>,
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

pub async fn render_preview(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<RenderPreviewForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    render_preview_response(
        &state.database,
        &user.central_id,
        &user.access_token,
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
                "findsmith-api-project-not-found",
                "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
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
            "findsmith-api-invalid-preview",
            "findsmith-api-the-title-wiki-or-proposed-wikitext-is-invalid",
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
                "findsmith-api-could-not-render-preview",
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

pub async fn page_preview(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<UpdateProjectForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error(&format!("/projects/{id}"));
    }
    page_preview_response(
        &state.database,
        &user.central_id,
        &user.access_token,
        &state.user_agent,
        &id,
        form,
    )
    .await
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
                "findsmith-api-project-not-found",
                "findsmith-api-this-project-does-not-exist-or-you-do-not-have-access-to-it",
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
                "findsmith-api-select-one-of-the-available-wikis",
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
    let skip = form.skip_options();
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
        skip: &skip,
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
        latest_job_outdated: false,
        latest_queue_preview: &[],
    })
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
        skip: &form.skip_options(),
        csrf_token: &form.csrf_token,
        page_title: &form.page_title,
        page_preview: None,
        page_preview_error: Some(localization::from_source(message)),
        latest_job_id: "",
        latest_job_status: "",
        latest_job_message: "",
        latest_job_total: 0,
        latest_job_found: 0,
        latest_job_duplicates: 0,
        latest_job_limit_reached: false,
        latest_job_building: false,
        latest_job_outdated: false,
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

pub(super) fn protection_text(snapshot: &PageSnapshot) -> String {
    if snapshot.protection.is_empty() {
        "None".to_owned()
    } else {
        snapshot
            .protection
            .iter()
            .map(|item| {
                localization::required_with_args(
                    "findsmith-api-protection-until",
                    &[&item.kind, &item.level, &item.expiry],
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}
