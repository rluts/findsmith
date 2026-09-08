use axum::{Router, routing::get};
use mwoauth::axum::routes as oauth_routes;
use sea_orm::DatabaseConnection;

use crate::{
    AppResult,
    auth::oauth_state,
    config::{AuthenticationConfig, Config},
    mediawiki::owner_identity,
    web::{
        OwnerState, WebState, healthz, owner_create_project, owner_delete_project, owner_index,
        owner_page_preview, owner_preview_project, owner_project, owner_update_project,
        owner_workspace, web_create_project, web_delete_project, web_oauth_index, web_page_preview,
        web_preview_project, web_project, web_update_project, web_workspace,
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
                    "/projects/{id}/preview",
                    axum::routing::post(web_preview_project),
                )
                .route(
                    "/projects/{id}/page-preview",
                    axum::routing::post(web_page_preview),
                )
                .route(
                    "/projects/{id}/delete",
                    axum::routing::post(web_delete_project),
                )
                .route("/healthz", get(healthz))
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
                    "/projects/{id}/preview",
                    axum::routing::post(owner_preview_project),
                )
                .route(
                    "/projects/{id}/page-preview",
                    axum::routing::post(owner_page_preview),
                )
                .route(
                    "/projects/{id}/delete",
                    axum::routing::post(owner_delete_project),
                )
                .route("/healthz", get(healthz))
                .with_state(state))
        }
    }
}
