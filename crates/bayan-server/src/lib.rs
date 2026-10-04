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

use tokio::net::TcpListener;

use crate::config::Config;
use crate::db::{Database, DatabaseError};

/// Why the server stopped with an error.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The database could not be opened or migrated.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// The HTTP server failed.
    #[error("HTTP server error: {0}")]
    Serve(std::io::ErrorKind),
}

/// Opens the database, then serves HTTP on `listener` until `shutdown` completes.
///
/// After `shutdown` completes the server stops accepting connections and gives requests in progress up to [`Config::shutdown_grace`] to finish, then closes the database.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or the HTTP server fails.
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

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        let _ = stop_rx.await;
    });
    let mut server = std::pin::pin!(server.into_future());
    let result = tokio::select! {
        result = &mut server => result,
        () = shutdown => {
            tracing::info!(grace_secs = config.shutdown_grace.as_secs(), "shutting down");
            let _ = stop_tx.send(());
            if let Ok(result) = tokio::time::timeout(config.shutdown_grace, &mut server).await {
                result
            } else {
                tracing::warn!("requests still running after the shutdown grace period; stopping anyway");
                Ok(())
            }
        }
    };
    database.close().await;
    tracing::info!("stopped");
    result.map_err(|error| RunError::Serve(error.kind()))
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
