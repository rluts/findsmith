use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::{jobs::JobError, localization, projects::ProjectError};

#[derive(Serialize)]
struct ErrorResponse {
    title: String,
    message: String,
    back_href: String,
}

#[derive(Serialize)]
struct ActionResponse {
    ok: bool,
    redirect: String,
    id: Option<String>,
}

pub(super) fn action_response(redirect: String, id: Option<String>) -> Response {
    Json(ActionResponse {
        ok: true,
        redirect,
        id,
    })
    .into_response()
}

pub(super) fn job_not_found(project_id: &str) -> Response {
    error_page(
        StatusCode::NOT_FOUND,
        "findsmith-api-review-not-found",
        "findsmith-api-this-review-does-not-exist-or-you-do-not-have-access-to-it",
        &format!("/projects/{project_id}"),
    )
}

pub(super) fn job_error_response(error: JobError, back_href: &str) -> Response {
    match error {
        JobError::Conflict => error_page(
            StatusCode::CONFLICT,
            "findsmith-api-review-changed-elsewhere",
            "findsmith-api-reload-the-review-to-get-the-latest-progress-then-try-again",
            back_href,
        ),
        JobError::Database(error) => internal_error(error),
        JobError::Serialization(error) => internal_error(error),
    }
}

pub(super) fn project_error_response(error: ProjectError, back_href: &str) -> Response {
    match error {
        ProjectError::InvalidName => error_page(
            StatusCode::BAD_REQUEST,
            "findsmith-api-invalid-project-name",
            "findsmith-api-project-name-must-contain-between-1-and-255-characters",
            back_href,
        ),
        ProjectError::DuplicateName => error_page(
            StatusCode::CONFLICT,
            "findsmith-api-project-name-already-exists",
            "findsmith-api-choose-a-different-name-for-this-project",
            back_href,
        ),
        ProjectError::Conflict => error_page(
            StatusCode::CONFLICT,
            "findsmith-api-project-changed-elsewhere",
            "findsmith-api-reload-the-project-to-get-the-latest-version-then-apply-your-changes-aga",
            back_href,
        ),
        ProjectError::Database(error) => internal_error(error),
        ProjectError::Serialization(error) => internal_error(error),
    }
}

pub(super) fn csrf_error(back_href: &str) -> Response {
    error_page(
        StatusCode::FORBIDDEN,
        "findsmith-api-request-could-not-be-verified",
        "findsmith-api-reload-the-page-and-try-again-no-changes-were-made",
        back_href,
    )
}

pub(super) fn error_page(
    status: StatusCode,
    title: &str,
    message: &str,
    back_href: &str,
) -> Response {
    render_with_status(
        status,
        ErrorResponse {
            title: localization::from_source(title),
            message: localization::from_source(message),
            back_href: back_href.to_owned(),
        },
    )
}

pub(super) fn render(template: impl Serialize) -> Response {
    render_with_status(StatusCode::OK, template)
}

fn render_with_status(status: StatusCode, template: impl Serialize) -> Response {
    (
        status,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "same-origin"),
        ],
        Json(template),
    )
        .into_response()
}

pub(super) fn internal_error(error: impl std::error::Error) -> Response {
    eprintln!("request failed: {error}");
    error_page_without_rendering()
}

fn error_page_without_rendering() -> Response {
    render_with_status(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorResponse {
            title: "Unexpected error".to_owned(),
            message: "An unexpected error occurred.".to_owned(),
            back_href: "/".to_owned(),
        },
    )
}
