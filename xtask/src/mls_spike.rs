//! Commands for the MLS spike of work package SRV-002 (`spikes/mls`, a Cargo workspace of its own; see its REPORT.md).
//!
//! - [`verify_steps`]: the spike's part of `cargo xtask verify` (format, Clippy, tests, documentation, cargo deny with the server's `deny.toml`).
//! - `mls-spike-wasm [node | chromium]`: the spike's tests in WebAssembly, in Node.js and in headless Chromium.
//! - `mls-spike-bench <native | node | chromium>`: the measurements of the report.
//! - `mls-spike-wasm-size`: the size of the WebAssembly a web client would download.
//!
//! The WebAssembly commands use the pinned tools that `scripts/dev-setup.sh` installs under `$BAYAN_TOOLS_DIR` (default `~/.local/share/bayandocs/server-tools`).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Result, cargo, step};

const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// The platforms cargo deny checks the spike's dependencies for. Checking each real build target, instead of every `cfg` at once, skips dependencies that only custom `cfg` flags switch on and that are never built: libcrux's Valgrind constant-time test hook (`crabgrind`, which compiles C code) and its proof-extraction macros, and wasm-bindgen-test's coverage runtime (`minicov`, also C).
const DENY_TARGETS: [&str; 4] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    WASM_TARGET,
];

/// Where the spike runs its WebAssembly tests.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Host {
    Node,
    /// On the page's main thread.
    Chromium,
    /// In a dedicated Web Worker, where the web app will run the engine and its MLS client (bayan-web never calls the engine from the main thread). The measurements run here: a computation that blocks the page's main thread for more than 30 seconds makes chromedriver lose contact with the page.
    ChromiumWorker,
}

impl Host {
    fn name(self) -> &'static str {
        match self {
            Host::Node => "Node.js",
            Host::Chromium => "headless Chromium",
            Host::ChromiumWorker => "headless Chromium, in a dedicated worker",
        }
    }
}

fn spike_manifest(root: &Path) -> PathBuf {
    root.join("spikes").join("mls").join("Cargo.toml")
}

/// A Cargo command for the spike's workspace.
fn spike_cargo(root: &Path, args: &[&str]) -> Command {
    let mut command = cargo(args);
    command.arg("--manifest-path").arg(spike_manifest(root));
    command
}

/// The spike's part of the verification gate, run by `cargo xtask verify` after the server's own steps.
pub fn verify_steps(root: &Path) -> Result {
    let manifest = spike_manifest(root);
    step(
        "MLS spike: format",
        cargo(&["fmt", "--all", "--check", "--manifest-path"]).arg(&manifest),
    )?;
    step(
        "MLS spike: lint",
        spike_cargo(
            root,
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
            ],
        )
        .args(["--", "-D", "warnings"]),
    )?;
    step(
        "MLS spike: test",
        &mut spike_cargo(root, &["test", "--workspace", "--all-features", "--locked"]),
    )?;
    step(
        "MLS spike: documentation",
        spike_cargo(
            root,
            &[
                "doc",
                "--workspace",
                "--all-features",
                "--no-deps",
                "--locked",
            ],
        )
        .env("RUSTDOCFLAGS", "-D warnings"),
    )?;
    let config = root.join("deny.toml");
    for target in DENY_TARGETS {
        step(
            &format!("MLS spike: dependency policy (cargo deny) for {target}"),
            cargo(&["deny", "--manifest-path"])
                .arg(&manifest)
                .arg("--config")
                .arg(&config)
                .args(["--locked", "--all-features", "--target", target, "check"]),
        )?;
    }
    Ok(())
}

/// The directory `scripts/dev-setup.sh` installs the WebAssembly tools into.
fn tools_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("BAYAN_TOOLS_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let data = match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").ok_or("set HOME or BAYAN_TOOLS_DIR")?)
            .join(".local")
            .join("share"),
    };
    Ok(data.join("bayandocs").join("server-tools"))
}

/// One of the pinned tools, or an error saying how to install it.
fn tool(tools: &Path, name: &str, program: &str) -> Result<PathBuf> {
    let path = tools.join(name).join(program);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{} is missing; install it with `scripts/dev-setup.sh {name}`",
            path.display()
        ))
    }
}

/// Configures a `cargo test --target wasm32-unknown-unknown` command to run in `host` with the pinned tools.
fn configure_host(command: &mut Command, root: &Path, host: Host) -> Result {
    let tools = tools_dir()?;
    command
        .env(
            "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER",
            tool(&tools, "wasm-bindgen", "wasm-bindgen-test-runner")?,
        )
        // The benchmarks run for minutes; the tests take seconds.
        .env("WASM_BINDGEN_TEST_TIMEOUT", "3600");
    match host {
        Host::Node => {
            let node = tool(&tools, "node", "node")?;
            let node_dir = node.parent().ok_or("node has no directory")?.to_path_buf();
            let mut path = vec![node_dir];
            path.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            command.env(
                "PATH",
                std::env::join_paths(path).map_err(|error| error.to_string())?,
            );
        }
        Host::Chromium | Host::ChromiumWorker => {
            let browser = tool(&tools, "chrome-headless-shell", "chrome-headless-shell")?;
            let browser = browser
                .to_str()
                .ok_or("the browser's path is not valid UTF-8")?;
            if browser.contains(['"', '\\']) {
                return Err("the browser's path must not contain quotes or backslashes".to_owned());
            }
            // wasm-bindgen-test-runner merges these WebDriver capabilities into its own (it adds headless mode itself).
            let config_dir = root
                .join("spikes")
                .join("mls")
                .join("target")
                .join("wasm-test");
            std::fs::create_dir_all(&config_dir)
                .map_err(|error| format!("cannot create {}: {error}", config_dir.display()))?;
            let config = config_dir.join("webdriver.json");
            std::fs::write(
                &config,
                format!("{{\"goog:chromeOptions\": {{\"binary\": \"{browser}\"}}}}\n"),
            )
            .map_err(|error| format!("cannot write {}: {error}", config.display()))?;
            // The runner checks WASM_BINDGEN_USE_BROWSER before WASM_BINDGEN_USE_DEDICATED_WORKER, so exactly one is set.
            let mode = if host == Host::Chromium {
                "WASM_BINDGEN_USE_BROWSER"
            } else {
                "WASM_BINDGEN_USE_DEDICATED_WORKER"
            };
            command
                .env(mode, "1")
                .env(
                    "CHROMEDRIVER",
                    tool(&tools, "chromedriver", "chromedriver")?,
                )
                .env("WASM_BINDGEN_TEST_WEBDRIVER_JSON", config);
        }
    }
    Ok(())
}

fn hosts(args: &[&str]) -> Result<Vec<Host>> {
    match args {
        [] => Ok(vec![Host::Node, Host::Chromium]),
        ["node"] => Ok(vec![Host::Node]),
        ["chromium"] => Ok(vec![Host::Chromium]),
        _ => Err("usage: cargo xtask mls-spike-wasm [node | chromium]".to_owned()),
    }
}

/// `cargo xtask mls-spike-wasm [node | chromium]`: every spike test, compiled to WebAssembly and run in Node.js, in headless Chromium, or both.
pub fn wasm_tests(root: &Path, args: &[&str]) -> Result {
    for host in hosts(args)? {
        let mut command = spike_cargo(
            root,
            &[
                "test",
                "--workspace",
                "--all-features",
                "--locked",
                "--release",
                "--target",
                WASM_TARGET,
            ],
        );
        configure_host(&mut command, root, host)?;
        step(
            &format!("MLS spike: tests in {}", host.name()),
            &mut command,
        )?;
    }
    Ok(())
}

/// `cargo xtask mls-spike-bench <native | node | chromium>`: the measurements, natively or in WebAssembly (in Chromium, in a dedicated worker).
pub fn bench(root: &Path, args: &[&str]) -> Result {
    let host = match args {
        ["native"] => {
            return step(
                "MLS spike: measurements, native",
                &mut spike_cargo(
                    root,
                    &[
                        "run",
                        "--release",
                        "--locked",
                        "--all-features",
                        "--package",
                        "bayan-mls-spike",
                        "--bin",
                        "mls-bench",
                    ],
                ),
            );
        }
        ["node"] => Host::Node,
        ["chromium"] => Host::ChromiumWorker,
        _ => {
            return Err("usage: cargo xtask mls-spike-bench <native | node | chromium>".to_owned());
        }
    };
    let mut command = spike_cargo(
        root,
        &[
            "test",
            "--release",
            "--locked",
            "--all-features",
            "--package",
            "bayan-mls-spike",
            "--target",
            WASM_TARGET,
            "--test",
            "bench",
        ],
    );
    command.args(["--", "--include-ignored", "--nocapture", "bench_full"]);
    configure_host(&mut command, root, host)?;
    step(
        &format!("MLS spike: measurements in {}", host.name()),
        &mut command,
    )
}

/// `cargo xtask mls-spike-wasm-size`: builds the spike's WebAssembly client (the exports of `wasm_api`) as a web page would load it, processed by wasm-bindgen, and reports its size, raw and compressed with gzip -9, for each build profile and with or without the provisional post-quantum suite.
pub fn wasm_size(root: &Path) -> Result {
    let tools = tools_dir()?;
    let wasm_bindgen = tool(&tools, "wasm-bindgen", "wasm-bindgen")?;
    let target_dir = root.join("spikes").join("mls").join("target");
    let mut lines = Vec::new();
    for (profile, directory) in [("release", "release"), ("release-size", "release-size")] {
        for features in [None, Some("provisional-pq")] {
            let mut build = spike_cargo(
                root,
                &[
                    "build",
                    "--locked",
                    "--package",
                    "bayan-mls-spike",
                    "--lib",
                    "--target",
                    WASM_TARGET,
                    "--profile",
                    profile,
                ],
            );
            if let Some(features) = features {
                build.args(["--features", features]);
            }
            step(
                &format!("MLS spike: WebAssembly build ({profile}, features {features:?})"),
                &mut build,
            )?;
            let input = target_dir
                .join(WASM_TARGET)
                .join(directory)
                .join("bayan_mls_spike.wasm");
            let output_dir = target_dir
                .join("wasm-size")
                .join(format!("{profile}-{}", features.unwrap_or("default")));
            crate::remove_if_exists(&output_dir)?;
            step(
                "wasm-bindgen",
                Command::new(&wasm_bindgen)
                    .args(["--target", "web", "--out-dir"])
                    .arg(&output_dir)
                    .arg(&input),
            )?;
            let wasm = output_dir.join("bayan_mls_spike_bg.wasm");
            let raw = std::fs::metadata(&wasm)
                .map_err(|error| format!("cannot read {}: {error}", wasm.display()))?
                .len();
            let gzip = Command::new("gzip")
                .args(["-9", "--stdout", "--no-name"])
                .arg(&wasm)
                .output()
                .map_err(|error| format!("cannot run gzip: {error}"))?;
            if !gzip.status.success() {
                return Err("gzip failed".to_owned());
            }
            lines.push(format!(
                "| {profile} | {} | {raw} | {} |",
                features.unwrap_or("default"),
                gzip.stdout.len()
            ));
        }
    }
    eprintln!("| profile | features | WebAssembly bytes | gzip -9 bytes |");
    eprintln!("|---|---|---|---|");
    for line in lines {
        eprintln!("{line}");
    }
    Ok(())
}
