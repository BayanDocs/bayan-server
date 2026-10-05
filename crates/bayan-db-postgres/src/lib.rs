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

/// The query parameters that the PostgreSQL driver (sqlx 0.9) reads from a connection URL, besides `options[NAME]` for any name.
///
/// The driver silently ignores any other parameter, so a misspelled `sslmode` would quietly leave the connection weaker than configured, and it logs the ignored parameter's name and value, which may be a secret. [`check_url`] therefore refuses other parameters. The list mirrors `sqlx-postgres/src/options/parse.rs`; the test `url_parameters_match_the_driver` compares the two, so a sqlx upgrade that changes the set fails it.
pub const URL_PARAMETERS: [&str; 18] = [
    "sslmode",
    "ssl-mode",
    "sslrootcert",
    "ssl-root-cert",
    "ssl-ca",
    "sslcert",
    "ssl-cert",
    "sslkey",
    "ssl-key",
    "statement-cache-capacity",
    "host",
    "hostaddr",
    "port",
    "dbname",
    "user",
    "password",
    "application_name",
    "options",
];

/// What [`check_url`] found wrong with a URL. It never contains any part of the URL, which usually holds a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlProblem {
    /// The query parameter at this position (the first is 1) is not one the driver reads.
    UnrecognizedParameter(usize),
    /// The URL contains `#`. The driver ignores everything after it (a URL fragment), and a `#` in the user name or password makes the URL invalid.
    Fragment,
}

/// Checks that the driver will read every part of `url`'s query: each parameter name must be in [`URL_PARAMETERS`], or be `options[NAME]`, written exactly so (same letter case, not percent-encoded). Also refuses URLs containing `#`. Only the names are inspected, never values or other parts of the URL.
///
/// # Errors
///
/// Returns the first problem found.
pub fn check_url(url: &str) -> Result<(), UrlProblem> {
    if url.contains('#') {
        return Err(UrlProblem::Fragment);
    }
    // Without `#`, the query is everything after the first `?`: neither the host part nor the path of a valid URL can contain one.
    let Some((_, query)) = url.split_once('?') else {
        return Ok(());
    };
    // Split the way the driver does: parameters are separated by `&`, empty ones are skipped, and a name ends at the first `=`. Names are compared as written, without decoding, so a percent-encoded name is refused even if it would decode to a known one.
    let parameters = query.split('&').filter(|parameter| !parameter.is_empty());
    for (index, parameter) in parameters.enumerate() {
        let name = parameter
            .split_once('=')
            .map_or(parameter, |(name, _)| name);
        let known =
            URL_PARAMETERS.contains(&name) || (name.starts_with("options[") && name.ends_with(']'));
        if !known {
            return Err(UrlProblem::UnrecognizedParameter(index + 1));
        }
    }
    Ok(())
}

/// Errors from [`connect`].
#[derive(Debug)]
pub enum ConnectError {
    /// The URL is not a valid PostgreSQL connection URL, or [`check_url`] refuses it. The URL is deliberately not included because it usually contains a password.
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
/// Returns [`ConnectError::InvalidUrl`] if the URL cannot be parsed or [`check_url`] refuses it, and [`ConnectError::Database`] if the database cannot be reached.
pub async fn connect(url: &str, max_connections: u32) -> Result<Pool, ConnectError> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(ACQUIRE_TIMEOUT)
        .connect_with(connect_options(url)?)
        .await
        .map_err(ConnectError::Database)
}

/// Parses `url` into connection options. URLs that [`check_url`] refuses never reach the driver, so it never logs their parameters.
fn connect_options(url: &str) -> Result<PgConnectOptions, ConnectError> {
    check_url(url).map_err(|_| ConnectError::InvalidUrl)?;
    Ok(PgConnectOptions::from_str(url)
        .map_err(|_| ConnectError::InvalidUrl)?
        .application_name("bayan-server")
        .disable_statement_logging())
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

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;
    use std::sync::{Arc, Mutex};

    use sqlx::postgres::PgConnectOptions;
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::{ConnectError, URL_PARAMETERS, UrlProblem, check_url, connect_options};

    /// The driver's log target for parameters it ignores. bayan-server's `logging::SUPPRESSED_TARGETS` must contain it.
    const PARSE_TARGET: &str = "sqlx_postgres::options::parse";

    /// A valid value for every name in `URL_PARAMETERS`, in the same order, then an `options[NAME]` parameter.
    const SAMPLES: [(&str, &str); 19] = [
        ("sslmode", "verify-full"),
        ("ssl-mode", "prefer"),
        ("sslrootcert", "/run/secrets/ca.pem"),
        ("ssl-root-cert", "/run/secrets/ca.pem"),
        ("ssl-ca", "/run/secrets/ca.pem"),
        ("sslcert", "/run/secrets/client.pem"),
        ("ssl-cert", "/run/secrets/client.pem"),
        ("sslkey", "/run/secrets/client.key"),
        ("ssl-key", "/run/secrets/client.key"),
        ("statement-cache-capacity", "100"),
        ("host", "db.internal"),
        ("hostaddr", "192.0.2.10"),
        ("port", "5433"),
        ("dbname", "bayan"),
        ("user", "bayan"),
        ("password", "x"),
        ("application_name", "x"),
        ("options", "-c%20search_path%3Dbayan"),
        ("options[search_path]", "bayan"),
    ];

    /// Records the target of every log event written on the current thread.
    #[derive(Clone, Default)]
    struct Targets(Arc<Mutex<Vec<String>>>);

    impl Subscriber for Targets {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _span: &Id, _values: &Record<'_>) {}
        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
        fn event(&self, event: &Event<'_>) {
            self.0
                .lock()
                .expect("targets lock")
                .push(event.metadata().target().to_owned());
        }
        fn enter(&self, _span: &Id) {}
        fn exit(&self, _span: &Id) {}
    }

    /// The targets of the log events written while `f` runs.
    fn log_targets(f: impl FnOnce()) -> Vec<String> {
        let targets = Targets::default();
        tracing::subscriber::with_default(targets.clone(), f);
        targets.0.lock().expect("targets lock").clone()
    }

    #[test]
    fn only_parameters_the_driver_reads_are_accepted() {
        let names: Vec<&str> = SAMPLES.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names[..URL_PARAMETERS.len()],
            URL_PARAMETERS,
            "one sample per name"
        );
        let all: Vec<String> = SAMPLES
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        for url in [
            format!(
                "postgres://bayan:pw@db.internal:5432/bayan?{}",
                all.join("&")
            ),
            "postgres://bayan:pw@db.internal/bayan".to_owned(),
            "postgresql://db.internal/bayan?".to_owned(),
            // Empty parameters are skipped, and a value may contain `=`.
            "postgres://db.internal/bayan?&&sslmode=verify-full&&options[a]=b=c&".to_owned(),
        ] {
            assert_eq!(check_url(&url), Ok(()), "{url}");
        }

        for (url, position) in [
            // libpq's parameter for the client key's passphrase, which this driver does not know: it would log the passphrase.
            ("postgres://db/bayan?sslpassword=Hunter2", 1),
            // A misspelling, a name in another letter case, and a percent-encoded name.
            (
                "postgres://db/bayan?sslmode=verify-full&sslmod=verify-full",
                2,
            ),
            ("postgres://db/bayan?SSLMODE=verify-full", 1),
            ("postgres://db/bayan?ssl%6Dode=verify-full", 1),
            // A bare value, a value without a name, and an unclosed `options[`.
            ("postgres://db/bayan?sslmode=verify-full&Hunter2", 2),
            ("postgres://db/bayan?=Hunter2", 1),
            ("postgres://db/bayan?options[search_path=bayan", 1),
            // Empty parameters do not count.
            ("postgres://db/bayan?&&port=5432&&&ssl_mode=require", 2),
        ] {
            assert_eq!(
                check_url(url),
                Err(UrlProblem::UnrecognizedParameter(position)),
                "{url}"
            );
        }

        for url in [
            "postgres://db/bayan#sslmode=verify-full",
            "postgres://db/bayan?sslmode=prefer#",
            "postgres://bayan:pa#ss@db/bayan",
        ] {
            assert_eq!(check_url(url), Err(UrlProblem::Fragment), "{url}");
        }
    }

    /// Guards `URL_PARAMETERS` against sqlx upgrades: the driver must read every listed parameter without logging it, and must still log the parameters it ignores under the target that bayan-server suppresses.
    #[test]
    fn url_parameters_match_the_driver() {
        for (name, value) in SAMPLES {
            let url = format!("postgres://db.internal/bayan?{name}={value}");
            let targets = log_targets(|| {
                if let Err(error) = PgConnectOptions::from_str(&url) {
                    panic!("the driver rejects {name}={value}: {error}");
                }
            });
            assert!(
                !targets.iter().any(|target| target == PARSE_TARGET),
                "the driver ignores the parameter {name}; remove it from URL_PARAMETERS"
            );
        }
        let targets = log_targets(|| {
            let _ = PgConnectOptions::from_str("postgres://db.internal/bayan?sslpassword=x");
        });
        assert!(
            targets.iter().any(|target| target == PARSE_TARGET),
            "the driver no longer logs ignored parameters under {PARSE_TARGET} (logged: {targets:?}); update this test and bayan-server's logging::SUPPRESSED_TARGETS"
        );
    }

    #[test]
    fn refused_urls_never_reach_the_driver() {
        let targets = log_targets(|| {
            assert!(matches!(
                connect_options("postgres://db.internal/bayan?sslpassword=Hunter2"),
                Err(ConnectError::InvalidUrl)
            ));
            assert!(connect_options("postgres://db.internal/bayan?sslmode=prefer").is_ok());
        });
        assert!(
            !targets.iter().any(|target| target == PARSE_TARGET),
            "{targets:?}"
        );
    }
}
