use axum::{
    extract::{Query, State},
    response::Response,
};
use axum_extra::extract::cookie::PrivateCookieJar;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::{
    auth::CurrentUser,
    csrf, localization,
    projects::ProjectRepository,
    wikis::{self},
};

use super::AppState;
use super::responses::{internal_error, project_error_response, render};

#[derive(Serialize)]
struct SessionResponse<'a> {
    username: Option<&'a str>,
    groups: Vec<String>,
    groups_error: Option<String>,
    can_logout: bool,
    language: &'a str,
    csrf_token: &'a str,
}

#[derive(Default, Deserialize)]
pub struct SessionQuery {
    #[serde(default)]
    include_groups: bool,
    wiki_id: Option<String>,
}

pub async fn session(
    State(state): State<AppState>,
    user: Option<CurrentUser>,
    Query(query): Query<SessionQuery>,
    jar: PrivateCookieJar,
) -> (PrivateCookieJar, Response) {
    let owner = user.as_ref().map(|user| user.central_id.as_str());
    let language = match localization::preferred_language(&state.database, owner, &jar).await {
        Ok(language) => language,
        Err(error) => return (jar, internal_error(error)),
    };
    let jar = if localization::cookie_language(&jar).as_deref() != Some(&language) {
        localization::set_cookie(jar, &language, state.csrf_secure)
    } else {
        jar
    };
    let (jar, csrf_token) = csrf::token(jar, state.csrf_secure);
    let response = render_index(
        user.as_ref(),
        &state.database,
        &state.user_agent,
        query.include_groups,
        query.wiki_id.as_deref(),
        &language,
        &csrf_token,
    )
    .await;
    (jar, response)
}

#[allow(clippy::too_many_arguments)]
async fn render_index(
    user: Option<&CurrentUser>,
    database: &DatabaseConnection,
    user_agent: &str,
    include_groups: bool,
    requested_wiki_id: Option<&str>,
    language: &str,
    csrf_token: &str,
) -> Response {
    if let Some(user) = user {
        debug_assert!(!user.access_token.is_empty());
    }
    let (groups, groups_error) = match (user, include_groups, requested_wiki_id) {
        (Some(user), true, Some(requested_wiki_id)) => {
            let wiki = match wikis::by_id(database, requested_wiki_id).await {
                Ok(wiki) => wiki,
                Err(error) => return internal_error(error),
            };
            let wiki_url = if let Some(wiki) = wiki {
                Some(wiki.api_url)
            } else {
                match ProjectRepository::new(database)
                    .list(&user.central_id)
                    .await
                {
                    Ok(projects) => projects
                        .into_iter()
                        .find(|project| project.wiki_url == requested_wiki_id)
                        .map(|project| project.wiki_url),
                    Err(error) => return project_error_response(error, "/workspace"),
                }
            };
            let Some(wiki_url) = wiki_url else {
                return render(SessionResponse {
                    username: Some(&user.username),
                    groups: Vec::new(),
                    groups_error: Some(format!(
                        "Could not load groups: unsupported wiki {requested_wiki_id}."
                    )),
                    can_logout: user.can_logout,
                    language,
                    csrf_token,
                });
            };
            match crate::mediawiki::user_groups(&wiki_url, user_agent, &user.access_token).await {
                Ok(groups) => (groups, None),
                Err(error) => (
                    Vec::new(),
                    Some(localization::required_with_args(
                        "findsmith-api-groups-load",
                        &[requested_wiki_id, &error.to_string()],
                    )),
                ),
            }
        }
        _ => (Vec::new(), None),
    };
    render(SessionResponse {
        username: user.map(|user| user.username.as_str()),
        groups,
        groups_error,
        can_logout: user.is_some_and(|user| user.can_logout),
        language,
        csrf_token,
    })
}
