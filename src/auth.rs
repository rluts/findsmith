use mwoauth::axum::{AxumOAuthState, AxumUser, CookieConfig, Key};
use mwoauth::{MwOAuthClient, MwOAuthConfig};
use url::Url;

use crate::{AppResult, config::WebOAuthConfig};

pub enum Authentication {
    WebOAuth(Box<AxumUser>),
    OwnerToken {
        username: String,
        access_token: String,
    },
}

impl Authentication {
    pub fn username(&self) -> &str {
        match self {
            Self::WebOAuth(user) => user.profile().username(),
            Self::OwnerToken { username, .. } => username,
        }
    }

    pub fn can_logout(&self) -> bool {
        matches!(self, Self::WebOAuth(_))
    }

    pub fn access_token(&self) -> &str {
        match self {
            Self::WebOAuth(user) => user.oauth2_token(),
            Self::OwnerToken { access_token, .. } => access_token,
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
