//! Runs `cargo metadata`, Cargo's description of the workspace as JSON.
//!
//! It reads manifests only: nothing is built and no build script runs. Without `--no-deps` it also reads the manifests of the dependencies, from Cargo's cache, downloading the crate files that are missing there.

use std::path::Path;
use std::process::{Command, Stdio};

/// The output of `cargo metadata --format-version 1 --locked` with `extra` arguments, run in the workspace at `root` with the Cargo that runs xtask.
pub fn metadata(root: &Path, extra: &[&str]) -> Result<String, String> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--locked"])
        .args(extra)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run `cargo metadata`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo metadata` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|_| "`cargo metadata` printed text that is not UTF-8".to_owned())
}
