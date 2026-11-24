//! Commands for the MLS spike of work package SRV-002 (`spikes/mls`, a Cargo workspace of its own; see its REPORT.md).
//!
//! - [`verify_steps`]: the spike's part of `cargo xtask verify` after the server's steps (format, Clippy, tests, documentation, and cargo deny with the spike's own `spikes/mls/deny.toml`, which is the server's `deny.toml`, byte for byte, plus one line, the advisory exception that the owner accepted for the spike only; a test below checks exactly that). The supply-chain checks at the very start of `verify` (exact pins, and the age and checksum of every package version a change adds to its `Cargo.lock`) cover the spike's workspace ([`workspace`]) too: they find it by its `Cargo.lock`.
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

/// The folder of the spike's Cargo workspace, which has its own `Cargo.lock`.
fn workspace(root: &Path) -> PathBuf {
    root.join("spikes").join("mls")
}

fn spike_manifest(root: &Path) -> PathBuf {
    workspace(root).join("Cargo.toml")
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
    // The spike's own policy: the server's, plus one line, the advisory exception that only the spike needs. The server's audit (`cargo xtask verify`) reads the root `deny.toml`, whose list of exceptions is empty.
    let config = workspace(root).join("deny.toml");
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

#[cfg(test)]
mod tests {
    /// The advisory that the owner accepted for the MLS spike only, on 2026-10-27 (bayan-server#8): RUSTSEC-2026-0387, a timing leak in crypto-bigint 0.5.5, which only the spike reaches.
    const SPIKE_ADVISORY: &str = "RUSTSEC-2026-0387";

    /// The server's policy (the root `deny.toml`) and the spike's (`spikes/mls/deny.toml`), as checked out.
    const SERVER_POLICY: &str = include_str!("../../deny.toml");
    const SPIKE_POLICY: &str = include_str!("../../spikes/mls/deny.toml");

    /// Checks that `spike` is `server` byte for byte, except for exactly one added line inside the `ignore` list of `[advisories]` (after its line `ignore = [` and before the line `]` that closes it), and that this line is exactly `  { id = "RUSTSEC-2026-0387", reason = "…" },` with a reason that contains no `"` and no `\`, so that the line holds that one exception and nothing else: no second exception, as a table or a bare string, and no other key. Also checks that the `ignore` list of `server` is empty, its line `ignore = [` directly followed by the line `]`, so that the server's own audit has no advisory exception, whatever an entry there would hold and however it would write an ID (a TOML escape such as `\u0030` writes the same ID with other characters). The server keeps no advisory exception of its own: one that a future security alert needs changes this check in a reviewed pull request, together with the owner's acceptance. The check does not judge the policy that both files share: a setting in both, such as a crate excluded from the graph, changes what both audits see, and only review catches it.
    fn check_spike_policy(server: &str, spike: &str) -> Result<(), String> {
        let server_lines: Vec<&str> = server.split_inclusive('\n').collect();
        let spike_lines: Vec<&str> = spike.split_inclusive('\n').collect();
        if spike_lines.len() != server_lines.len() + 1 {
            return Err(format!(
                "spikes/mls/deny.toml has {} lines and the root deny.toml {}; it must have exactly one line more",
                spike_lines.len(),
                server_lines.len()
            ));
        }
        // The first line that differs is the added one; every line after it must equal the root file's line before it.
        let added = server_lines
            .iter()
            .zip(&spike_lines)
            .position(|(server_line, spike_line)| server_line != spike_line)
            .unwrap_or(server_lines.len());
        if spike_lines[added + 1..] != server_lines[added..] {
            return Err(
                "spikes/mls/deny.toml differs from the root deny.toml in more than one added line"
                    .to_owned(),
            );
        }
        // The added line must lie inside the `ignore` list of `[advisories]`, as the root file lays it out.
        let line_is = |line: &str, text: &str| line.trim_end_matches(['\n', '\r']) == text;
        let advisories = server_lines
            .iter()
            .position(|line| line_is(line, "[advisories]"))
            .ok_or("the root deny.toml has no line `[advisories]`")?;
        let open = advisories
            + server_lines[advisories..]
                .iter()
                .position(|line| line_is(line, "ignore = ["))
                .ok_or("the root deny.toml has no line `ignore = [` after `[advisories]`")?;
        if server_lines[advisories + 1..open]
            .iter()
            .any(|line| line.trim_start().starts_with('['))
        {
            return Err(
                "the root deny.toml's line `ignore = [` is not in the [advisories] table"
                    .to_owned(),
            );
        }
        let close = open
            + server_lines[open..]
                .iter()
                .position(|line| line_is(line, "]"))
                .ok_or("the root deny.toml's `ignore` list has no closing line `]`")?;
        // The server keeps no advisory exception of its own: its list is empty, whatever an entry would hold and however it would write an ID.
        if close != open + 1 {
            return Err(
                "the root deny.toml's `ignore` list in [advisories] is not empty: the server keeps no advisory exception of its own, so its line `ignore = [` must be followed directly by the line `]`"
                    .to_owned(),
            );
        }
        if added != close {
            return Err(format!(
                "the line that spikes/mls/deny.toml adds (line {}) is not inside the `ignore` list of [advisories]",
                added + 1
            ));
        }
        // The added line holds exactly the one exception.
        let line = spike_lines[added].trim_end_matches(['\n', '\r']);
        let reason = line
            .strip_prefix(&format!("  {{ id = \"{SPIKE_ADVISORY}\", reason = \""))
            .and_then(|rest| rest.strip_suffix("\" },"))
            .ok_or_else(|| {
                format!(
                    "the line that spikes/mls/deny.toml adds must be exactly `  {{ id = \"{SPIKE_ADVISORY}\", reason = \"…\" }},`, not `{line}`"
                )
            })?;
        if reason.is_empty() || reason.contains(['"', '\\']) {
            return Err(format!(
                "the reason of {SPIKE_ADVISORY} must be a non-empty string without `\"` or `\\`, not `{reason}`"
            ));
        }
        Ok(())
    }

    /// The spike's audit reads `spikes/mls/deny.toml` and the server's the root `deny.toml`: the two must stay one policy, apart from the one exception that the owner accepted for the spike only, so that every other rule of the server's policy holds for the spike too, the exception never covers the server, and nothing else can join it unnoticed.
    #[test]
    fn the_spike_policy_is_the_servers_plus_one_advisory_exception() {
        assert_eq!(check_spike_policy(SERVER_POLICY, SPIKE_POLICY), Ok(()));
    }

    /// `text` with the first occurrence of `from` replaced by `to`; fails if `from` does not occur, so that no negative case passes because its edit missed.
    fn edited(text: &str, from: &str, to: &str) -> String {
        assert!(
            text.contains(from),
            "the edit's text {from:?} is not in the file"
        );
        text.replacen(from, to, 1)
    }

    /// The line of the exception in the spike's policy, with its line break (the comment above the list names the advisory too).
    fn exception_line() -> &'static str {
        SPIKE_POLICY
            .split_inclusive('\n')
            .find(|line| !line.trim_start().starts_with('#') && line.contains(SPIKE_ADVISORY))
            .expect("spikes/mls/deny.toml has no line for the spike's advisory")
    }

    /// Asserts that the spike's policy, edited as given, fails the check.
    fn refused(from: &str, to: &str) {
        let spike = edited(SPIKE_POLICY, from, to);
        assert!(
            check_spike_policy(SERVER_POLICY, &spike).is_err(),
            "an edit of spikes/mls/deny.toml from {from:?} to {to:?} passed the check"
        );
    }

    #[test]
    fn refuses_a_second_exception_as_a_bare_string() {
        let line = exception_line();
        // On a line of its own, and on the exception's own line; cargo-deny applies both.
        refused(line, &format!("{line}  \"RUSTSEC-2026-9999\",\n"));
        refused(line, &line.replace(" },\n", " }, \"RUSTSEC-2026-9999\",\n"));
    }

    #[test]
    fn refuses_a_second_table_on_the_exceptions_line() {
        let line = exception_line();
        refused(
            line,
            &line.replace(
                " },\n",
                " }, { id = \"RUSTSEC-2026-9999\", reason = \"another\" },\n",
            ),
        );
    }

    #[test]
    fn refuses_an_added_key_in_advisories_or_anywhere_else() {
        refused(
            "unsound = \"all\"\n",
            "unsound = \"all\"\ndb-urls = [\"https://example.invalid/advisory-db\"]\n",
        );
        refused(
            "[licenses]\n",
            "[licenses]\nexceptions = [{ crate = \"x\", allow = [\"GPL-3.0\"] }]\n",
        );
        refused("[bans]\n", "[bans]\nskip = [\"cc\"]\n");
    }

    #[test]
    fn refuses_the_list_closed_differently() {
        let line = exception_line();
        let closed = format!("{line}]\n");
        // A trailing space after `]`, then a key before a later `]`, as the security review found.
        refused(
            &closed,
            &format!("{line}] \ndb-urls = [\"https://example.invalid/advisory-db\"\n]\n"),
        );
        // The list closed on the exception's own line.
        refused(&closed, &line.replace(" },\n", " } ]\n"));
    }

    #[test]
    fn refuses_a_reason_that_could_end_its_string_or_escape() {
        let line = exception_line();
        refused(
            line,
            &line.replace(
                "reason = \"",
                "reason = \"a\", db-urls = [\"x\"], note = \"",
            ),
        );
        refused(line, &line.replace("reason = \"", "reason = \"a \\u0022 "));
    }

    #[test]
    fn refuses_the_exception_outside_the_ignore_list() {
        let line = exception_line();
        // The exception moved above the list: the added line is outside it.
        let moved = edited(SPIKE_POLICY, line, "");
        let moved = edited(&moved, "ignore = [\n", &format!("{line}ignore = [\n"));
        assert!(check_spike_policy(SERVER_POLICY, &moved).is_err());
    }

    /// An entry in the server's own list, added to both files so that the spike's file is still the root's plus its one line: each is refused, and for that reason, however the entry writes its ID.
    #[test]
    fn refuses_any_entry_in_the_servers_own_list() {
        let entries = [
            // The spike's advisory with a TOML Unicode escape for the first 0 of 0387, which TOML, and so cargo-deny, reads as the same ID.
            concat!(
                r#"  { id = "RUSTSEC-2026-\u0030387", reason = "escaped" },"#,
                "\n"
            ),
            // Another advisory, as a bare string and as a table.
            "  \"RUSTSEC-2026-9999\",\n",
            "  { id = \"RUSTSEC-2026-9999\", reason = \"another\" },\n",
            // The spike's own exception, in the server's list too.
            exception_line(),
        ];
        for entry in entries {
            let server = edited(
                SERVER_POLICY,
                "ignore = [\n",
                &format!("ignore = [\n{entry}"),
            );
            let spike = edited(
                SPIKE_POLICY,
                "ignore = [\n",
                &format!("ignore = [\n{entry}"),
            );
            let Err(error) = check_spike_policy(&server, &spike) else {
                panic!("an entry {entry:?} in both lists passed the check");
            };
            assert!(
                error.contains("is not empty"),
                "an entry {entry:?} in both lists was refused for another reason: {error}"
            );
        }
    }

    #[test]
    fn refuses_a_rule_changed_in_one_file_only() {
        refused(
            "multiple-versions = \"warn\"\n",
            "multiple-versions = \"allow\"\n",
        );
        let server = edited(
            SERVER_POLICY,
            "multiple-versions = \"warn\"\n",
            "multiple-versions = \"deny\"\n",
        );
        assert!(check_spike_policy(&server, SPIKE_POLICY).is_err());
    }
}
