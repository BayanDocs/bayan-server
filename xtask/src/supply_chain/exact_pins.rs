//! `cargo xtask check-exact-pins`: every dependency is pinned exactly (ADR-0017 rule 5).
//!
//! Two views of the dependencies are checked, so that neither can be used to slip past the other:
//!
//! 1. Every entry of `[workspace.dependencies]` in the workspace's root `Cargo.toml`, as written: a crates.io dependency has an exact `=x.y.z` version requirement (Cargo reads a plain `"1.2.3"` as "1.2.3 or any later 1.x"), a path dependency points to a workspace member, and a Git dependency names a fixed `rev` or `tag` (never a branch). An entry may not name another registry.
//! 2. Every dependency of every workspace member, as Cargo resolves the manifests (`cargo metadata`): the same rules, also for a dependency that a member declares itself instead of inheriting it with `name.workspace = true`.
//!
//! Both views are checked in every workspace of the repository (`workspace.rs`): the main workspace at the root, and each secondary workspace below it, such as bayan-core's `fuzz/`. A secondary workspace's path dependencies may also point to members of the main workspace, whose own dependencies are checked with the main workspace; the main workspace's may not point outside it. The check also fails when Git tracks a `Cargo.toml` that belongs to none of these workspaces, which Cargo could build without a committed lockfile (`workspace::check_manifests`).
//!
//! Exact pins make the versions in `Cargo.lock` the only ones that can be built, so `cargo update` cannot move a dependency without a reviewed change to the manifests.

use std::collections::BTreeSet;
use std::path::Path;

use super::cargo;
use super::json::{self, Value};
use super::manifest::{self, Dependency};
use super::workspace::{self, Members, Workspace};

/// Runs the check for every workspace of the repository at `root`, and checks that every `Cargo.toml` that Git tracks belongs to one of them (`workspace::check_manifests`), reporting what it checked through `report`.
pub fn check(root: &Path, report: &mut dyn FnMut(&str)) -> Result<(), String> {
    let main = Workspace::main(root);
    let secondary = workspace::secondary(root)?;
    let main_metadata = metadata(&main)?;
    let main_members = Members::from_metadata(&main_metadata)?;
    let mut problems = check_workspace(
        &main,
        &main_metadata,
        &Allowed {
            own: &main_members,
            main: None,
        },
        report,
    )?;
    let mut secondary_members = Vec::new();
    for workspace in &secondary {
        let metadata = metadata(workspace)?;
        let members = Members::from_metadata(&metadata)?;
        problems.extend(check_workspace(
            workspace,
            &metadata,
            &Allowed {
                own: &members,
                main: Some(&main_members),
            },
            report,
        )?);
        secondary_members.push(members);
    }
    let mut failures = Vec::new();
    if !problems.is_empty() {
        failures.push(format!(
            "dependencies must be pinned exactly (ADR-0017 rule 5):\n  - {}\nDeclare each third-party dependency once in [workspace.dependencies] of the workspace's root Cargo.toml with an exact version, such as `name = \"=1.2.3\"`, and use it in a crate with `name.workspace = true`. A path dependency may point only to a member of the crate's own workspace or, from a secondary workspace (a folder below the root with its own Cargo.lock), to a member of the main workspace.",
            problems.join("\n  - ")
        ));
    }
    let members: Vec<&Members> = std::iter::once(&main_members)
        .chain(&secondary_members)
        .collect();
    if let Err(problem) = workspace::check_manifests(root, &secondary, &members, report) {
        failures.push(problem);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n\n"))
    }
}

/// Cargo's description of the members of `workspace` (`cargo metadata --no-deps`), after checking that Cargo reads its folder as the root of a workspace.
fn metadata(workspace: &Workspace) -> Result<Value, String> {
    let metadata = json::parse(&cargo::metadata(&workspace.folder, &["--no-deps"])?)?;
    workspace::check_root(workspace, &metadata)?;
    Ok(metadata)
}

/// Checks `workspace`, whose members `metadata` describes and whose path dependencies may point to `allowed`, reports what it checked if it found nothing wrong, and returns the problems it found.
fn check_workspace(
    workspace: &Workspace,
    metadata: &Value,
    allowed: &Allowed<'_>,
    report: &mut dyn FnMut(&str),
) -> Result<Vec<String>, String> {
    let manifest = workspace.file("Cargo.toml");
    let manifest_path = workspace.folder.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let declared = manifest::workspace_dependencies(&text)
        .map_err(|problem| workspace.locate("Cargo.toml", problem))?;
    let mut problems = Vec::new();
    for dependency in &declared {
        if let Err(problem) = check_declared(dependency, &workspace.folder, allowed) {
            problems.push(format!(
                "[workspace.dependencies] `{}` ({manifest} line {}): {problem}",
                dependency.name, dependency.line
            ));
        }
    }
    let mut used = 0;
    // The members of the main workspace that a secondary workspace depends on by path.
    let mut on_main = BTreeSet::new();
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
            match check_resolved(dependency, allowed) {
                Ok(()) => {
                    if let Some(member) = dependency
                        .string("path")
                        .and_then(|path| allowed.on_main(Path::new(path)))
                    {
                        on_main.insert(member.to_owned());
                    }
                }
                Err(problem) => problems.push(format!(
                    "{package_name} depends on `{}`: {problem}",
                    dependency.string("name").unwrap_or("?")
                )),
            }
        }
    }
    if !problems.is_empty() {
        return Ok(problems);
    }
    if workspace.is_main() {
        report(&format!(
            "all {} entries of [workspace.dependencies] and all {used} dependencies of the {} workspace members are pinned exactly",
            declared.len(),
            allowed.own.count()
        ));
    } else {
        let members = match allowed.own.count() {
            1 => "1 workspace member".to_owned(),
            count => format!("{count} workspace members"),
        };
        let entries = match declared.len() {
            0 => ", and it has no [workspace.dependencies]".to_owned(),
            count => format!(", and so are all {count} entries of its [workspace.dependencies]"),
        };
        let on_main = if on_main.is_empty() {
            String::new()
        } else {
            format!(
                "; it depends by path on members of the main workspace ({}), whose own dependencies the main workspace's check covers",
                on_main.into_iter().collect::<Vec<_>>().join(", ")
            )
        };
        report(&format!(
            "{manifest}: all {used} dependencies of its {members} are pinned exactly{entries}{on_main}"
        ));
    }
    Ok(problems)
}

/// The folders that a workspace's path dependencies may point to: its own members', and for a secondary workspace also the main workspace's.
struct Allowed<'a> {
    /// The members of the workspace being checked.
    own: &'a Members,
    /// The members of the main workspace, when the workspace being checked is a secondary one.
    main: Option<&'a Members>,
}

impl Allowed<'_> {
    /// Whether a path dependency may point to `folder`.
    fn contains(&self, folder: &Path) -> bool {
        self.own.at(folder).is_some() || self.on_main(folder).is_some()
    }

    /// The name of the member of the main workspace whose folder is `folder`, when the workspace being checked is a secondary one.
    fn on_main(&self, folder: &Path) -> Option<&str> {
        self.main.and_then(|main| main.at(folder))
    }

    /// What a path dependency must point to, for messages.
    fn expected(&self) -> &'static str {
        if self.main.is_some() {
            "the folder of a member of this workspace or of the main workspace"
        } else {
            "the folder of a workspace member"
        }
    }
}

/// Checks one entry of `[workspace.dependencies]` of the workspace in `folder`, whose path dependencies may point to `allowed`.
fn check_declared(
    dependency: &Dependency,
    folder: &Path,
    allowed: &Allowed<'_>,
) -> Result<(), String> {
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
        if !allowed.contains(&folder.join(path)) {
            return Err(format!("`{path}` is not {}", allowed.expected()));
        }
        return Ok(());
    }
    if dependency.has("git") {
        if dependency.has("branch") {
            return Err(
                "a Git dependency must name a fixed `rev` or `tag`, not a branch".to_owned(),
            );
        }
        return match (dependency.field("rev"), dependency.field("tag")) {
            (Some(rev), None) if commit_id(rev) => Ok(()),
            (Some(rev), None) => Err(format!(
                "`rev = \"{rev}\"` is not a full commit hash, so it can name a branch; write the commit's 40-character hash"
            )),
            (None, Some(tag)) if !tag.is_empty() => Ok(()),
            _ => Err(
                "a Git dependency must name one fixed `rev` (a full commit hash) or `tag`"
                    .to_owned(),
            ),
        };
    }
    if dependency.field("version").is_none() {
        return Err("it has no version; write an exact one, such as `\"=1.2.3\"`".to_owned());
    }
    Ok(())
}

/// Checks one dependency of a workspace member, as `cargo metadata` describes it, in a workspace whose path dependencies may point to `allowed`.
fn check_resolved(dependency: &Value, allowed: &Allowed<'_>) -> Result<(), String> {
    match dependency.string("source") {
        None => {
            let path = dependency
                .string("path")
                .ok_or("it has neither a source nor a path")?;
            if allowed.contains(Path::new(path)) {
                Ok(())
            } else {
                Err(format!("its path {path} is not {}", allowed.expected()))
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
        Some(source) if source.starts_with("git+") => fixed_git_source(source),
        Some(source) => Err(format!("its source {source} is not crates.io")),
    }
}

/// Checks that a Git source, as Cargo writes it (`git+<address>?<reference>`, followed by `#<commit>` in `Cargo.lock`), names a fixed reference: `rev=` with a full commit hash, or `tag=`. Cargo writes no reference for a dependency on the default branch, and it appends the reference to an address that may already have a query of its own, so the reference is what follows the last `?`, and an address that holds another `?` or a `#` is refused as ambiguous.
fn fixed_git_source(source: &str) -> Result<(), String> {
    let without_commit = source.split_once('#').map_or(source, |(source, _)| source);
    let fixed = without_commit
        .rsplit_once('?')
        .filter(|(address, _)| !address.contains(['?', '#']))
        .and_then(|(_, reference)| reference.split_once('='))
        .is_some_and(|(kind, value)| match kind {
            "rev" => commit_id(value),
            "tag" => !value.is_empty(),
            _ => false,
        });
    if fixed {
        Ok(())
    } else {
        Err(format!(
            "the Git source {source} must name one fixed `rev` (a full commit hash) or `tag`"
        ))
    }
}

/// Whether `text` is a full Git commit hash: 40 (SHA-1) or 64 (SHA-256) hexadecimal digits.
fn commit_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
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

#[cfg(test)]
mod tests {
    use super::super::git::testing::TestRepository;
    use super::*;

    fn members(root: &Path) -> Members {
        Members::of(&[("own", &root.join("crates").join("own"))])
    }

    fn declared(text: &str) -> Vec<Result<(), String>> {
        let root = std::env::temp_dir();
        let members = members(&root);
        let allowed = Allowed {
            own: &members,
            main: None,
        };
        manifest::workspace_dependencies(&format!("[workspace.dependencies]\n{text}"))
            .unwrap()
            .iter()
            .map(|dependency| check_declared(dependency, &root, &allowed))
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
            "a = \"1.2.3\"\nb = { version = \"^0.8\" }\nc = { features = [\"x\"] }\nd = { path = \"../outside\" }\ne = { git = \"https://example.org/e\" }\nf = { git = \"https://example.org/f\", branch = \"main\" }\ng = { version = \"=1.0.0\", registry = \"other\" }\nh = { path = \"crates/own\", version = \"1.0\" }\ni = { git = \"https://example.org/i\", rev = \"refs/heads/main\" }\nj = { git = \"https://example.org/j\", rev = \"eb23095\" }\nk = { git = \"https://example.org/k\", rev = \"eb23095592359c454a586d16d08b2bd3af44b551\", tag = \"v1\" }\n",
        )
        .into_iter()
        .map(Result::unwrap_err)
        .collect();
        assert_eq!(problems.len(), 11);
        assert!(
            problems[8].contains("is not a full commit hash"),
            "{problems:?}"
        );
        assert!(
            problems[9].contains("is not a full commit hash"),
            "{problems:?}"
        );
        assert!(problems[10].contains("one fixed `rev`"), "{problems:?}");
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
        assert!(problems[4].contains("one fixed `rev`"), "{problems:?}");
        assert!(problems[5].contains("not a branch"), "{problems:?}");
        assert!(problems[6].contains("another registry"), "{problems:?}");
        assert!(problems[7].contains("`1.0`"), "{problems:?}");
    }

    fn resolved(json_text: &str) -> Result<(), String> {
        let root = std::env::temp_dir();
        let members = members(&root);
        check_resolved(
            &json::parse(json_text).unwrap(),
            &Allowed {
                own: &members,
                main: None,
            },
        )
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
        // The reference is what follows the last `?`: an address with a query of its own cannot pass a branch off as a revision.
        for source in [
            "git+https://example.org/floating?rev=0?branch=main",
            "git+https://example.org/floating?rev=eb23095592359c454a586d16d08b2bd3af44b551?branch=main",
            "git+https://example.org/r?rev=refs/heads/main",
            "git+https://example.org/r?rev=eb23095",
            "git+https://example.org/r?branch=main#eb23095592359c454a586d16d08b2bd3af44b551",
        ] {
            assert!(
                resolved(&format!(
                    "{{\"name\":\"g\",\"source\":\"{source}\",\"req\":\"*\"}}"
                ))
                .is_err(),
                "{source}"
            );
        }
        assert_eq!(
            resolved(
                "{\"name\":\"g\",\"source\":\"git+https://example.org/g?rev=eb23095592359c454a586d16d08b2bd3af44b551#eb23095592359c454a586d16d08b2bd3af44b551\",\"req\":\"*\"}"
            ),
            Ok(())
        );
    }

    /// `text` as a JSON string, for paths that may contain backslashes (Windows).
    fn json_string(text: &str) -> String {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// A path dependency on `folder`, as `cargo metadata` describes it.
    fn path_dependency(folder: &Path) -> Value {
        json::parse(&format!(
            "{{\"name\":\"x\",\"source\":null,\"req\":\"*\",\"path\":{}}}",
            json_string(&folder.to_string_lossy())
        ))
        .unwrap()
    }

    /// A secondary workspace, such as bayan-core's `fuzz/`, may depend by path on members of the main workspace and on its own members, and on nothing else (AC-1 of bayan-core#22). The folders exist, so that `..` in a path written in a manifest resolves as it does in a real repository.
    #[test]
    fn a_secondary_workspace_may_depend_by_path_on_members_of_the_main_workspace() {
        let test = TestRepository::new();
        let root = test.path();
        for folder in ["crates/own", "fuzz", "elsewhere"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        let main = members(root);
        let fuzz = Members::of(&[("bayan-fuzz", &root.join("fuzz"))]);
        let secondary = Allowed {
            own: &fuzz,
            main: Some(&main),
        };
        let own = root.join("crates").join("own");
        assert_eq!(check_resolved(&path_dependency(&own), &secondary), Ok(()));
        assert_eq!(secondary.on_main(&own), Some("own"));
        assert_eq!(
            check_resolved(&path_dependency(&root.join("fuzz")), &secondary),
            Ok(())
        );
        assert_eq!(secondary.on_main(&root.join("fuzz")), None);
        let problem =
            check_resolved(&path_dependency(&root.join("elsewhere")), &secondary).unwrap_err();
        assert!(
            problem.contains(
                "is not the folder of a member of this workspace or of the main workspace"
            ),
            "{problem}"
        );
        // The same, written in [workspace.dependencies] of the secondary workspace's root Cargo.toml, relative to its folder.
        let entries = manifest::workspace_dependencies(
            "[workspace.dependencies]\nown = { path = \"../crates/own\" }\nother = { path = \"../elsewhere\" }\n",
        )
        .unwrap();
        assert_eq!(
            check_declared(&entries[0], &root.join("fuzz"), &secondary),
            Ok(())
        );
        assert!(
            check_declared(&entries[1], &root.join("fuzz"), &secondary)
                .unwrap_err()
                .contains("`../elsewhere` is not the folder of a member of this workspace or of the main workspace")
        );
        // The main workspace's rule does not change: its crates may not depend on a secondary workspace's.
        let main_only = Allowed {
            own: &main,
            main: None,
        };
        assert!(
            check_resolved(&path_dependency(&root.join("fuzz")), &main_only)
                .unwrap_err()
                .contains("is not the folder of a workspace member")
        );
    }
}
