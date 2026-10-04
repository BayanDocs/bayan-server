//! PostgreSQL metadata store of the BayanDocs server, the optional backend for larger deployments (ADR-0015).
//!
//! The database holds only server metadata needed to route ciphertext; it never stores document plaintext or keys (zero-knowledge invariant, ADR-0015 §6). The driver is pure Rust.
//!
//! **No TLS yet:** every TLS backend sqlx offers needs C or assembly code, which SRV-001 may not add (ADR-0006), so connections are unencrypted and the database must be reachable only over a trusted network (for example the same host or a private container network). URLs that require TLS (`sslmode=require` and stronger) fail to connect.
//!
//! Queries use sqlx's compile-time-checked macros. Their metadata is committed in `.sqlx/` and regenerated with `cargo xtask sqlx-prepare`.

use std::str::FromStr as _;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

pub use sqlx::PgPool as Pool;
pub use sqlx::{Error, migrate::MigrateError};

/// The versioned schema migrations, embedded in the binary at compile time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// How long to wait for a free connection, or for a new connection to be established, before failing.
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors from [`connect`].
#[derive(Debug)]
pub enum ConnectError {
    /// The URL is not a valid PostgreSQL connection URL. The URL is deliberately not included because it usually contains a password.
    InvalidUrl,
    /// The database could not be reached or refused the connection.
    Database(Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUrl => f.write_str("the PostgreSQL URL is not valid"),
            Self::Database(error) => write!(f, "cannot connect to PostgreSQL: {error}"),
        }
    }
}

impl std::error::Error for ConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidUrl => None,
            Self::Database(error) => Some(error),
        }
    }
}

/// Connects to the PostgreSQL database at `url` (`postgres://user:password@host:port/database`).
///
/// SQL statements are never logged.
///
/// # Errors
///
/// Returns [`ConnectError::InvalidUrl`] if the URL cannot be parsed and [`ConnectError::Database`] if the database cannot be reached.
pub async fn connect(url: &str, max_connections: u32) -> Result<Pool, ConnectError> {
    let options = PgConnectOptions::from_str(url)
        .map_err(|_| ConnectError::InvalidUrl)?
        .application_name("bayan-server")
        .disable_statement_logging();
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect_with(options)
        .await
        .map_err(ConnectError::Database)
}

/// Applies every migration that has not been applied yet.
///
/// # Errors
///
/// Returns an error if a migration fails or the database contains migrations this binary does not know (for example after a downgrade).
pub async fn migrate(pool: &Pool) -> Result<(), MigrateError> {
    MIGRATOR.run(pool).await
}

/// Returns whether the database is reachable and its schema has been initialized.
///
/// # Errors
///
/// Returns an error if the query fails, for example because the database is unreachable or not migrated.
pub async fn is_ready(pool: &Pool) -> Result<bool, Error> {
    let row = sqlx::query_scalar!("SELECT id FROM server_instance WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    Ok(row == Some(1))
}
