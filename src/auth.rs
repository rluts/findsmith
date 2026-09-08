use mwoauth::axum::{AxumOAuthState, AxumUser, CookieConfig, Key};
use mwoauth::{MwOAuthClient, MwOAuthConfig};
use url::Url;

use crate::{AppResult, config::WebOAuthConfig};

#[derive(Clone, Copy)]
pub enum Role {
    User,
    Admin,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
        }
    }
}

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

    pub fn role(&self) -> Role {
        match self {
            Self::WebOAuth(_) => Role::User,
            Self::OwnerToken { .. } => Role::Admin,
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

    Ok(AxumOAuthState::new(client, Key::from(&config.cookie_key)).with_cookie_config(cookies))
}
