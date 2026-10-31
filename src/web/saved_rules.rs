use crate::auth::CurrentUser;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use axum_extra::extract::{Form, cookie::PrivateCookieJar};
use sea_orm::DatabaseConnection;
use serde::Deserialize;

use crate::{csrf, projects::ReplacementRule, saved_rules};

use super::AppState;
use super::responses::{csrf_error, error_page};

#[derive(Deserialize)]
pub struct SaveRuleForm {
    csrf_token: String,
    rule_json: String,
}

#[derive(Deserialize)]
pub struct DeleteRuleForm {
    csrf_token: String,
}

pub async fn saved_rules(State(state): State<AppState>, user: CurrentUser) -> Response {
    saved_rules_response(&state.database, &user.central_id).await
}

async fn saved_rules_response(database: &DatabaseConnection, owner: &str) -> Response {
    match saved_rules::list(database, owner).await {
        Ok(rules) => Json(rules).into_response(),
        Err(message) => error_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Saved rules unavailable",
            &message,
            "/workspace",
        ),
    }
}

pub async fn save_rule(
    State(state): State<AppState>,
    user: CurrentUser,
    jar: PrivateCookieJar,
    Form(form): Form<SaveRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    save_rule_response(&state.database, &user.central_id, None, form).await
}

pub async fn update_saved_rule(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<SaveRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    save_rule_response(&state.database, &user.central_id, Some(&id), form).await
}

async fn save_rule_response(
    database: &DatabaseConnection,
    owner: &str,
    id: Option<&str>,
    form: SaveRuleForm,
) -> Response {
    let rule: ReplacementRule = match serde_json::from_str(&form.rule_json) {
        Ok(rule) => rule,
        Err(_) => {
            return error_page(
                StatusCode::BAD_REQUEST,
                "Invalid rule",
                "Rule data is invalid.",
                "/workspace",
            );
        }
    };
    match saved_rules::save(database, owner, id, rule).await {
        Ok(saved) => Json(saved).into_response(),
        Err(message) => error_page(
            if message == "findsmith-api-saved-rule-unavailable" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            },
            "findsmith-api-could-not-save-rule",
            &message,
            "/workspace",
        ),
    }
}

pub async fn delete_saved_rule(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<String>,
    jar: PrivateCookieJar,
    Form(form): Form<DeleteRuleForm>,
) -> Response {
    if csrf::verify(&jar, &form.csrf_token).is_err() {
        return csrf_error("/workspace");
    }
    delete_saved_rule_response(&state.database, &user.central_id, &id).await
}

async fn delete_saved_rule_response(
    database: &DatabaseConnection,
    owner: &str,
    id: &str,
) -> Response {
    match saved_rules::delete(database, owner, id).await {
        Ok(true) => Json(serde_json::json!({"ok": true})).into_response(),
        Ok(false) => error_page(
            StatusCode::NOT_FOUND,
            "findsmith-api-saved-rule-not-found",
            "findsmith-api-saved-rule-unavailable",
            "/workspace",
        ),
        Err(message) => error_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "findsmith-api-could-not-delete-rule",
            &message,
            "/workspace",
        ),
    }
}
