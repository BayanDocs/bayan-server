//! Typed server configuration.
//!
//! Settings come from, in increasing order of precedence: built-in defaults, an optional TOML file named by `BAYAN_CONFIG_FILE`, and `BAYAN_*` environment variables. Secrets can be read from files with the `*_FILE` convention (`BAYAN_DATABASE_URL_FILE`, or `database_url_file` in the TOML file) so they never appear in the process environment. Every setting is documented in `docs/configuration.md`.
//!
//! Validation is strict: unknown `BAYAN_*` variables and unknown TOML keys are errors, so a typo cannot silently fall back to a default. Error messages name the setting (and, for the TOML file, the line and key) but never repeat a configured value, a file's path or a file's contents: any of them may be a secret put in the wrong place.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bayan_db_postgres::UrlProblem;
use serde::Deserialize;

/// Prefix of every environment variable the server reads.
const ENV_PREFIX: &str = "BAYAN_";
/// The environment variable naming the optional TOML configuration file.
pub const CONFIG_FILE_VAR: &str = "BAYAN_CONFIG_FILE";
/// Upper bound on the size of the configuration file and of secret files, so a misconfigured path (for example a device file) cannot exhaust memory.
const MAX_FILE_BYTES: u64 = 64 * 1024;
/// File name of the SQLite database inside the data directory.
pub const SQLITE_FILE_NAME: &str = "bayan.sqlite3";

/// One setting: its environment variable, its TOML key, and what its TOML value must look like (used in error messages instead of the value itself).
struct Setting {
    env: &'static str,
    key: &'static str,
    expects: &'static str,
}

const LISTEN: Setting = Setting {
    env: "BAYAN_LISTEN",
    key: "listen",
    expects: "a quoted IP address and port, such as \"0.0.0.0:8080\"",
};
const DATA_DIR: Setting = Setting {
    env: "BAYAN_DATA_DIR",
    key: "data_dir",
    expects: "a quoted path",
};
const DATABASE_URL: Setting = Setting {
    env: "BAYAN_DATABASE_URL",
    key: "database_url",
    expects: "a quoted postgres:// URL",
};
const DATABASE_URL_FILE: Setting = Setting {
    env: "BAYAN_DATABASE_URL_FILE",
    key: "database_url_file",
    expects: "a quoted path",
};
const DATABASE_MAX_CONNECTIONS: Setting = Setting {
    env: "BAYAN_DATABASE_MAX_CONNECTIONS",
    key: "database_max_connections",
    expects: "a whole number",
};
const WEB_DIR: Setting = Setting {
    env: "BAYAN_WEB_DIR",
    key: "web_dir",
    expects: "a quoted path",
};
const LOG_FORMAT: Setting = Setting {
    env: "BAYAN_LOG_FORMAT",
    key: "log_format",
    expects: "\"text\" or \"json\"",
};
const LOG_LEVEL: Setting = Setting {
    env: "BAYAN_LOG_LEVEL",
    key: "log_level",
    expects: "\"error\", \"warn\", \"info\", \"debug\" or \"trace\"",
};
const MAX_REQUEST_BODY_BYTES: Setting = Setting {
    env: "BAYAN_MAX_REQUEST_BODY_BYTES",
    key: "max_request_body_bytes",
    expects: "a whole number of bytes",
};
const REQUEST_TIMEOUT_SECS: Setting = Setting {
    env: "BAYAN_REQUEST_TIMEOUT_SECS",
    key: "request_timeout_secs",
    expects: "a whole number of seconds",
};
const HEADER_READ_TIMEOUT_SECS: Setting = Setting {
    env: "BAYAN_HEADER_READ_TIMEOUT_SECS",
    key: "header_read_timeout_secs",
    expects: "a whole number of seconds",
};
const SHUTDOWN_GRACE_SECS: Setting = Setting {
    env: "BAYAN_SHUTDOWN_GRACE_SECS",
    key: "shutdown_grace_secs",
    expects: "a whole number of seconds",
};

/// Every setting, used to reject unknown `BAYAN_*` variables and to name TOML keys in errors.
const SETTINGS: [&Setting; 12] = [
    &LISTEN,
    &DATA_DIR,
    &DATABASE_URL,
    &DATABASE_URL_FILE,
    &DATABASE_MAX_CONNECTIONS,
    &WEB_DIR,
    &LOG_FORMAT,
    &LOG_LEVEL,
    &MAX_REQUEST_BODY_BYTES,
    &REQUEST_TIMEOUT_SECS,
    &HEADER_READ_TIMEOUT_SECS,
    &SHUTDOWN_GRACE_SECS,
];

/// The validated server configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Address and port the HTTP server listens on.
    pub listen: SocketAddr,
    /// Directory for the server's persistent data (the SQLite database by default).
    pub data_dir: PathBuf,
    /// Which metadata database to use.
    pub database: DatabaseConfig,
    /// Maximum number of open database connections.
    pub database_max_connections: u32,
    /// Directory of static files to serve (the built web app), if any.
    pub web_dir: Option<PathBuf>,
    /// Log output format.
    pub log_format: LogFormat,
    /// Most verbose log level that is written.
    pub log_level: LogLevel,
    /// Largest accepted request body, in bytes.
    pub max_request_body_bytes: usize,
    /// Time after which a request whose head has arrived but which is not yet answered gets `408 Request Timeout`.
    pub request_timeout: Duration,
    /// Time a connection gets to send a complete request head (request line and headers). It also limits how long a connection may sit idle, before its first request or between keep-alive requests. Connections that exceed it are closed.
    pub header_read_timeout: Duration,
    /// Time in-flight requests get to finish after a shutdown signal.
    pub shutdown_grace: Duration,
}

/// Which metadata database the server uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseConfig {
    /// The embedded SQLite database at this path (the default).
    Sqlite {
        /// Path of the database file.
        path: PathBuf,
    },
    /// A PostgreSQL server.
    Postgres {
        /// Connection URL; it usually contains a password.
        url: Secret,
    },
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable lines.
    Text,
    /// One JSON object per line, for log collectors.
    Json,
}

/// Most verbose log level that is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Errors only.
    Error,
    /// Warnings and errors.
    Warn,
    /// Informational messages (the default).
    Info,
    /// Debugging details.
    Debug,
    /// Everything.
    Trace,
}

/// A secret value that is never printed: its `Debug` output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a secret value.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Returns the secret value. Never log or display it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// Why the configuration is invalid. Messages name the setting but never contain secret values.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    /// An environment variable starting with `BAYAN_` is not a known setting.
    #[error(
        "unknown environment variable {0} (see docs/configuration.md for the supported settings)"
    )]
    UnknownVariable(String),
    /// An environment variable is not valid Unicode.
    #[error("environment variable {0} is not valid Unicode")]
    NotUnicode(String),
    /// A setting has a value that cannot be parsed or is out of range.
    #[error("invalid value for {setting}: {reason}")]
    Invalid {
        /// The environment variable or TOML key.
        setting: String,
        /// What is wrong, without the value if it may be secret.
        reason: String,
    },
    /// A setting was given both directly and as a file.
    #[error("{0} and {1} are both set; set only one")]
    Conflict(&'static str, &'static str),
    /// A file named by the configuration cannot be read. The path is not repeated: a secret may have been put where the path belongs.
    #[error("cannot read the file named by {setting}: {reason}")]
    File {
        /// The setting that names the file.
        setting: String,
        /// Why it cannot be read.
        reason: String,
    },
    /// The TOML configuration file is invalid. Only the line number and a known key are reported, never the file's text: the parser's own messages quote values, which may be secrets.
    #[error("invalid configuration file (BAYAN_CONFIG_FILE){}: {problem}", toml_location(*.line, *.key))]
    Toml {
        /// The line of the problem, if the parser reported one.
        line: Option<usize>,
        /// The setting's TOML key on that line, if it is a known one.
        key: Option<&'static str>,
        /// What is wrong, built only from fixed text.
        problem: String,
    },
}

fn toml_location(line: Option<usize>, key: Option<&str>) -> String {
    match (line, key) {
        (Some(line), Some(key)) => format!(" at line {line}, key `{key}`"),
        (Some(line), None) => format!(" at line {line}"),
        (None, _) => String::new(),
    }
}

/// The optional TOML file. Every key is optional; unknown keys are rejected.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    listen: Option<String>,
    data_dir: Option<PathBuf>,
    database_url: Option<String>,
    database_url_file: Option<PathBuf>,
    database_max_connections: Option<u32>,
    web_dir: Option<PathBuf>,
    log_format: Option<String>,
    log_level: Option<String>,
    max_request_body_bytes: Option<u64>,
    request_timeout_secs: Option<u64>,
    header_read_timeout_secs: Option<u64>,
    shutdown_grace_secs: Option<u64>,
}

impl Config {
    /// Loads the configuration from the process environment and the optional configuration file.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first invalid setting.
    pub fn from_process_env() -> Result<Self, ConfigError> {
        let mut vars = BTreeMap::new();
        for (name, value) in std::env::vars_os() {
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(ENV_PREFIX) {
                continue;
            }
            let value = value
                .into_string()
                .map_err(|_| ConfigError::NotUnicode(name.to_owned()))?;
            vars.insert(name.to_owned(), value);
        }
        Self::from_vars(&vars)
    }

    /// Loads the configuration from the given environment variables (only `BAYAN_*` names are considered) and the configuration file they name.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first invalid setting.
    pub fn from_vars(vars: &BTreeMap<String, String>) -> Result<Self, ConfigError> {
        for name in vars.keys().filter(|name| name.starts_with(ENV_PREFIX)) {
            if name != CONFIG_FILE_VAR && !SETTINGS.iter().any(|setting| setting.env == name) {
                return Err(ConfigError::UnknownVariable(name.clone()));
            }
        }
        let file = match vars.get(CONFIG_FILE_VAR) {
            Some(path) => read_config_file(Path::new(path))?,
            None => FileConfig::default(),
        };
        let env = |setting: &Setting| env_value(vars, setting);

        let listen = match (env(&LISTEN), file.listen.as_deref()) {
            (Some(value), _) => parse_listen(LISTEN.env, value)?,
            (None, Some(value)) => parse_listen(LISTEN.key, value)?,
            (None, None) => SocketAddr::from(([127, 0, 0, 1], 8080)),
        };
        let data_dir = env(&DATA_DIR)
            .map(PathBuf::from)
            .or_else(|| file.data_dir.clone())
            .unwrap_or_else(|| PathBuf::from("data"));
        if data_dir.as_os_str().is_empty() {
            return Err(invalid(DATA_DIR.env, "must not be empty"));
        }
        let database = database_config(vars, &file, &data_dir)?;
        let database_max_connections = match env(&DATABASE_MAX_CONNECTIONS) {
            Some(value) => parse_number(DATABASE_MAX_CONNECTIONS.env, value)?,
            None => file.database_max_connections.map_or(10, u64::from),
        };
        let database_max_connections = in_range(
            DATABASE_MAX_CONNECTIONS.env,
            database_max_connections,
            1,
            1000,
        )?;
        let web_dir = env(&WEB_DIR)
            .map(PathBuf::from)
            .or_else(|| file.web_dir.clone());
        if web_dir
            .as_ref()
            .is_some_and(|dir| dir.as_os_str().is_empty())
        {
            return Err(invalid(
                WEB_DIR.env,
                "must not be empty; leave it unset to serve no static files",
            ));
        }
        let log_format = match env(&LOG_FORMAT).or(file.log_format.as_deref()) {
            None | Some("text") => LogFormat::Text,
            Some("json") => LogFormat::Json,
            Some(_) => return Err(invalid(LOG_FORMAT.env, "expected \"text\" or \"json\"")),
        };
        let log_level = match env(&LOG_LEVEL).or(file.log_level.as_deref()) {
            Some("error") => LogLevel::Error,
            Some("warn") => LogLevel::Warn,
            None | Some("info") => LogLevel::Info,
            Some("debug") => LogLevel::Debug,
            Some("trace") => LogLevel::Trace,
            Some(_) => {
                return Err(invalid(
                    LOG_LEVEL.env,
                    "expected error, warn, info, debug or trace",
                ));
            }
        };
        let max_request_body_bytes = number_setting(
            &MAX_REQUEST_BODY_BYTES,
            env(&MAX_REQUEST_BODY_BYTES),
            file.max_request_body_bytes,
        )?
        .unwrap_or(1024 * 1024);
        let max_request_body_bytes = in_range(
            MAX_REQUEST_BODY_BYTES.env,
            max_request_body_bytes,
            1,
            1024 * 1024 * 1024,
        )?;
        let request_timeout = number_setting(
            &REQUEST_TIMEOUT_SECS,
            env(&REQUEST_TIMEOUT_SECS),
            file.request_timeout_secs,
        )?
        .unwrap_or(30);
        let request_timeout = Duration::from_secs(in_range(
            REQUEST_TIMEOUT_SECS.env,
            request_timeout,
            1,
            3600,
        )?);
        let header_read_timeout = number_setting(
            &HEADER_READ_TIMEOUT_SECS,
            env(&HEADER_READ_TIMEOUT_SECS),
            file.header_read_timeout_secs,
        )?
        .unwrap_or(10);
        let header_read_timeout = Duration::from_secs(in_range(
            HEADER_READ_TIMEOUT_SECS.env,
            header_read_timeout,
            1,
            300,
        )?);
        // Below Docker's default stop timeout (10 seconds), so `docker stop` ends with a clean exit; see docs/deployment.md.
        let shutdown_grace = number_setting(
            &SHUTDOWN_GRACE_SECS,
            env(&SHUTDOWN_GRACE_SECS),
            file.shutdown_grace_secs,
        )?
        .unwrap_or(5);
        let shutdown_grace =
            Duration::from_secs(in_range(SHUTDOWN_GRACE_SECS.env, shutdown_grace, 0, 3600)?);

        Ok(Self {
            listen,
            data_dir,
            database,
            database_max_connections,
            web_dir,
            log_format,
            log_level,
            max_request_body_bytes,
            request_timeout,
            header_read_timeout,
            shutdown_grace,
        })
    }
}

fn database_config(
    vars: &BTreeMap<String, String>,
    file: &FileConfig,
    data_dir: &Path,
) -> Result<DatabaseConfig, ConfigError> {
    let url = match (vars.get(DATABASE_URL.env), vars.get(DATABASE_URL_FILE.env)) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::Conflict(
                DATABASE_URL.env,
                DATABASE_URL_FILE.env,
            ));
        }
        (Some(url), None) => Some((DATABASE_URL.env, url.clone())),
        (None, Some(path)) => Some((
            DATABASE_URL_FILE.env,
            read_secret_file(DATABASE_URL_FILE.env, Path::new(path))?,
        )),
        (None, None) => match (&file.database_url, &file.database_url_file) {
            (Some(_), Some(_)) => {
                return Err(ConfigError::Conflict(
                    DATABASE_URL.key,
                    DATABASE_URL_FILE.key,
                ));
            }
            (Some(url), None) => Some((DATABASE_URL.key, url.clone())),
            (None, Some(path)) => Some((
                DATABASE_URL_FILE.key,
                read_secret_file(DATABASE_URL_FILE.key, path)?,
            )),
            (None, None) => None,
        },
    };
    match url {
        None => Ok(DatabaseConfig::Sqlite {
            path: data_dir.join(SQLITE_FILE_NAME),
        }),
        Some((setting, url)) => {
            // Only the scheme and the query parameters' names are inspected, and nothing from the URL is reported: any part of it may hold a password.
            if !(url.starts_with("postgres://") || url.starts_with("postgresql://")) {
                return Err(invalid(
                    setting,
                    "expected a postgres:// or postgresql:// URL; leave it unset to use the built-in SQLite database",
                ));
            }
            // The driver would silently ignore these parts of the URL, and log an ignored parameter's value.
            match bayan_db_postgres::check_url(&url) {
                Ok(()) => Ok(DatabaseConfig::Postgres {
                    url: Secret::new(url),
                }),
                Err(UrlProblem::UnrecognizedParameter(position)) => Err(invalid(
                    setting,
                    &format!(
                        "query parameter {position} is not one the PostgreSQL driver reads, so it would be ignored; parameter names are case-sensitive, and docs/configuration.md lists the supported ones"
                    ),
                )),
                Err(UrlProblem::Fragment) => Err(invalid(
                    setting,
                    "the URL contains `#`, which ends the part the PostgreSQL driver reads; write a `#` in a password as %23",
                )),
            }
        }
    }
}

fn read_config_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let text = read_limited(CONFIG_FILE_VAR, path)?;
    toml::from_str(&text).map_err(|error| toml_error(&text, &error))
}

/// Describes a TOML error by line number, known key and a fixed description. The parser's message is only classified, never shown, because it quotes the offending value.
fn toml_error(text: &str, error: &toml::de::Error) -> ConfigError {
    let line = error.span().map(|span| {
        let start = span.start.min(text.len());
        text.as_bytes()[..start]
            .iter()
            .filter(|&&byte| byte == b'\n')
            .count()
            + 1
    });
    let setting = line.and_then(|line| {
        let key = text.lines().nth(line - 1)?.split_once('=')?.0.trim();
        SETTINGS.iter().copied().find(|setting| setting.key == key)
    });
    let message = error.message();
    let problem = if message.starts_with("unknown field") {
        "unknown key (see docs/configuration.md for the supported settings)".to_owned()
    } else if message.contains("duplicate") {
        "the key is set more than once".to_owned()
    } else if message.starts_with("invalid type")
        || message.starts_with("invalid value")
        || message.starts_with("invalid length")
    {
        match setting {
            Some(setting) => format!("wrong kind of value; expected {}", setting.expects),
            None => "wrong kind of value".to_owned(),
        }
    } else {
        "not valid TOML; every line must be `key = value`, with text values in quotes".to_owned()
    };
    ConfigError::Toml {
        line,
        key: setting.map(|setting| setting.key),
        problem,
    }
}

/// Reads a secret from a file, dropping one trailing line ending (as left by editors and `echo`).
fn read_secret_file(setting: &str, path: &Path) -> Result<String, ConfigError> {
    let mut text = read_limited(setting, path)?;
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    if text.is_empty() {
        return Err(ConfigError::File {
            setting: setting.to_owned(),
            reason: "the file is empty".to_owned(),
        });
    }
    Ok(text)
}

fn read_limited(setting: &str, path: &Path) -> Result<String, ConfigError> {
    use std::io::Read as _;
    let file_error = |reason: String| ConfigError::File {
        setting: setting.to_owned(),
        reason,
    };
    let file = std::fs::File::open(path).map_err(|error| file_error(error.kind().to_string()))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| file_error(error.kind().to_string()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(file_error(format!("larger than {MAX_FILE_BYTES} bytes")));
    }
    String::from_utf8(bytes).map_err(|_| file_error("not valid UTF-8".to_owned()))
}

fn parse_listen(setting: &str, value: &str) -> Result<SocketAddr, ConfigError> {
    value.parse().map_err(|_| {
        invalid(
            setting,
            "expected an IP address and port, such as 127.0.0.1:8080 or [::]:8080",
        )
    })
}

fn env_value<'vars>(
    vars: &'vars BTreeMap<String, String>,
    setting: &Setting,
) -> Option<&'vars str> {
    vars.get(setting.env).map(String::as_str)
}

/// A numeric setting: the environment value if present (parsed), else the file value.
fn number_setting(
    setting: &Setting,
    env: Option<&str>,
    file: Option<u64>,
) -> Result<Option<u64>, ConfigError> {
    match env {
        Some(value) => parse_number(setting.env, value).map(Some),
        None => Ok(file),
    }
}

fn parse_number(setting: &str, value: &str) -> Result<u64, ConfigError> {
    value
        .parse()
        .map_err(|_| invalid(setting, "expected a whole number"))
}

fn in_range<T: TryFrom<u64>>(
    setting: &str,
    value: u64,
    min: u64,
    max: u64,
) -> Result<T, ConfigError> {
    if value < min || value > max {
        return Err(invalid(
            setting,
            &format!("must be between {min} and {max}"),
        ));
    }
    T::try_from(value).map_err(|_| invalid(setting, "too large for this platform"))
}

fn invalid(setting: &str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        setting: setting.to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bayan-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join(name);
        std::fs::write(&path, contents).expect("write temp file");
        path
    }

    #[test]
    fn defaults_use_sqlite_in_the_data_directory() {
        let config = Config::from_vars(&BTreeMap::new()).expect("defaults are valid");
        assert_eq!(config.listen, SocketAddr::from(([127, 0, 0, 1], 8080)));
        assert_eq!(
            config.database,
            DatabaseConfig::Sqlite {
                path: PathBuf::from("data").join(SQLITE_FILE_NAME)
            }
        );
        assert_eq!(config.log_format, LogFormat::Text);
        assert_eq!(config.log_level, LogLevel::Info);
        assert_eq!(config.max_request_body_bytes, 1024 * 1024);
        assert_eq!(config.request_timeout, Duration::from_secs(30));
        assert_eq!(config.header_read_timeout, Duration::from_secs(10));
        assert_eq!(config.shutdown_grace, Duration::from_secs(5));
        assert_eq!(config.web_dir, None);
    }

    #[test]
    fn environment_overrides_file_which_overrides_defaults() {
        let file = temp_file(
            "precedence.toml",
            "listen = \"0.0.0.0:9000\"\nlog_format = \"json\"\nlog_level = \"debug\"\nrequest_timeout_secs = 5\n",
        );
        let config = Config::from_vars(&vars(&[
            ("BAYAN_CONFIG_FILE", file.to_str().expect("utf-8 path")),
            ("BAYAN_LOG_LEVEL", "warn"),
        ]))
        .expect("valid configuration");
        assert_eq!(config.listen, SocketAddr::from(([0, 0, 0, 0], 9000)));
        assert_eq!(config.log_format, LogFormat::Json);
        assert_eq!(config.log_level, LogLevel::Warn);
        assert_eq!(config.request_timeout, Duration::from_secs(5));
    }

    #[test]
    fn unknown_variables_and_keys_are_rejected() {
        assert_eq!(
            Config::from_vars(&vars(&[("BAYAN_LISTN", "0.0.0.0:80")])),
            Err(ConfigError::UnknownVariable("BAYAN_LISTN".to_owned()))
        );
        let error = file_error(
            "unknown.toml",
            "log_level = \"info\"\nlisen = \"0.0.0.0:80\"\n",
        );
        assert_eq!(
            error.to_string(),
            "invalid configuration file (BAYAN_CONFIG_FILE) at line 2: unknown key (see docs/configuration.md for the supported settings)"
        );
    }

    /// The error for a configuration file with `contents`.
    fn file_error(name: &str, contents: &str) -> ConfigError {
        let file = temp_file(name, contents);
        Config::from_vars(&vars(&[(
            "BAYAN_CONFIG_FILE",
            file.to_str().expect("utf-8 path"),
        )]))
        .expect_err("the configuration file is invalid")
    }

    #[test]
    fn header_read_timeout_is_validated() {
        let config = Config::from_vars(&vars(&[("BAYAN_HEADER_READ_TIMEOUT_SECS", "3")]))
            .expect("valid configuration");
        assert_eq!(config.header_read_timeout, Duration::from_secs(3));
        let error = Config::from_vars(&vars(&[("BAYAN_HEADER_READ_TIMEOUT_SECS", "0")]))
            .expect_err("0 is out of range");
        assert_eq!(
            error.to_string(),
            "invalid value for BAYAN_HEADER_READ_TIMEOUT_SECS: must be between 1 and 300"
        );
    }

    /// Regression test (SRV-001 review): a secret put where a file path or another value belongs must not reach the error message, which ends up in logs.
    #[test]
    fn misplaced_secrets_never_appear_in_errors() {
        const SECRET: &str = "hunter2";
        let url = "postgres://bayan:hunter2@db/bayan";
        let mut errors = vec![
            // The URL itself given as the secret file's path.
            Config::from_vars(&vars(&[("BAYAN_DATABASE_URL_FILE", url)]))
                .expect_err("no such file"),
            Config::from_vars(&vars(&[("BAYAN_CONFIG_FILE", url)])).expect_err("no such file"),
        ];
        for (name, contents) in [
            // A quoted URL under a numeric key: the parser's message would quote it.
            (
                "wrong-type.toml",
                "database_max_connections = \"postgres://bayan:hunter2@db/x\"\n",
            ),
            // An unquoted URL: a TOML syntax error.
            (
                "unquoted.toml",
                "database_url = postgres://bayan:hunter2@db/x\n",
            ),
            // A bare line, and a secret used as a key.
            ("bare.toml", "postgres://bayan:hunter2@db/x\n"),
            ("secret-key.toml", "hunter2 = \"x\"\n"),
            // A path under the URL key, given as an array.
            (
                "array.toml",
                "database_url_file = [\"/run/secrets/hunter2\"]\n",
            ),
            // The URL under a file key: read as a path that does not exist.
            (
                "url-as-path.toml",
                "database_url_file = \"postgres://bayan:hunter2@db/x\"\n",
            ),
        ] {
            errors.push(file_error(name, contents));
        }
        for error in &errors {
            let message = error.to_string();
            assert!(
                !message.contains(SECRET),
                "secret in error message: {message}"
            );
            assert!(
                !format!("{error:?}").contains(SECRET),
                "secret in debug output: {error:?}"
            );
        }
        // The messages still say where to look.
        assert_eq!(
            errors[0].to_string(),
            "cannot read the file named by BAYAN_DATABASE_URL_FILE: entity not found"
        );
        assert_eq!(
            errors[2].to_string(),
            "invalid configuration file (BAYAN_CONFIG_FILE) at line 1, key `database_max_connections`: wrong kind of value; expected a whole number"
        );
        assert!(errors[3].to_string().contains("at line 1"), "{}", errors[3]);
    }

    #[test]
    fn invalid_values_name_the_setting() {
        let error = Config::from_vars(&vars(&[("BAYAN_DATABASE_MAX_CONNECTIONS", "0")]))
            .expect_err("0 is out of range");
        assert_eq!(
            error.to_string(),
            "invalid value for BAYAN_DATABASE_MAX_CONNECTIONS: must be between 1 and 1000"
        );
        let error = Config::from_vars(&vars(&[("BAYAN_LISTEN", "localhost")]))
            .expect_err("needs an IP address");
        assert!(
            error
                .to_string()
                .starts_with("invalid value for BAYAN_LISTEN:"),
            "{error}"
        );
        let error =
            Config::from_vars(&vars(&[("BAYAN_LOG_FORMAT", "xml")])).expect_err("unknown format");
        assert!(
            error
                .to_string()
                .starts_with("invalid value for BAYAN_LOG_FORMAT:"),
            "{error}"
        );
    }

    #[test]
    fn database_url_is_read_from_a_secret_file() {
        let file = temp_file("database-url", "postgres://bayan:hunter2@db/bayan\n");
        let config = Config::from_vars(&vars(&[(
            "BAYAN_DATABASE_URL_FILE",
            file.to_str().expect("utf-8 path"),
        )]))
        .expect("valid configuration");
        assert_eq!(
            config.database,
            DatabaseConfig::Postgres {
                url: Secret::new("postgres://bayan:hunter2@db/bayan".to_owned())
            }
        );
    }

    #[test]
    fn postgres_url_parameters_the_driver_reads_are_accepted() {
        let url = "postgres://bayan@db/bayan?sslmode=verify-full&sslrootcert=/run/secrets/ca.pem&statement-cache-capacity=0&options[search_path]=bayan";
        let config =
            Config::from_vars(&vars(&[("BAYAN_DATABASE_URL", url)])).expect("valid configuration");
        assert_eq!(
            config.database,
            DatabaseConfig::Postgres {
                url: Secret::new(url.to_owned())
            }
        );
    }

    /// Regression test (SRV-001 review): the PostgreSQL driver silently ignores URL query parameters it does not read and logs their values, so such URLs are refused at startup, from every source, with errors that repeat nothing from the URL.
    #[test]
    fn postgres_url_parameters_the_driver_ignores_are_refused() {
        const SECRET: &str = "Hunter2-Key-Passphrase";
        let url = format!("postgres://bayan@db/bayan?sslmode=verify-full&sslpassword={SECRET}");
        let url_file = temp_file("url-with-unknown-parameter", &url);
        let config_file = temp_file(
            "url-with-unknown-parameter.toml",
            &format!("database_url = \"{url}\"\n"),
        );
        let fragment = format!("postgres://bayan:{SECRET}@db/bayan#sslmode=verify-full");
        let errors = [
            Config::from_vars(&vars(&[("BAYAN_DATABASE_URL", &url)])),
            Config::from_vars(&vars(&[(
                "BAYAN_DATABASE_URL_FILE",
                url_file.to_str().expect("utf-8 path"),
            )])),
            Config::from_vars(&vars(&[(
                "BAYAN_CONFIG_FILE",
                config_file.to_str().expect("utf-8 path"),
            )])),
            // A misspelled `sslmode` would otherwise leave the connection weaker than configured, without a word.
            Config::from_vars(&vars(&[(
                "BAYAN_DATABASE_URL",
                "postgres://bayan@db/bayan?sslmod=verify-full",
            )])),
            Config::from_vars(&vars(&[("BAYAN_DATABASE_URL", &fragment)])),
        ]
        .map(|result| result.expect_err("the URL is refused"));
        for error in &errors {
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(
                    !text.contains(SECRET)
                        && !text.contains("sslpassword")
                        && !text.contains("sslmod="),
                    "part of the URL in the error: {text}"
                );
            }
        }
        assert_eq!(
            errors[0].to_string(),
            "invalid value for BAYAN_DATABASE_URL: query parameter 2 is not one the PostgreSQL driver reads, so it would be ignored; parameter names are case-sensitive, and docs/configuration.md lists the supported ones"
        );
        for (error, setting) in errors[1..3]
            .iter()
            .zip(["BAYAN_DATABASE_URL_FILE", "database_url"])
        {
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("invalid value for {setting}: query parameter 2 ")),
                "{error}"
            );
        }
        assert!(
            errors[3].to_string().contains("query parameter 1 "),
            "{}",
            errors[3]
        );
        assert_eq!(
            errors[4].to_string(),
            "invalid value for BAYAN_DATABASE_URL: the URL contains `#`, which ends the part the PostgreSQL driver reads; write a `#` in a password as %23"
        );
    }

    #[test]
    fn secrets_never_appear_in_debug_output_or_errors() {
        let config = Config::from_vars(&vars(&[(
            "BAYAN_DATABASE_URL",
            "postgres://bayan:hunter2@db/bayan",
        )]))
        .expect("valid configuration");
        assert!(!format!("{config:?}").contains("hunter2"));

        let error = Config::from_vars(&vars(&[(
            "BAYAN_DATABASE_URL",
            "mysql://bayan:hunter2@db/bayan",
        )]))
        .expect_err("only PostgreSQL URLs are accepted");
        assert!(!error.to_string().contains("hunter2"), "{error}");
        assert!(error.to_string().contains("BAYAN_DATABASE_URL"), "{error}");
    }

    #[test]
    fn database_url_and_file_conflict() {
        let file = temp_file("conflict-url", "postgres://db/bayan");
        let error = Config::from_vars(&vars(&[
            ("BAYAN_DATABASE_URL", "postgres://db/bayan"),
            (
                "BAYAN_DATABASE_URL_FILE",
                file.to_str().expect("utf-8 path"),
            ),
        ]))
        .expect_err("both set");
        assert_eq!(
            error,
            ConfigError::Conflict("BAYAN_DATABASE_URL", "BAYAN_DATABASE_URL_FILE")
        );
    }

    #[test]
    fn missing_and_empty_secret_files_are_errors() {
        let error = Config::from_vars(&vars(&[(
            "BAYAN_DATABASE_URL_FILE",
            "/nonexistent/bayan-secret",
        )]))
        .expect_err("missing file");
        assert!(matches!(error, ConfigError::File { .. }), "{error:?}");
        let empty = temp_file("empty-secret", "\n");
        let error = Config::from_vars(&vars(&[(
            "BAYAN_DATABASE_URL_FILE",
            empty.to_str().expect("utf-8 path"),
        )]))
        .expect_err("empty file");
        assert!(error.to_string().contains("the file is empty"), "{error}");
    }
}
