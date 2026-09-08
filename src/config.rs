use std::env;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use url::Url;

use crate::AppResult;

pub struct Config {
    pub debug: bool,
    pub port: String,
    pub wiki_url: String,
    pub user_agent: String,
    pub authentication: AuthenticationConfig,
}

pub enum AuthenticationConfig {
    WebOAuth(WebOAuthConfig),
    OwnerToken(String),
}

pub struct WebOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub cookie_key: Vec<u8>,
    pub callback_url: Url,
}

impl Config {
    pub fn load() -> AppResult<Self> {
        let debug = debug_enabled()?;
        let owner_access_token = env::var("OWNER_ACCESS_TOKEN").ok();
        if owner_access_token.is_some() && !debug {
            panic!("OWNER_ACCESS_TOKEN is only allowed when DEBUG=true");
        }

        let authentication = match owner_access_token {
            Some(access_token) => {
                if access_token.trim().is_empty() {
                    return Err("OWNER_ACCESS_TOKEN must not be empty".into());
                }
                AuthenticationConfig::OwnerToken(access_token)
            }
            None => AuthenticationConfig::WebOAuth(web_oauth_config(debug)?),
        };

        Ok(Self {
            debug,
            port: env::var("PORT").unwrap_or_else(|_| "8000".to_owned()),
            wiki_url: env::var("WIKI_URL")
                .unwrap_or_else(|_| "https://meta.wikimedia.org/w/api.php".to_owned()),
            user_agent: env::var("USER_AGENT")
                .unwrap_or_else(|_| "FindSmith/0.1 (toolsforge.wikimedia.org)".to_owned()),
            authentication,
        })
    }
}

fn web_oauth_config(debug: bool) -> AppResult<WebOAuthConfig> {
    let cookie_key = STANDARD
        .decode(required_env("COOKIE_KEY")?.trim())
        .map_err(|_| "COOKIE_KEY must be valid base64")?;
    if cookie_key.len() != 64 {
        return Err("COOKIE_KEY must decode to exactly 64 bytes".into());
    }

    let callback_url = env::var("OAUTH_CALLBACK_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8000/oauth/callback".to_owned());
    let callback_url = Url::parse(&callback_url)?;
    let local_callback = matches!(
        callback_url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1")
    );
    if debug && !local_callback {
        return Err("DEBUG=true is only allowed for local callback URLs".into());
    }

    Ok(WebOAuthConfig {
        client_id: required_env("OAUTH_CLIENT_ID")?,
        client_secret: required_env("OAUTH_CLIENT_SECRET")?,
        cookie_key,
        callback_url,
    })
}

fn debug_enabled() -> AppResult<bool> {
    match env::var("DEBUG") {
        Ok(value) if value == "true" => Ok(true),
        Ok(value) if value == "false" => Ok(false),
        Ok(_) => Err("DEBUG must be either true or false".into()),
        Err(env::VarError::NotPresent) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn required_env(name: &str) -> AppResult<String> {
    env::var(name).map_err(|_| format!("required environment variable {name} is not set").into())
}
