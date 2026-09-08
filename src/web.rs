use askama::Template;
use axum::{
    extract::State,
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use mwoauth::axum::{AxumOAuthState, AxumUser};

use crate::auth::Authentication;

#[derive(Clone)]
pub struct OwnerState {
    pub username: String,
    pub access_token: String,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    username: Option<&'a str>,
    role: Option<&'a str>,
    can_logout: bool,
}

pub async fn web_oauth_index(
    State(_state): State<AxumOAuthState>,
    user: Option<AxumUser>,
) -> Response {
    let authentication = user.map(|user| Authentication::WebOAuth(Box::new(user)));
    render_index(authentication.as_ref())
}

pub async fn owner_index(State(state): State<OwnerState>) -> Response {
    let authentication = Authentication::OwnerToken {
        username: state.username.clone(),
        access_token: state.access_token.clone(),
    };
    render_index(Some(&authentication))
}

pub async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

fn render_index(authentication: Option<&Authentication>) -> Response {
    if let Some(authentication) = authentication {
        debug_assert!(!authentication.access_token().is_empty());
    }
    let template = IndexTemplate {
        username: authentication.map(Authentication::username),
        role: authentication.map(|authentication| authentication.role().as_str()),
        can_logout: authentication.is_some_and(Authentication::can_logout),
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
