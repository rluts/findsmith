use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
    routing::{any, get, get_service, post},
};
use mwoauth::axum::{AxumOAuthState, Key, refresh_session, routes as oauth_routes};
use sea_orm::DatabaseConnection;
use tower_http::{
    services::{ServeDir, ServeFile},
    timeout::TimeoutLayer,
};

use crate::{
    AppResult,
    auth::{AuthMode, CurrentUser, oauth_state},
    config::{AuthenticationConfig, Config},
    mediawiki::owner_identity,
    web::{
        AppState, add_wiki, autosave_project, build_project, create_project, delete_project,
        delete_saved_rule, duplicate_project, healthz, job_action, job_queue_action, job_recompute,
        job_review, language_middleware, page_preview, project, readyz, render_preview, save_rule,
        saved_rules, session, set_language, start_job, translation_languages, translations,
        update_project, update_saved_rule, wikis, workspace,
    },
};

pub async fn router(config: Config, database: DatabaseConnection) -> AppResult<Router> {
    let auth = match config.authentication {
        AuthenticationConfig::WebOAuth(oauth_config) => AuthMode::OAuth(Arc::new(oauth_state(
            oauth_config,
            &config.cookie_key,
            &config.auth_wiki_url,
            &config.user_agent,
            config.debug,
        )?)),
        AuthenticationConfig::OwnerToken(access_token) => {
            let identity =
                owner_identity(&config.auth_wiki_url, &config.user_agent, &access_token).await?;
            AuthMode::Owner(CurrentUser {
                central_id: identity.central_id,
                username: identity.username,
                access_token,
                can_logout: false,
            })
        }
    };
    let state = AppState {
        database,
        user_agent: config.user_agent,
        csrf_secure: !config.debug,
        cookie_key: Key::from(&config.cookie_key),
        auth,
    };
    let frontend = get_service(ServeFile::new("frontend/dist/index.html"))
        .layer(axum::middleware::from_fn(disable_frontend_cache));
    let mut router = Router::new()
        .route("/api/session", get(session))
        .route("/api/language", post(set_language))
        .route("/api/translations", get(translation_languages))
        .route("/api/translations/{language}", get(translations))
        .route("/api/workspace", get(workspace))
        .route("/api/wikis", get(wikis).post(add_wiki))
        .route("/api/rules", get(saved_rules).post(save_rule))
        .route("/api/rules/{id}", post(update_saved_rule))
        .route("/api/rules/{id}/delete", post(delete_saved_rule))
        .route("/api/projects", post(create_project))
        .route("/api/projects/{id}/render-preview", post(render_preview))
        .route("/api/projects/{id}", get(project).post(update_project))
        .route("/api/projects/{id}/build", post(build_project))
        .route("/api/projects/{id}/page-preview", post(page_preview))
        .route("/api/projects/{id}/delete", post(delete_project))
        .route("/api/projects/{id}/autosave", post(autosave_project))
        .route("/api/projects/{id}/duplicate", post(duplicate_project))
        .route("/api/projects/{id}/jobs", post(start_job))
        .route("/api/projects/{id}/jobs/{job_id}", get(job_review))
        .route("/api/projects/{id}/jobs/{job_id}/action", post(job_action))
        .route(
            "/api/projects/{id}/jobs/{job_id}/recompute",
            post(job_recompute),
        )
        .route(
            "/api/projects/{id}/jobs/{job_id}/queue",
            post(job_queue_action),
        )
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/api/{*path}", any(api_not_found))
        .nest_service("/assets", ServeDir::new("frontend/dist/assets"))
        .route_service("/", frontend.clone())
        .route_service("/workspace", frontend.clone())
        .route_service("/add-wiki", frontend.clone())
        .route_service("/projects/{id}", frontend.clone())
        .route_service("/projects/{id}/jobs/{job_id}", frontend);
    if let AuthMode::OAuth(oauth) = &state.auth {
        router =
            router.merge(oauth_routes::<AxumOAuthState>().with_state(AxumOAuthState::clone(oauth)));
    }
    router = router
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            language_middleware,
        ))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024));
    if let AuthMode::OAuth(oauth) = &state.auth {
        router = router.layer(axum::middleware::from_fn_with_state(
            AxumOAuthState::clone(oauth),
            refresh_session,
        ));
    }
    Ok(router
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(60),
        ))
        .with_state(state))
}

async fn disable_frontend_cache(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, no-cache, must-revalidate, max-age=0"),
    );
    response
}

async fn api_not_found() -> (StatusCode, axum::Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "title": crate::localization::required("findsmith-api-endpoint-not-found"),
            "message": crate::localization::required("findsmith-api-endpoint-not-found-message")
        })),
    )
}
