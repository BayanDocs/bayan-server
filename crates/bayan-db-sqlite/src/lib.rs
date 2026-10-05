//! SQLite metadata store of the BayanDocs server, the default backend (ADR-0015).
//!
//! The database holds only server metadata needed to route ciphertext; it never stores document plaintext or keys (zero-knowledge invariant, ADR-0015 §6). SQLite is C code, bundled and compiled from source. ADR-0006 §3 allows it here because it only processes metadata the server itself generates, always through bound parameters, never documents or client-supplied SQL.
//!
//! Queries use sqlx's compile-time-checked macros. Their metadata is committed in `.sqlx/` and regenerated with `cargo xtask sqlx-prepare`.

use std::path::Path;
use std::str::FromStr as _;
use std::time::Duration;

use sqlx::ConnectOptions as _;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use sqlx::SqlitePool as Pool;
pub use sqlx::{Error, migrate::MigrateError};

/// The versioned schema migrations, embedded in the binary at compile time.
pub static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// How long a connection waits for another connection's write lock before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Opens (creating it if necessary) the database file at `path`.
///
/// The connections use write-ahead logging, enforce foreign keys, keep temporary tables in memory (the container's root filesystem is read-only), and never log SQL statements.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or created.
pub async fn connect(path: &Path, max_connections: u32) -> Result<Pool, Error> {
    let options = SqliteConnectOptions::from_str("sqlite:")?
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .foreign_keys(true)
        .busy_timeout(BUSY_TIMEOUT)
        .pragma("temp_store", "memory")
        .disable_statement_logging();
    SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
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
