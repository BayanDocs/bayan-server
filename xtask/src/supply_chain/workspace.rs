//! The Cargo workspaces of a repository, which both checks read: the main workspace at the repository's root, and every secondary workspace in a folder below it with a `Cargo.toml` and a `Cargo.lock` of its own, such as bayan-core's `fuzz/` (cargo-fuzz builds it with the nightly toolchain) and bayan-server's `spikes/mls`.
//!
//! The secondary workspaces are found by their lockfiles: every `Cargo.lock` that Git tracks below the root belongs to one, so a new workspace is checked from the commit that adds its lockfile, without anyone having to list it. Three guards make the checks fail instead of passing over what they cannot see:
//!
//! - a lockfile in a folder that Cargo does not read as a workspace's root (Cargo would never use it);
//! - a file named like `Cargo.lock` in another letter case (which Windows and macOS open as `Cargo.lock`);
//! - a `Cargo.toml` that Git tracks but that belongs to no checked workspace, being neither the root manifest of the main workspace or of a secondary one nor the manifest of one of their members (`check_manifests`). Cargo can build such a folder as a workspace of its own, from a lockfile that may never have been committed, so no check would see what it builds: a folder with a `[workspace]` table whose `Cargo.lock` is not committed or was removed, and also a package in a folder that a workspace excludes, which is a workspace of its own without a `[workspace]` table.
//!
//! A secondary workspace follows the rules of the main one, with one difference: it may also depend by path on members of the main workspace. Their code is in this repository and the main workspace's checks cover their own dependencies, while the versions of those dependencies that the secondary `Cargo.lock` lists are checked with it. A secondary workspace may depend on nothing else outside itself.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::git;
use super::json::Value;

/// The name of a workspace's lockfile, which Cargo keeps next to the workspace's root `Cargo.toml`.
const LOCKFILE: &str = "Cargo.lock";

/// The name of the manifest of a package or a workspace.
const MANIFEST: &str = "Cargo.toml";

/// A Cargo workspace of the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// The folder of its root `Cargo.toml` and of its `Cargo.lock`.
    pub folder: PathBuf,
    /// That folder relative to the repository's root, with `/` between folders, such as `fuzz`; empty for the main workspace.
    pub name: String,
}

impl Workspace {
    /// The main workspace, at the repository's root `root`.
    pub fn main(root: &Path) -> Self {
        Self {
            folder: root.to_path_buf(),
            name: String::new(),
        }
    }

    /// Whether this is the main workspace.
    pub fn is_main(&self) -> bool {
        self.name.is_empty()
    }

    /// The path of `file` in this workspace's folder relative to the repository's root, for people: `Cargo.lock`, `fuzz/Cargo.lock`.
    pub fn file(&self, file: &str) -> String {
        if self.is_main() {
            file.to_owned()
        } else {
            format!("{}/{file}", self.name)
        }
    }

    /// `problem`, a message that starts with the name of `file`, such as `Cargo.lock line 7: …`, with that name replaced by the file's path relative to the repository's root, so that a message about a secondary workspace's lockfile names `fuzz/Cargo.lock`.
    pub fn locate(&self, file: &str, problem: String) -> String {
        match problem.strip_prefix(file) {
            Some(rest) if !self.is_main() => format!("{}{rest}", self.file(file)),
            _ => problem,
        }
    }
}

/// The secondary workspaces of the repository at `root`, in the order of their folders: the folders below `root` where Git tracks a `Cargo.lock`.
pub fn secondary(root: &Path) -> Result<Vec<Workspace>, String> {
    let mut workspaces = Vec::new();
    let mut problems = Vec::new();
    for path in git::tracked_files(root)? {
        let (folder, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        if path == LOCKFILE || !name.eq_ignore_ascii_case(LOCKFILE) {
            continue;
        }
        if name == LOCKFILE {
            workspaces.push(Workspace {
                folder: below(root, folder),
                name: folder.to_owned(),
            });
        } else {
            problems.push(format!(
                "{path} is named like {LOCKFILE} in another letter case, which Windows and macOS open as {LOCKFILE}, so this check cannot be sure which lockfile Cargo reads there; rename it {LOCKFILE}, or remove it"
            ));
        }
    }
    if problems.is_empty() {
        Ok(workspaces)
    } else {
        Err(problems.join("\n"))
    }
}

/// Checks that Cargo reads the folder of `workspace` as the root of a workspace, according to `metadata`, the output of `cargo metadata` run in that folder. Otherwise Cargo uses another workspace's lockfile there, and the `Cargo.lock` in the folder is not what Cargo builds.
pub fn check_root(workspace: &Workspace, metadata: &Value) -> Result<(), String> {
    let found = metadata
        .string("workspace_root")
        .ok_or("`cargo metadata` names no workspace root")?;
    if canonical(Path::new(found)) == canonical(&workspace.folder) {
        Ok(())
    } else {
        Err(format!(
            "Cargo never uses {lockfile}: it reads the folder {} as part of the workspace at {found}, whose lockfile it uses instead. Remove {lockfile}, or make the folder a workspace of its own, with a `[workspace]` table in its Cargo.toml.",
            workspace.folder.display(),
            lockfile = workspace.file(LOCKFILE)
        ))
    }
}

/// Checks that every `Cargo.toml` that Git tracks in the repository at `root` belongs to a workspace that the checks read: that it is the root manifest of the main workspace or of one of the `secondary` ones, or the manifest of one of the `members` of those workspaces. Cargo can build any other as a workspace of its own, from a lockfile that may never have been committed, so that no check would see what it builds. A file named like `Cargo.toml` in another letter case counts too, because Windows and macOS open it as `Cargo.toml`.
pub fn check_manifests(
    root: &Path,
    secondary: &[Workspace],
    members: &[&Members],
    report: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let roots: Vec<PathBuf> = std::iter::once(root)
        .chain(secondary.iter().map(|workspace| workspace.folder.as_path()))
        .map(canonical)
        .collect();
    let mut count = 0;
    let mut outside = Vec::new();
    for path in git::tracked_files(root)? {
        let (folder, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
        if !name.eq_ignore_ascii_case(MANIFEST) {
            continue;
        }
        count += 1;
        let folder = canonical(&below(root, folder));
        if !roots.contains(&folder) && members.iter().all(|members| members.at(&folder).is_none()) {
            outside.push(path);
        }
    }
    if outside.is_empty() {
        report(&format!(
            "each of the {count} {MANIFEST} files that Git tracks is the root manifest of a checked workspace or the manifest of one of its members"
        ));
        Ok(())
    } else {
        Err(format!(
            "Git tracks {} {MANIFEST} file(s) that belong to no workspace these checks read, being neither the root manifest of the main workspace or of a secondary one (a folder below the root whose {LOCKFILE} Git tracks) nor the manifest of one of their members:\n  - {}\nCargo can build each of them as a workspace of its own, from a {LOCKFILE} that was never committed, so no check sees what it builds. Commit its {LOCKFILE} (its folder is then checked as a secondary workspace), make it a member of a checked workspace, or remove it.",
            outside.len(),
            outside.join("\n  - ")
        ))
    }
}

/// `path` with links resolved, for comparisons; `path` as given if it does not exist.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The folder `relative`, a path relative to `root` with `/` between folders (empty for `root` itself), below `root`.
fn below(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// The members of a workspace as `cargo metadata` lists them, by name and folder, to recognize path dependencies on them and packages that Cargo builds from their folders.
pub struct Members {
    /// Each member's name and the folder of its manifest, with links resolved.
    members: Vec<(String, PathBuf)>,
}

impl Members {
    /// The packages that `metadata`, the output of `cargo metadata` with or without `--no-deps`, lists as workspace members.
    pub fn from_metadata(metadata: &Value) -> Result<Self, String> {
        let ids: BTreeSet<&str> = metadata
            .get("workspace_members")
            .and_then(Value::as_array)
            .ok_or("`cargo metadata` lists no workspace members")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut members = Vec::new();
        for package in metadata
            .get("packages")
            .and_then(Value::as_array)
            .ok_or("`cargo metadata` lists no packages")?
        {
            let (Some(id), Some(name), Some(manifest)) = (
                package.string("id"),
                package.string("name"),
                package.string("manifest_path"),
            ) else {
                return Err(
                    "`cargo metadata` describes a package without an id, a name or a manifest path"
                        .to_owned(),
                );
            };
            if ids.contains(id) {
                let manifest = Path::new(manifest);
                members.push((
                    name.to_owned(),
                    canonical(manifest.parent().unwrap_or(manifest)),
                ));
            }
        }
        Ok(Self { members })
    }

    /// Members with the given names and folders, for tests.
    #[cfg(test)]
    pub fn of(members: &[(&str, &Path)]) -> Self {
        Self {
            members: members
                .iter()
                .map(|(name, folder)| ((*name).to_owned(), canonical(folder)))
                .collect(),
        }
    }

    /// How many members there are.
    pub fn count(&self) -> usize {
        self.members.len()
    }

    /// The name of the member whose folder is `folder`, if there is one.
    pub fn at(&self, folder: &Path) -> Option<&str> {
        let folder = canonical(folder);
        self.members
            .iter()
            .find(|(_, member)| *member == folder)
            .map(|(name, _)| name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::super::git::testing::TestRepository;
    use super::super::json;
    use super::*;

    /// `text` as a JSON string, for paths that may contain backslashes (Windows).
    fn json_string(text: &str) -> String {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    }

    #[test]
    fn finds_the_secondary_workspaces_by_their_lockfiles() {
        let test = TestRepository::new();
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "crates/own/Cargo.toml",
            "fuzz/Cargo.toml",
            "fuzz/Cargo.lock",
            "spikes/mls/Cargo.toml",
            "spikes/mls/Cargo.lock",
        ] {
            test.write(path, "");
        }
        test.commit("workspaces", 1_000_000_000);
        // A lockfile that Git does not track is not part of the repository's history, so no change can bring it in unnoticed.
        test.write("scratch/Cargo.lock", "");
        let found = secondary(test.path()).unwrap();
        let names: Vec<&str> = found
            .iter()
            .map(|workspace| workspace.name.as_str())
            .collect();
        assert_eq!(names, ["fuzz", "spikes/mls"]);
        assert_eq!(found[1].folder, test.path().join("spikes").join("mls"));
        assert_eq!(found[1].file("Cargo.lock"), "spikes/mls/Cargo.lock");
        assert!(!found[0].is_main());
        let main = Workspace::main(test.path());
        assert!(main.is_main());
        assert_eq!(main.file("Cargo.lock"), "Cargo.lock");
        // A repository with one workspace has no secondary ones.
        assert_eq!(secondary(&test.path().join("fuzz")), Ok(Vec::new()));
    }

    #[test]
    fn refuses_a_lockfile_named_in_another_letter_case() {
        let test = TestRepository::new();
        test.write("Cargo.lock", "");
        test.write("fuzz/CARGO.LOCK", "");
        test.commit("workspaces", 1_000_000_000);
        let problem = secondary(test.path()).unwrap_err();
        assert!(
            problem.starts_with("fuzz/CARGO.LOCK is named like Cargo.lock in another letter case"),
            "{problem}"
        );
    }

    /// Runs `check_manifests` on the repository of `test`, whose main workspace has the members `main` and whose secondary workspaces have the members `others`, and returns its result and what it reported.
    fn manifests(
        test: &TestRepository,
        main: &Members,
        others: &[&Members],
    ) -> (Result<(), String>, Vec<String>) {
        let found = secondary(test.path()).unwrap();
        let members: Vec<&Members> = std::iter::once(main)
            .chain(others.iter().copied())
            .collect();
        let mut lines = Vec::new();
        let result = check_manifests(test.path(), &found, &members, &mut |line: &str| {
            lines.push(line.to_owned());
        });
        (result, lines)
    }

    #[test]
    fn refuses_a_manifest_that_belongs_to_no_checked_workspace() {
        let test = TestRepository::new();
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "crates/own/Cargo.toml",
            "fuzz/Cargo.toml",
            "fuzz/Cargo.lock",
        ] {
            test.write(path, "");
        }
        test.commit("workspaces", 1_000_000_000);
        let main = Members::of(&[("own", &test.path().join("crates").join("own"))]);
        let fuzz = Members::of(&[("bayan-fuzz", &test.path().join("fuzz"))]);
        let (result, lines) = manifests(&test, &main, &[&fuzz]);
        assert_eq!(result, Ok(()));
        assert_eq!(
            lines,
            [
                "each of the 3 Cargo.toml files that Git tracks is the root manifest of a checked workspace or the manifest of one of its members"
            ]
        );
        // Cargo builds each of these as a workspace of its own: a folder with a `[workspace]` table and no committed lockfile (with a range requirement that no check would see), a package in a folder that the main workspace excludes, which needs no `[workspace]` table for that, and a manifest named in another letter case, which Windows and macOS open as Cargo.toml.
        test.write(
            "tools/demo/Cargo.toml",
            "[workspace]\n\n[package]\nname = \"demo\"\n\n[dependencies]\nitoa = \"1\"\n",
        );
        test.write(
            "examples/excluded/Cargo.toml",
            "[package]\nname = \"excluded\"\n",
        );
        test.write("tools/other/CARGO.TOML", "[workspace]\n");
        test.commit("outside", 1_000_000_100);
        let (result, lines) = manifests(&test, &main, &[&fuzz]);
        assert!(lines.is_empty(), "{lines:?}");
        let problem = result.unwrap_err();
        assert!(
            problem.starts_with(
                "Git tracks 3 Cargo.toml file(s) that belong to no workspace these checks read"
            ),
            "{problem}"
        );
        assert!(
            problem.contains(
                "\n  - examples/excluded/Cargo.toml\n  - tools/demo/Cargo.toml\n  - tools/other/CARGO.TOML\n"
            ),
            "{problem}"
        );
        assert!(
            problem.ends_with("Commit its Cargo.lock (its folder is then checked as a secondary workspace), make it a member of a checked workspace, or remove it."),
            "{problem}"
        );
        // A manifest that Git does not track is not part of what a change can merge.
        test.write("scratch/Cargo.toml", "[workspace]\n");
        let problem = manifests(&test, &main, &[&fuzz]).0.unwrap_err();
        assert!(problem.starts_with("Git tracks 3 "), "{problem}");
    }

    #[test]
    fn refuses_a_secondary_workspace_whose_lockfile_was_removed() {
        let test = TestRepository::new();
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "fuzz/Cargo.toml",
            "fuzz/Cargo.lock",
        ] {
            test.write(path, "");
        }
        test.commit("workspaces", 1_000_000_000);
        std::fs::remove_file(test.path().join("fuzz").join("Cargo.lock")).unwrap();
        test.commit("remove fuzz/Cargo.lock", 1_000_000_100);
        // Without its lockfile, fuzz/ is no longer found as a secondary workspace, but Cargo still builds it as a workspace of its own.
        assert_eq!(secondary(test.path()), Ok(Vec::new()));
        let (result, lines) = manifests(&test, &Members::of(&[]), &[]);
        assert!(lines.is_empty(), "{lines:?}");
        let problem = result.unwrap_err();
        assert!(
            problem.starts_with("Git tracks 1 Cargo.toml file(s)"),
            "{problem}"
        );
        assert!(problem.contains("\n  - fuzz/Cargo.toml\n"), "{problem}");
    }

    #[test]
    fn names_the_files_of_a_secondary_workspace_in_messages() {
        let fuzz = Workspace {
            folder: std::env::temp_dir().join("fuzz"),
            name: "fuzz".to_owned(),
        };
        assert_eq!(
            fuzz.locate("Cargo.lock", "Cargo.lock line 7: `x`".to_owned()),
            "fuzz/Cargo.lock line 7: `x`"
        );
        assert_eq!(
            fuzz.locate("Cargo.lock", "cannot read it".to_owned()),
            "cannot read it"
        );
        let main = Workspace::main(&std::env::temp_dir());
        assert_eq!(
            main.locate("Cargo.lock", "Cargo.lock line 7: `x`".to_owned()),
            "Cargo.lock line 7: `x`"
        );
    }

    #[test]
    fn checks_that_cargo_reads_the_folder_as_a_workspace_root() {
        let root = std::env::temp_dir();
        let fuzz = Workspace {
            folder: root.join("fuzz"),
            name: "fuzz".to_owned(),
        };
        let rooted_at = |folder: &Path| {
            json::parse(&format!(
                "{{\"workspace_root\":{}}}",
                json_string(&folder.to_string_lossy())
            ))
            .unwrap()
        };
        assert_eq!(check_root(&fuzz, &rooted_at(&root.join("fuzz"))), Ok(()));
        // A Cargo.lock in a folder of the main workspace, such as a member's: Cargo uses the main workspace's lockfile there.
        let problem = check_root(&fuzz, &rooted_at(&root)).unwrap_err();
        assert!(
            problem.starts_with("Cargo never uses fuzz/Cargo.lock"),
            "{problem}"
        );
        assert!(check_root(&fuzz, &json::parse("{}").unwrap()).is_err());
    }

    #[test]
    fn reads_the_members_from_cargo_metadata() {
        let root = std::env::temp_dir().join("bayandocs-xtask-members");
        let own = root.join("crates").join("own");
        let cache = std::env::temp_dir()
            .join("bayandocs-xtask-cache")
            .join("serde-1.0.229");
        let package = |id: &str, name: &str, folder: &Path| {
            format!(
                "{{\"id\":{},\"name\":{},\"manifest_path\":{}}}",
                json_string(id),
                json_string(name),
                json_string(&folder.join("Cargo.toml").to_string_lossy())
            )
        };
        let metadata = json::parse(&format!(
            "{{\"packages\":[{},{}],\"workspace_members\":[\"path+file:///own#0.1.0\"]}}",
            package("path+file:///own#0.1.0", "own", &own),
            package(
                "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.229",
                "serde",
                &cache
            ),
        ))
        .unwrap();
        let members = Members::from_metadata(&metadata).unwrap();
        assert_eq!(members.count(), 1);
        assert_eq!(members.at(&own), Some("own"));
        assert_eq!(members.at(&cache), None);
        assert!(Members::from_metadata(&json::parse("{\"packages\":[]}").unwrap()).is_err());
    }
}
