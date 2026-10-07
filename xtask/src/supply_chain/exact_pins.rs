//! `cargo xtask check-exact-pins`: every dependency is pinned exactly (ADR-0017 rule 5).
//!
//! Two views of the dependencies are checked, so that neither can be used to slip past the other:
//!
//! 1. Every entry of `[workspace.dependencies]` in the root `Cargo.toml`, as written: a crates.io dependency has an exact `=x.y.z` version requirement (Cargo reads a plain `"1.2.3"` as "1.2.3 or any later 1.x"), a path dependency points to a workspace member, and a Git dependency names a fixed `rev` or `tag` (never a branch). An entry may not name another registry.
//! 2. Every dependency of every workspace member, as Cargo resolves the manifests (`cargo metadata`): the same rules, also for a dependency that a member declares itself instead of inheriting it with `name.workspace = true`.
//!
//! Exact pins make the versions in `Cargo.lock` the only ones that can be built, so `cargo update` cannot move a dependency without a reviewed change to the manifests.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::json::{self, Value};
use super::manifest::{self, Dependency};

/// Runs the check for the workspace at `root`, reporting what it checked through `report`.
pub fn check(root: &Path, report: &mut dyn FnMut(&str)) -> Result<(), String> {
    let manifest_path = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let declared = manifest::workspace_dependencies(&text)?;
    let metadata = json::parse(&cargo_metadata(root)?)?;
    let members = Members::from_metadata(&metadata)?;
    let mut problems = Vec::new();
    for dependency in &declared {
        if let Err(problem) = check_declared(dependency, root, &members) {
            problems.push(format!(
                "[workspace.dependencies] `{}` (Cargo.toml line {}): {problem}",
                dependency.name, dependency.line
            ));
        }
    }
    let mut used = 0;
    for package in metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("`cargo metadata` lists no packages")?
    {
        let package_name = package.string("name").unwrap_or("?");
        for dependency in package
            .get("dependencies")
            .and_then(Value::as_array)
            .unwrap_or_default()
        {
            used += 1;
            if let Err(problem) = check_resolved(dependency, &members) {
                problems.push(format!(
                    "{package_name} depends on `{}`: {problem}",
                    dependency.string("name").unwrap_or("?")
                ));
            }
        }
    }
    if problems.is_empty() {
        report(&format!(
            "all {} entries of [workspace.dependencies] and all {used} dependencies of the {} workspace members are pinned exactly",
            declared.len(),
            members.folders.len()
        ));
        Ok(())
    } else {
        Err(format!(
            "dependencies must be pinned exactly (ADR-0017 rule 5):\n  - {}\nDeclare each third-party dependency once in [workspace.dependencies] of the root Cargo.toml with an exact version, such as `name = \"=1.2.3\"`, and use it in a crate with `name.workspace = true`.",
            problems.join("\n  - ")
        ))
    }
}

/// The folders of the workspace members, to recognize path dependencies on them.
struct Members {
    folders: Vec<PathBuf>,
}

impl Members {
    fn from_metadata(metadata: &Value) -> Result<Self, String> {
        let mut folders = Vec::new();
        for package in metadata
            .get("packages")
            .and_then(Value::as_array)
            .ok_or("`cargo metadata` lists no packages")?
        {
            let manifest = package
                .string("manifest_path")
                .ok_or("`cargo metadata` names a package without a manifest path")?;
            if let Some(folder) = Path::new(manifest).parent() {
                folders.push(canonical(folder));
            }
        }
        Ok(Self { folders })
    }

    fn contains(&self, folder: &Path) -> bool {
        self.folders.contains(&canonical(folder))
    }
}

/// The folder with links resolved, for comparisons; the folder as given if it does not exist.
fn canonical(folder: &Path) -> PathBuf {
    std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf())
}

/// Checks one entry of `[workspace.dependencies]`.
fn check_declared(dependency: &Dependency, root: &Path, members: &Members) -> Result<(), String> {
    for key in ["registry", "registry-index"] {
        if dependency.has(key) {
            return Err(format!(
                "`{key}` names another registry; dependencies come only from crates.io"
            ));
        }
    }
    if dependency.has("version") && dependency.field("version").is_none() {
        return Err("its version is not a plain string".to_owned());
    }
    if let Some(version) = dependency.field("version") {
        exact(version)?;
    }
    if dependency.has("path") {
        let path = dependency
            .field("path")
            .ok_or("its path is not a plain string")?;
        if !members.contains(&root.join(path)) {
            return Err(format!("`{path}` is not the folder of a workspace member"));
        }
        return Ok(());
    }
    if dependency.has("git") {
        if dependency.has("branch") {
            return Err(
                "a Git dependency must name a fixed `rev` or `tag`, not a branch".to_owned(),
            );
        }
        if dependency.field("rev").is_none() && dependency.field("tag").is_none() {
            return Err("a Git dependency must name a fixed `rev` or `tag`".to_owned());
        }
        return Ok(());
    }
    if dependency.field("version").is_none() {
        return Err("it has no version; write an exact one, such as `\"=1.2.3\"`".to_owned());
    }
    Ok(())
}

/// Checks one dependency of a workspace member, as `cargo metadata` describes it.
fn check_resolved(dependency: &Value, members: &Members) -> Result<(), String> {
    match dependency.string("source") {
        None => {
            let path = dependency
                .string("path")
                .ok_or("it has neither a source nor a path")?;
            if members.contains(Path::new(path)) {
                Ok(())
            } else {
                Err(format!(
                    "its path {path} is not the folder of a workspace member"
                ))
            }
        }
        Some(source) if source.starts_with("registry+") || source.starts_with("sparse+") => {
            if dependency.string("registry").is_some() {
                return Err(
                    "it comes from another registry; dependencies come only from crates.io"
                        .to_owned(),
                );
            }
            exact(dependency.string("req").unwrap_or_default())
        }
        Some(source) if source.starts_with("git+") => {
            let query = source.split_once('?').map_or("", |(_, query)| query);
            if query
                .split('&')
                .any(|pair| pair.starts_with("rev=") || pair.starts_with("tag="))
            {
                Ok(())
            } else {
                Err(format!(
                    "the Git source {source} must name a fixed `rev` or `tag`"
                ))
            }
        }
        Some(source) => Err(format!("its source {source} is not crates.io")),
    }
}

/// Checks that `requirement` is exact: `=` and a full version, such as `=1.2.3` or `=1.0.0-beta.2`.
fn exact(requirement: &str) -> Result<(), String> {
    let invalid = || {
        format!(
            "`{requirement}` is not an exact version requirement; write `=x.y.z`, such as `=1.2.3`"
        )
    };
    let version = requirement.strip_prefix('=').ok_or_else(invalid)?;
    let (version, build) = version
        .split_once('+')
        .map_or((version, None), |(version, build)| (version, Some(build)));
    let (core, pre_release) = version
        .split_once('-')
        .map_or((version, None), |(core, pre_release)| {
            (core, Some(pre_release))
        });
    let numbers: Vec<&str> = core.split('.').collect();
    let identifiers_valid = |text: Option<&str>| {
        text.is_none_or(|text| {
            text.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        })
    };
    if numbers.len() == 3
        && numbers
            .iter()
            .all(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        && identifiers_valid(pre_release)
        && identifiers_valid(build)
    {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Cargo's description of the workspace members, `cargo metadata --no-deps`, as JSON. It reads the manifests only: nothing is built and no build script runs.
fn cargo_metadata(root: &Path) -> Result<String, String> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--no-deps", "--format-version", "1", "--locked"])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn members(root: &Path) -> Members {
        Members {
            folders: vec![canonical(&root.join("crates").join("own"))],
        }
    }

    fn declared(text: &str) -> Vec<Result<(), String>> {
        let root = std::env::temp_dir();
        manifest::workspace_dependencies(&format!("[workspace.dependencies]\n{text}"))
            .unwrap()
            .iter()
            .map(|dependency| check_declared(dependency, &root, &members(&root)))
            .collect()
    }

    #[test]
    fn accepts_exact_versions_only() {
        for requirement in [
            "=1.2.3",
            "=0.0.1",
            "=10.20.30",
            "=1.0.0-beta.2",
            "=1.0.0-rc.1+build.5",
            "=1.2.3+meta",
        ] {
            assert_eq!(exact(requirement), Ok(()), "{requirement}");
        }
        for requirement in [
            "1.2.3",
            "^1.2.3",
            "~1.2.3",
            ">=1.2.3",
            "=1.2",
            "=1",
            "*",
            "=1.2.*",
            "= 1.2.3",
            "=1.2.3,<2",
            "=1.2.3-",
            "=1.2.3-beta..1",
            "=a.b.c",
            "",
        ] {
            assert!(exact(requirement).is_err(), "{requirement}");
        }
    }

    #[test]
    fn checks_every_form_of_workspace_dependency() {
        assert_eq!(
            declared(
                "a = \"=1.2.3\"\nb = { version = \"=0.8.9\", default-features = false }\nc.version = \"=2.0.0\"\nown = { path = \"crates/own\" }\ng = { git = \"https://example.org/g\", rev = \"eb23095592359c454a586d16d08b2bd3af44b551\" }\nt = { git = \"https://example.org/t\", tag = \"v1.0.0\", version = \"=1.0.0\" }\n"
            ),
            [Ok(()), Ok(()), Ok(()), Ok(()), Ok(()), Ok(())]
        );
        let problems: Vec<String> = declared(
            "a = \"1.2.3\"\nb = { version = \"^0.8\" }\nc = { features = [\"x\"] }\nd = { path = \"../outside\" }\ne = { git = \"https://example.org/e\" }\nf = { git = \"https://example.org/f\", branch = \"main\" }\ng = { version = \"=1.0.0\", registry = \"other\" }\nh = { path = \"crates/own\", version = \"1.0\" }\n",
        )
        .into_iter()
        .map(Result::unwrap_err)
        .collect();
        assert_eq!(problems.len(), 8);
        assert!(
            problems[0].contains("`1.2.3` is not an exact version requirement"),
            "{problems:?}"
        );
        assert!(problems[1].contains("`^0.8`"), "{problems:?}");
        assert!(problems[2].contains("it has no version"), "{problems:?}");
        assert!(
            problems[3].contains("is not the folder of a workspace member"),
            "{problems:?}"
        );
        assert!(problems[4].contains("fixed `rev` or `tag`"), "{problems:?}");
        assert!(problems[5].contains("not a branch"), "{problems:?}");
        assert!(problems[6].contains("another registry"), "{problems:?}");
        assert!(problems[7].contains("`1.0`"), "{problems:?}");
    }

    fn resolved(json_text: &str) -> Result<(), String> {
        let root = std::env::temp_dir();
        check_resolved(&json::parse(json_text).unwrap(), &members(&root))
    }

    #[test]
    fn checks_the_dependencies_cargo_resolves() {
        let crates_io = "registry+https://github.com/rust-lang/crates.io-index";
        assert_eq!(
            resolved(&format!(
                "{{\"name\":\"a\",\"source\":\"{crates_io}\",\"req\":\"=1.2.3\",\"registry\":null}}"
            )),
            Ok(())
        );
        assert!(
            resolved(&format!(
                "{{\"name\":\"a\",\"source\":\"{crates_io}\",\"req\":\"^1.2.3\",\"registry\":null}}"
            ))
            .is_err()
        );
        assert!(resolved(&format!("{{\"name\":\"a\",\"source\":\"{crates_io}\",\"req\":\"=1.2.3\",\"registry\":\"https://example.org/index\"}}")).is_err());
        let own = std::env::temp_dir().join("crates").join("own");
        let own = json_string(&own.to_string_lossy());
        assert_eq!(
            resolved(&format!(
                "{{\"name\":\"own\",\"source\":null,\"req\":\"*\",\"path\":{own}}}"
            )),
            Ok(())
        );
        let outside = json_string(&std::env::temp_dir().join("elsewhere").to_string_lossy());
        assert!(
            resolved(&format!(
                "{{\"name\":\"x\",\"source\":null,\"req\":\"*\",\"path\":{outside}}}"
            ))
            .is_err()
        );
        assert_eq!(
            resolved(
                "{\"name\":\"g\",\"source\":\"git+https://example.org/g?rev=eb23095592359c454a586d16d08b2bd3af44b551\",\"req\":\"*\"}"
            ),
            Ok(())
        );
        assert_eq!(
            resolved(
                "{\"name\":\"t\",\"source\":\"git+https://example.org/t?tag=v1\",\"req\":\"*\"}"
            ),
            Ok(())
        );
        assert!(resolved("{\"name\":\"b\",\"source\":\"git+https://example.org/b?branch=main\",\"req\":\"*\"}").is_err());
        assert!(
            resolved("{\"name\":\"b\",\"source\":\"git+https://example.org/b\",\"req\":\"*\"}")
                .is_err()
        );
        assert!(
            resolved("{\"name\":\"o\",\"source\":\"directory+/vendor\",\"req\":\"=1.0.0\"}")
                .is_err()
        );
    }

    /// `text` as a JSON string, for paths that may contain backslashes (Windows).
    fn json_string(text: &str) -> String {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }
}
