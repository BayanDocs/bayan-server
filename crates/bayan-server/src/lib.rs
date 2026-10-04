//! The BayanDocs collaboration server (ADR-0015).
//!
//! A single binary that will store and relay end-to-end-encrypted collaboration data and serve the web app. It is **zero-knowledge**: no code path receives, stores, derives or logs document plaintext or document keys.
//!
//! This crate holds the server skeleton: typed configuration ([`config`]), content-free structured logging ([`logging`]), the metadata database ([`db`]), the HTTP application ([`http`]) and the process lifecycle ([`run`]). Accounts, document storage, WebSockets and MLS come in later work packages.

pub mod config;
pub mod db;
pub mod healthcheck;
pub mod http;
pub mod logging;
pub mod version;

use std::future::Future;
use std::time::Duration;

use tokio::net::TcpListener;

use crate::config::Config;
use crate::db::{Database, DatabaseError};

/// Why the server stopped with an error.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The database could not be opened or migrated.
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

/// How long closing the database may take after the HTTP server has stopped. Requests still running after the shutdown grace period may hold connections; the process does not wait for them beyond this.
pub const DATABASE_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Opens the database, then serves HTTP on `listener` until `shutdown` completes.
///
/// After `shutdown` completes the server stops accepting connections and gives requests in progress up to [`Config::shutdown_grace`] to finish, then closes the database (waiting at most [`DATABASE_CLOSE_TIMEOUT`]). Connections that do not send a complete request head within [`Config::header_read_timeout`] are closed.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or migrated.
pub async fn run(
    config: &Config,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), RunError> {
    let database = Database::open(&config.database, config.database_max_connections).await?;
    tracing::info!(backend = database.backend(), "database ready");
    let app = http::app(
        config,
        http::AppState {
            database: database.clone(),
        },
    );
    http::serve(
        listener,
        app,
        config.header_read_timeout,
        config.shutdown_grace,
        shutdown,
    )
    .await;
    if tokio::time::timeout(DATABASE_CLOSE_TIMEOUT, database.close())
        .await
        .is_err()
    {
        tracing::warn!("database connections still in use; stopping without waiting for them");
    }
    tracing::info!("stopped");
    Ok(())
}

/// Completes when the process receives `SIGTERM` (sent by container runtimes) or `SIGINT` (Ctrl+C).
pub async fn shutdown_signal() {
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
}
