use std::time::Duration;

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
    routing::{any, get, get_service},
};
use mwoauth::axum::{refresh_session, routes as oauth_routes};
use sea_orm::DatabaseConnection;
use tower_http::{
    services::{ServeDir, ServeFile},
    timeout::TimeoutLayer,
};

use crate::{
    AppResult,
    auth::oauth_state,
    config::{AuthenticationConfig, Config},
    mediawiki::owner_identity,
    web::{
        OwnerState, WebState, healthz, owner_add_wiki, owner_build_project, owner_create_project,
        owner_delete_project, owner_delete_saved_rule, owner_index, owner_job_action,
        owner_job_queue_action, owner_job_recompute, owner_job_review, owner_page_preview,
        owner_project, owner_render_preview, owner_save_rule, owner_saved_rules, owner_start_job,
        owner_update_project, owner_update_saved_rule, owner_wikis, owner_workspace, web_add_wiki,
        web_build_project, web_create_project, web_delete_project, web_delete_saved_rule,
        web_job_action, web_job_queue_action, web_job_recompute, web_job_review, web_oauth_index,
        web_page_preview, web_project, web_render_preview, web_save_rule, web_saved_rules,
        web_start_job, web_update_project, web_update_saved_rule, web_wikis, web_workspace,
    },
};

pub async fn router(config: Config, database: DatabaseConnection) -> AppResult<Router> {
    match config.authentication {
        AuthenticationConfig::WebOAuth(oauth_config) => {
            let oauth = oauth_state(
                oauth_config,
                &config.cookie_key,
                &config.auth_wiki_url,
                &config.user_agent,
                config.debug,
            )?;
            let state = WebState {
                oauth,
                database,
                user_agent: config.user_agent,
                csrf_secure: !config.debug,
            };
            let frontend = get_service(ServeFile::new("frontend/dist/index.html"))
                .layer(axum::middleware::from_fn(disable_frontend_cache));
            Ok(oauth_routes()
                .route("/api/session", get(web_oauth_index))
                .route("/api/workspace", get(web_workspace))
                .route("/api/wikis", get(web_wikis).post(web_add_wiki))
                .route("/api/rules", get(web_saved_rules).post(web_save_rule))
                .route(
                    "/api/rules/{id}",
                    axum::routing::post(web_update_saved_rule),
                )
                .route(
                    "/api/rules/{id}/delete",
                    axum::routing::post(web_delete_saved_rule),
                )
                .route("/api/projects", axum::routing::post(web_create_project))
                .route(
                    "/api/projects/{id}/render-preview",
                    axum::routing::post(web_render_preview),
                )
                .route(
                    "/api/projects/{id}",
                    get(web_project).post(web_update_project),
                )
                .route(
                    "/api/projects/{id}/build",
                    axum::routing::post(web_build_project),
                )
                .route(
                    "/api/projects/{id}/page-preview",
                    axum::routing::post(web_page_preview),
                )
                .route(
                    "/api/projects/{id}/delete",
                    axum::routing::post(web_delete_project),
                )
                .route(
                    "/api/projects/{id}/jobs",
                    axum::routing::post(web_start_job),
                )
                .route("/api/projects/{id}/jobs/{job_id}", get(web_job_review))
                .route(
                    "/api/projects/{id}/jobs/{job_id}/action",
                    axum::routing::post(web_job_action),
                )
                .route(
                    "/api/projects/{id}/jobs/{job_id}/recompute",
                    axum::routing::post(web_job_recompute),
                )
                .route(
                    "/api/projects/{id}/jobs/{job_id}/queue",
                    axum::routing::post(web_job_queue_action),
                )
                .route("/healthz", get(healthz))
                .route("/api/{*path}", any(api_not_found))
                .nest_service("/assets", ServeDir::new("frontend/dist/assets"))
                .route_service("/", frontend.clone())
                .route_service("/workspace", frontend.clone())
                .route_service("/add-wiki", frontend.clone())
                .route_service("/projects/{id}", frontend.clone())
                .route_service("/projects/{id}/jobs/{job_id}", frontend)
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
                .layer(axum::middleware::from_fn_with_state(
                    state.oauth.clone(),
                    refresh_session,
                ))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    Duration::from_secs(60),
                ))
                .with_state(state))
        }
        AuthenticationConfig::OwnerToken(access_token) => {
            let identity =
                owner_identity(&config.auth_wiki_url, &config.user_agent, &access_token).await?;
            let state = OwnerState {
                central_id: identity.central_id,
                username: identity.username,
                access_token,
                database,
                user_agent: config.user_agent,
                cookie_key: mwoauth::axum::Key::from(&config.cookie_key),
                csrf_secure: !config.debug,
            };
            let frontend = get_service(ServeFile::new("frontend/dist/index.html"))
                .layer(axum::middleware::from_fn(disable_frontend_cache));
            Ok(Router::new()
                .route("/api/session", get(owner_index))
                .route("/api/workspace", get(owner_workspace))
                .route("/api/wikis", get(owner_wikis).post(owner_add_wiki))
                .route("/api/rules", get(owner_saved_rules).post(owner_save_rule))
                .route(
                    "/api/rules/{id}",
                    axum::routing::post(owner_update_saved_rule),
                )
                .route(
                    "/api/rules/{id}/delete",
                    axum::routing::post(owner_delete_saved_rule),
                )
                .route("/api/projects", axum::routing::post(owner_create_project))
                .route(
                    "/api/projects/{id}/render-preview",
                    axum::routing::post(owner_render_preview),
                )
                .route(
                    "/api/projects/{id}",
                    get(owner_project).post(owner_update_project),
                )
                .route(
                    "/api/projects/{id}/build",
                    axum::routing::post(owner_build_project),
                )
                .route(
                    "/api/projects/{id}/page-preview",
                    axum::routing::post(owner_page_preview),
                )
                .route(
                    "/api/projects/{id}/delete",
                    axum::routing::post(owner_delete_project),
                )
                .route(
                    "/api/projects/{id}/jobs",
                    axum::routing::post(owner_start_job),
                )
                .route("/api/projects/{id}/jobs/{job_id}", get(owner_job_review))
                .route(
                    "/api/projects/{id}/jobs/{job_id}/action",
                    axum::routing::post(owner_job_action),
                )
                .route(
                    "/api/projects/{id}/jobs/{job_id}/recompute",
                    axum::routing::post(owner_job_recompute),
                )
                .route(
                    "/api/projects/{id}/jobs/{job_id}/queue",
                    axum::routing::post(owner_job_queue_action),
                )
                .route("/healthz", get(healthz))
                .route("/api/{*path}", any(api_not_found))
                .nest_service("/assets", ServeDir::new("frontend/dist/assets"))
                .route_service("/", frontend.clone())
                .route_service("/workspace", frontend.clone())
                .route_service("/add-wiki", frontend.clone())
                .route_service("/projects/{id}", frontend.clone())
                .route_service("/projects/{id}/jobs/{job_id}", frontend)
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    Duration::from_secs(60),
                ))
                .with_state(state))
        }
    }
}

async fn disable_frontend_cache(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate, max-age=0"),
    );
    response
}

async fn api_not_found() -> (StatusCode, axum::Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "title": "API endpoint not found",
            "message": "The requested API endpoint does not exist."
        })),
    )
}
