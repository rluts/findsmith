use std::error::Error;

mod app;
mod auth;
mod config;
mod csrf;
mod database;
mod diff;
mod dotnet_regex;
mod entities;
mod error;
mod jobs;
mod lists;
mod localization;
mod masking;
mod mediawiki;
mod migration;
mod policy;
mod projects;
mod rules;
mod saved_rules;
mod skip;
mod structural;
mod typos;
mod web;
mod wikis;

type AppResult<T> = Result<T, Box<dyn Error>>;

#[tokio::main]
async fn main() -> AppResult<()> {
    let config = config::Config::load()?;
    let port = config.port.clone();
    let database = database::connect(config.database.clone()).await?;
    web::spawn_build_recovery(database.clone());
    wikis::spawn_refresh(database.clone(), config.user_agent.clone());
    let app = app::router(config, database).await?;
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    println!("FindSmith listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => {},
        _ = tokio::signal::ctrl_c() => {}
    }
    println!("FindSmith shutting down");
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    println!("FindSmith shutting down");
}
