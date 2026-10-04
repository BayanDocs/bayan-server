//! The `bayan-server` executable.
//!
//! Usage: `bayan-server` (serve), `bayan-server healthcheck` (exit 0 if the local server is healthy; used by the container health check) or `bayan-server version`. Configuration comes from `BAYAN_*` environment variables and an optional TOML file; see `docs/configuration.md`.

use std::process::ExitCode;

use bayan_server::config::Config;
use bayan_server::version::VersionInfo;

const USAGE: &str = "usage: bayan-server [serve | healthcheck | version]\nConfiguration: BAYAN_* environment variables and an optional TOML file (BAYAN_CONFIG_FILE); see docs/configuration.md.";

#[expect(
    clippy::print_stderr,
    clippy::print_stdout,
    reason = "the command-line interface reports to the terminal before logging is set up"
)]
fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    if args.next().is_some() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let command = match command.as_ref().map(|arg| arg.to_str()) {
        None => "serve",
        Some(Some(command)) => command,
        Some(None) => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match command {
        "version" | "--version" => {
            let info = VersionInfo::current();
            println!("{} {} ({})", info.name, info.version, info.commit);
            return ExitCode::SUCCESS;
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        "serve" | "healthcheck" => {}
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }

    let config = match Config::from_process_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("bayan-server: configuration error: {error}");
            return ExitCode::from(78); // EX_CONFIG from sysexits.h
        }
    };
    if command == "healthcheck" {
        return match bayan_server::healthcheck::check(config.listen) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("bayan-server: health check failed: {error}");
                ExitCode::FAILURE
            }
        };
    }

    if let Err(error) = bayan_server::logging::init(config.log_format, config.log_level) {
        eprintln!("bayan-server: cannot set up logging: {error}");
        return ExitCode::FAILURE;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(error = %error, "cannot start the async runtime");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        let info = VersionInfo::current();
        tracing::info!(
            version = info.version,
            commit = info.commit,
            "starting bayan-server"
        );
        let listener = match tokio::net::TcpListener::bind(config.listen).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!(address = %config.listen, error = %error, "cannot listen");
                return ExitCode::FAILURE;
            }
        };
        tracing::info!(address = %config.listen, "listening");
        match bayan_server::run(&config, listener, bayan_server::shutdown_signal()).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(error = %error, "server stopped with an error");
                ExitCode::FAILURE
            }
        }
    })
}
