//! Version information reported by `/version` and `bayan-server version`.

use serde::Serialize;

/// The source revision the binary was built from, passed in at build time through the `BAYAN_BUILD_COMMIT` environment variable (the container build sets it); `unknown` for local builds.
pub const BUILD_COMMIT: &str = match option_env!("BAYAN_BUILD_COMMIT") {
    Some(commit) if !commit.is_empty() => commit,
    _ => "unknown",
};

/// Name, version and source revision of the running server.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VersionInfo {
    /// Always `bayan-server`.
    pub name: &'static str,
    /// The crate version.
    pub version: &'static str,
    /// The source revision, or `unknown`.
    pub commit: &'static str,
}

impl VersionInfo {
    /// Information about this build.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
            commit: BUILD_COMMIT,
        }
    }
}
