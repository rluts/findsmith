use std::time::Duration;

use axum::{Router, extract::DefaultBodyLimit, http::StatusCode, routing::get};
use mwoauth::axum::{refresh_session, routes as oauth_routes};
use sea_orm::DatabaseConnection;
use tower_http::timeout::TimeoutLayer;

use crate::{
    AppResult,
    auth::oauth_state,
    config::{AuthenticationConfig, Config},
    mediawiki::owner_identity,
    web::{
        OwnerState, WebState, healthz, owner_build_project, owner_create_project,
        owner_delete_project, owner_index, owner_job_action, owner_job_queue_action,
        owner_job_recompute, owner_job_review, owner_page_preview, owner_project, owner_start_job,
        owner_update_project, owner_workspace, web_build_project, web_create_project,
        web_delete_project, web_job_action, web_job_queue_action, web_job_recompute,
        web_job_review, web_oauth_index, web_page_preview, web_project, web_start_job,
        web_update_project, web_workspace,
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
            Ok(oauth_routes()
                .route("/", get(web_oauth_index))
                .route("/workspace", get(web_workspace))
                .route("/projects", axum::routing::post(web_create_project))
                .route("/projects/{id}", get(web_project).post(web_update_project))
                .route(
                    "/projects/{id}/build",
                    axum::routing::post(web_build_project),
                )
                .route(
                    "/projects/{id}/page-preview",
                    axum::routing::post(web_page_preview),
                )
                .route(
                    "/projects/{id}/delete",
                    axum::routing::post(web_delete_project),
                )
                .route("/projects/{id}/jobs", axum::routing::post(web_start_job))
                .route("/projects/{id}/jobs/{job_id}", get(web_job_review))
                .route(
                    "/projects/{id}/jobs/{job_id}/action",
                    axum::routing::post(web_job_action),
                )
                .route(
                    "/projects/{id}/jobs/{job_id}/recompute",
                    axum::routing::post(web_job_recompute),
                )
                .route(
                    "/projects/{id}/jobs/{job_id}/queue",
                    axum::routing::post(web_job_queue_action),
                )
                .route("/healthz", get(healthz))
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
            Ok(Router::new()
                .route("/", get(owner_index))
                .route("/workspace", get(owner_workspace))
                .route("/projects", axum::routing::post(owner_create_project))
                .route(
                    "/projects/{id}",
                    get(owner_project).post(owner_update_project),
                )
                .route(
                    "/projects/{id}/build",
                    axum::routing::post(owner_build_project),
                )
                .route(
                    "/projects/{id}/page-preview",
                    axum::routing::post(owner_page_preview),
                )
                .route(
                    "/projects/{id}/delete",
                    axum::routing::post(owner_delete_project),
                )
                .route("/projects/{id}/jobs", axum::routing::post(owner_start_job))
                .route("/projects/{id}/jobs/{job_id}", get(owner_job_review))
                .route(
                    "/projects/{id}/jobs/{job_id}/action",
                    axum::routing::post(owner_job_action),
                )
                .route(
                    "/projects/{id}/jobs/{job_id}/recompute",
                    axum::routing::post(owner_job_recompute),
                )
                .route(
                    "/projects/{id}/jobs/{job_id}/queue",
                    axum::routing::post(owner_job_queue_action),
                )
                .route("/healthz", get(healthz))
                .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::REQUEST_TIMEOUT,
                    Duration::from_secs(60),
                ))
                .with_state(state))
        }
    }
}
