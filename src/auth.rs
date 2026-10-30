use std::{convert::Infallible, sync::Arc};

use axum::{
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::{StatusCode, request::Parts},
};
use mwoauth::axum::{AxumOAuthState, AxumUser, CookieConfig, Key};
use mwoauth::{MwOAuthClient, MwOAuthConfig};
use url::Url;

use crate::{AppResult, config::WebOAuthConfig, web::AppState};

/// How requests are authenticated, chosen once at startup.
#[derive(Clone)]
pub enum AuthMode {
    /// Users sign in through Wikimedia OAuth; tokens live in the session cookie.
    OAuth(Arc<AxumOAuthState>),
    /// Local development: every request acts as one fixed account.
    Owner(CurrentUser),
}

/// The signed-in user, independent of the authentication mode.
#[derive(Clone)]
pub struct CurrentUser {
    pub central_id: String,
    pub username: String,
    pub access_token: String,
    pub can_logout: bool,
}

impl CurrentUser {
    fn from_oauth(user: AxumUser) -> Self {
        Self {
            central_id: user.profile().central_id().to_owned(),
            username: user.profile().username().to_owned(),
            access_token: user.oauth2_token().to_owned(),
            can_logout: true,
        }
    }
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = StatusCode;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        match &state.auth {
            AuthMode::Owner(user) => Ok(user.clone()),
            AuthMode::OAuth(oauth) => {
                let user =
                    <AxumUser as FromRequestParts<_>>::from_request_parts(parts, oauth.as_ref())
                        .await?;
                Ok(Self::from_oauth(user))
            }
        }
    }
}

impl OptionalFromRequestParts<AppState> for CurrentUser {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Option<Self>, Self::Rejection> {
        match &state.auth {
            AuthMode::Owner(user) => Ok(Some(user.clone())),
            AuthMode::OAuth(oauth) => {
                let user = <AxumUser as OptionalFromRequestParts<_>>::from_request_parts(
                    parts,
                    oauth.as_ref(),
                )
                .await?;
                Ok(user.map(Self::from_oauth))
            }
        }
    }
}

pub fn oauth_state(
    config: WebOAuthConfig,
    cookie_key: &[u8],
    wiki_url: &str,
    user_agent: &str,
    debug: bool,
) -> AppResult<AxumOAuthState> {
    let oauth_config = MwOAuthConfig::confidential(
        Url::parse(wiki_url)?,
        config.client_id,
        config.client_secret,
        config.callback_url,
        user_agent,
    )?;
    let client = MwOAuthClient::new(oauth_config)?;
    let cookies = CookieConfig {
        secure: !debug,
        ..CookieConfig::default()
    };

    Ok(AxumOAuthState::new(client, Key::from(cookie_key)).with_cookie_config(cookies))
}
