use axum::{Router, routing::get};
use mwoauth::axum::routes as oauth_routes;

use crate::{
    AppResult,
    auth::oauth_state,
    config::{AuthenticationConfig, Config},
    mediawiki::owner_username,
    web::{OwnerState, healthz, owner_index, web_oauth_index},
};

pub async fn router(config: Config) -> AppResult<Router> {
    match config.authentication {
        AuthenticationConfig::WebOAuth(oauth_config) => {
            let state = oauth_state(
                oauth_config,
                &config.wiki_url,
                &config.user_agent,
                config.debug,
            )?;
            Ok(oauth_routes()
                .route("/", get(web_oauth_index))
                .route("/healthz", get(healthz))
                .with_state(state))
        }
        AuthenticationConfig::OwnerToken(access_token) => {
            let username =
                owner_username(&config.wiki_url, &config.user_agent, &access_token).await?;
            let state = OwnerState {
                username,
                access_token,
            };
            Ok(Router::new()
                .route("/", get(owner_index))
                .route("/healthz", get(healthz))
                .with_state(state))
        }
    }
}
