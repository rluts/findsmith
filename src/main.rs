use std::error::Error;

mod app;
mod auth;
mod config;
mod csrf;
mod database;
mod diff;
mod entities;
mod jobs;
mod lists;
mod masking;
mod mediawiki;
mod migration;
mod projects;
mod rules;
mod saved_rules;
mod structural;
mod web;
mod wikis;

type AppResult<T> = Result<T, Box<dyn Error>>;

#[tokio::main]
async fn main() -> AppResult<()> {
    let config = config::Config::load()?;
    let port = config.port.clone();
    let database = database::connect(config.database.clone()).await?;
    let app = app::router(config, database).await?;
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("FindSmith listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
