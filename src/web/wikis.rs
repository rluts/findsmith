use crate::auth::CurrentUser;
use axum::{extract::State, http::StatusCode, response::Response};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::{
    csrf,
    wikis::{self, WikiView},
};

use super::AppState;
use super::responses::{action_response, csrf_error, error_page, internal_error, render};

#[derive(Deserialize)]
pub struct AddWikiForm {
    domain: String,
    csrf_token: String,
}

#[derive(Serialize)]
struct WikisResponse {
    wikis: Vec<WikiView>,
    csrf_token: String,
}

pub async fn wikis(
    State(state): State<AppState>,
    _user: CurrentUser,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let (jar, token) = csrf::token(jar, state.csrf_secure);
    (jar, wikis_response(&state.database, token).await)
}

async fn wikis_response(database: &DatabaseConnection, csrf_token: String) -> Response {
    match wikis::list(database).await {
        Ok(items) => render(WikisResponse {
            wikis: items.into_iter().map(WikiView::from).collect(),
            csrf_token,
        }),
        Err(error) => internal_error(error),
    }
}

pub async fn add_wiki(
    State(state): State<AppState>,
    _user: CurrentUser,
    jar: PrivateCookieJar,
    Form(form): Form<AddWikiForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/add-wiki");
    }
    add_wiki_response(&state.database, &state.user_agent, &form.domain).await
}

async fn add_wiki_response(
    database: &DatabaseConnection,
    user_agent: &str,
    domain: &str,
) -> Response {
    match wikis::register(database, user_agent, domain).await {
        Ok(wiki) => action_response("/workspace".into(), Some(wiki.id)),
        Err(message) => error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-could-not-add-wiki",
            &message,
            "/add-wiki",
        ),
    }
}
