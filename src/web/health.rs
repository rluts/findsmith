use super::AppState;
use axum::{extract::State, http::StatusCode};
use std::time::Duration;

pub async fn healthz() -> StatusCode {
    StatusCode::NO_CONTENT
}

pub async fn readyz(State(state): State<AppState>) -> StatusCode {
    let database = &state.database;
    let result = tokio::time::timeout(Duration::from_secs(2), database.ping()).await;
    match result {
        Ok(Ok(_)) => StatusCode::NO_CONTENT,
        Err(_) | Ok(Err(_)) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
