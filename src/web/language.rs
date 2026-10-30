use axum::{
    Json,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};

use crate::{auth::CurrentUser, csrf, localization};

use super::AppState;
use super::responses::{csrf_error, error_page, internal_error, render};

pub async fn language_middleware(
    State(state): State<AppState>,
    user: Option<CurrentUser>,
    jar: PrivateCookieJar,
    request: Request,
    next: Next,
) -> Response {
    if !request.uri().path().starts_with("/api/")
        || request.uri().path() == "/api/translations"
        || request.uri().path().starts_with("/api/translations/")
    {
        return next.run(request).await;
    }
    let owner = user.as_ref().map(|user| user.central_id.as_str());
    match localization::preferred_language(&state.database, owner, &jar).await {
        Ok(language) => localization::in_language(language, next.run(request)).await,
        Err(error) => internal_error(error),
    }
}

#[derive(Deserialize)]
pub struct LanguageForm {
    language: String,
    csrf_token: String,
}

#[derive(Serialize)]
struct LanguageResponse<'a> {
    language: &'a str,
}

pub async fn translations(Path(language): Path<String>) -> Response {
    match localization::catalog(&language) {
        Some(catalog) => (
            [
                (header::CACHE_CONTROL, "public, max-age=300"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            Json(catalog),
        )
            .into_response(),
        None => error_page(
            StatusCode::NOT_FOUND,
            "findsmith-api-unknown-language",
            "findsmith-api-no-translation-catalog-exists-for-this-language",
            "/",
        ),
    }
}

pub async fn translation_languages() -> Response {
    (
        [
            (header::CACHE_CONTROL, "public, max-age=300"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Json(localization::available_languages()),
    )
        .into_response()
}

async fn save_language(
    database: &DatabaseConnection,
    owner: Option<&str>,
    jar: PrivateCookieJar,
    secure: bool,
    form: LanguageForm,
) -> (PrivateCookieJar, Response) {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return (jar, csrf_error("/"));
    }
    let language = form.language.to_ascii_lowercase();
    if !localization::valid_language(&language) {
        return (
            jar,
            error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-invalid-language",
                "findsmith-api-use-a-valid-language-code",
                "/",
            ),
        );
    }
    if !localization::available_language(&language) {
        return (
            jar,
            error_page(
                StatusCode::BAD_REQUEST,
                "findsmith-api-language-not-available",
                "findsmith-api-this-interface-language-is-not-available-yet",
                "/",
            ),
        );
    }
    if let Err(error) = localization::save_language(database, owner, &language).await {
        return (jar, internal_error(error));
    }
    (
        localization::set_cookie(jar, &language, secure),
        render(LanguageResponse {
            language: &language,
        }),
    )
}

pub async fn set_language(
    State(state): State<AppState>,
    user: Option<CurrentUser>,
    jar: PrivateCookieJar,
    Form(form): Form<LanguageForm>,
) -> (PrivateCookieJar, Response) {
    let owner = user.as_ref().map(|user| user.central_id.as_str());
    save_language(&state.database, owner, jar, state.csrf_secure, form).await
}
