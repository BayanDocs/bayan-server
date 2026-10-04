//! Structured logging with a content-free policy.
//!
//! The server logs operational facts only. It never logs request or response bodies, header values (in particular `Authorization` and `Cookie`), query strings, raw request paths, document content, document titles, file names or user identifiers (ADR-0015 §7, threat T20). Request logs name the matched route template (for example `/readyz`), never the path the client sent. `tests/logging.rs` proves this policy by capturing every log line at the most verbose level while sending requests full of canary values.

use std::io;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::config::{LogFormat, LogLevel};

impl From<LogLevel> for LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Error => Self::ERROR,
            LogLevel::Warn => Self::WARN,
            LogLevel::Info => Self::INFO,
            LogLevel::Debug => Self::DEBUG,
            LogLevel::Trace => Self::TRACE,
        }
    }
}

/// Builds the log subscriber for `format` and `level`, writing to `writer`.
///
/// Colors are disabled so log files never contain terminal escape codes.
pub fn subscriber<W>(
    format: LogFormat,
    level: LogLevel,
    writer: W,
) -> Box<dyn tracing::Subscriber + Send + Sync>
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    let builder = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(LevelFilter::from(level))
        .with_ansi(false)
        .with_target(true);
    match format {
        LogFormat::Text => Box::new(builder.finish()),
        LogFormat::Json => Box::new(
            builder
                .json()
                .flatten_event(true)
                .with_current_span(false)
                .finish(),
        ),
    }
}

/// Installs the process-wide log subscriber, writing to standard error.
///
/// # Errors
///
/// Returns an error if a subscriber has already been installed.
pub fn init(
    format: LogFormat,
    level: LogLevel,
) -> Result<(), tracing::subscriber::SetGlobalDefaultError> {
    tracing::subscriber::set_global_default(subscriber(format, level, io::stderr))
}
