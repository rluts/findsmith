use std::{env, error::Error};

use askama::Template;
use axum::{
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use mwoauth::axum::{AxumOAuthState, AxumUser, Key, routes as oauth_routes};
use mwoauth::{MwOAuthClient, MwOAuthConfig};
use url::Url;

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    username: Option<&'a str>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let state = oauth_state()?;
    let app = oauth_routes()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .with_state(state);

    let port = env::var("PORT").unwrap_or_else(|_| "8000".to_owned());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("FindSmith listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn oauth_state() -> Result<AxumOAuthState, Box<dyn Error>> {
    let client_id = required_env("OAUTH_CLIENT_ID")?;
    let client_secret = required_env("OAUTH_CLIENT_SECRET")?;
    let cookie_key = STANDARD
        .decode(required_env("COOKIE_KEY")?.trim())
        .map_err(|_| "COOKIE_KEY must be valid base64")?;
    if cookie_key.len() != 64 {
        return Err("COOKIE_KEY must decode to exactly 64 bytes".into());
    }

    let wiki_url =
        env::var("WIKI_URL").unwrap_or_else(|_| "https://meta.wikimedia.org/w/api.php".to_owned());
    let callback_url = env::var("OAUTH_CALLBACK_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8000/oauth/callback".to_owned());
    let user_agent =
        env::var("USER_AGENT").unwrap_or_else(|_| "FindSmith/0.1 (Wikimedia Toolforge)".to_owned());
    let config = MwOAuthConfig::confidential(
        Url::parse(&wiki_url)?,
        client_id,
        client_secret,
        Url::parse(&callback_url)?,
        user_agent,
    )?;
    let client = MwOAuthClient::new(config)?;

    Ok(AxumOAuthState::new(client, Key::from(&cookie_key)))
}

async fn index(State(_state): State<AxumOAuthState>, user: Option<AxumUser>) -> Response {
    let template = IndexTemplate {
        username: user.as_ref().map(|user| user.profile().username()),
    };

    match template.render() {
        Ok(page) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::REFERRER_POLICY, "same-origin"),
            ],
            Html(page),
        )
            .into_response(),
        Err(error) => {
            eprintln!("failed to render index template: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    env::var(name).map_err(|_| format!("required environment variable {name} is not set").into())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
