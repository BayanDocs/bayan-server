//! The metadata database: SQLite by default, PostgreSQL when configured (ADR-0015 §3).
//!
//! The rest of the server talks to [`Database`] and never to a specific backend. Queries live in the backend crates (`bayan-db-sqlite`, `bayan-db-postgres`), which have their own migrations and compile-time-checked queries.

use std::path::Path;

use crate::config::DatabaseConfig;

/// A connection pool for the configured metadata database.
#[derive(Debug, Clone)]
pub enum Database {
    /// The embedded SQLite database.
    Sqlite(bayan_db_sqlite::Pool),
    /// A PostgreSQL server.
    Postgres(bayan_db_postgres::Pool),
}

/// Why the database could not be opened or migrated.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    /// The data directory could not be created.
    #[error("cannot create the data directory: {0}")]
    DataDir(std::io::ErrorKind),
    /// The SQLite database could not be opened.
    #[error("cannot open the SQLite database: {0}")]
    Sqlite(#[source] bayan_db_sqlite::Error),
    /// The PostgreSQL database could not be reached.
    #[error(transparent)]
    Postgres(bayan_db_postgres::ConnectError),
    /// A schema migration failed. (Both backends re-export the same sqlx error type.)
    #[error("database migration failed: {0}")]
    Migrate(#[source] bayan_db_sqlite::MigrateError),
}

impl Database {
    /// Opens the configured database and applies pending migrations.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened, reached or migrated.
    pub async fn open(
        config: &DatabaseConfig,
        max_connections: u32,
    ) -> Result<Self, DatabaseError> {
        let database = match config {
            DatabaseConfig::Sqlite { path } => {
                if let Some(parent) = path.parent() {
                    create_private_dir(parent)
                        .map_err(|error| DatabaseError::DataDir(error.kind()))?;
                }
                Self::Sqlite(
                    bayan_db_sqlite::connect(path, max_connections)
                        .await
                        .map_err(DatabaseError::Sqlite)?,
                )
            }
            DatabaseConfig::Postgres { url } => Self::Postgres(
                bayan_db_postgres::connect(url.expose(), max_connections)
                    .await
                    .map_err(DatabaseError::Postgres)?,
            ),
        };
        match &database {
            Self::Sqlite(pool) => bayan_db_sqlite::migrate(pool).await,
            Self::Postgres(pool) => bayan_db_postgres::migrate(pool).await,
        }
        .map_err(DatabaseError::Migrate)?;
        Ok(database)
    }

    /// Short name of the backend, for logs.
    #[must_use]
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Sqlite(_) => "sqlite",
            Self::Postgres(_) => "postgres",
        }
    }

    /// Returns whether the database is reachable and migrated. Failures are reported as `false`.
    pub async fn is_ready(&self) -> bool {
        let result = match self {
            Self::Sqlite(pool) => bayan_db_sqlite::is_ready(pool).await,
            Self::Postgres(pool) => bayan_db_postgres::is_ready(pool).await,
        };
        match result {
            Ok(ready) => ready,
            Err(error) => {
                tracing::warn!(backend = self.backend(), error = %error, "database readiness check failed");
                false
            }
        }
    }

    /// Closes every connection, waiting for queries in progress.
    pub async fn close(&self) {
        match self {
            Self::Sqlite(pool) => pool.close().await,
            Self::Postgres(pool) => pool.close().await,
        }
    }
}

/// Creates `dir` (and its parents) if missing; a newly created directory is readable only by the server's user.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}
