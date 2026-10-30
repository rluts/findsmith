use axum::extract::FromRef;
use mwoauth::axum::Key;
use sea_orm::DatabaseConnection;

use crate::auth::AuthMode;

mod edit;
mod health;
mod job_actions;
mod language;
mod preview;
mod project_form;
mod projects;
mod queue_build;
mod responses;
mod review;
mod saved_rules;
mod session;
mod summary;
mod wikis;

pub use health::{healthz, readyz};
pub use job_actions::{job_action, job_queue_action};
pub use language::{language_middleware, set_language, translation_languages, translations};
pub use preview::{page_preview, render_preview};
pub use projects::{
    autosave_project, create_project, delete_project, duplicate_project, project, update_project,
    workspace,
};
pub use queue_build::{build_project, start_job};
pub use review::{job_recompute, job_review};
pub use saved_rules::{delete_saved_rule, save_rule, saved_rules, update_saved_rule};
pub use session::session;
pub use wikis::{add_wiki, wikis};

#[derive(Clone)]
pub struct AppState {
    pub database: DatabaseConnection,
    pub user_agent: String,
    pub csrf_secure: bool,
    pub cookie_key: Key,
    pub auth: AuthMode,
}

impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
