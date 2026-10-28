//! `cargo xtask check-lockfile-age`: every package version that a change adds to a `Cargo.lock` must have been published on crates.io at least 24 hours before the commit that added it, and before now (ADR-0017 rule 4), and must be exactly what crates.io published.
//!
//! Cargo enforces the minimum age itself only from Rust 1.100 (`global-min-publish-age` in `.cargo/config.toml`), and even then only when it resolves new versions: it never re-checks versions that are already in `Cargo.lock`. This check closes that gap for every pull request, in every workspace of the repository (`workspace.rs`): the main workspace at the root and each secondary workspace below it, such as bayan-core's `fuzz/`, each with its own `Cargo.lock`. It also fails when Git tracks a `Cargo.toml` that belongs to none of them, which Cargo could build from a lockfile that was never committed (`workspace::check_manifests`). In each workspace:
//!
//! 1. It makes sure that the `Cargo.lock` is what Cargo builds: the packages Cargo resolves (`cargo metadata`, with every feature of the workspace's members, as Cargo resolves the lockfile) must be exactly those of `Cargo.lock`, and every package outside the workspace must come from a registry or a Git repository, not from a folder. Otherwise a `lockfile-path` setting could make Cargo read another lockfile, and a `[patch]`, `[replace]` or `paths` override, or vendored sources, could build code from a folder in place of a crate that the lockfile lists, without either showing up below. The one exception is for secondary workspaces, which may build members of the main workspace from their folders: that code is in this repository, its own dependencies are checked with the main workspace, and their versions in the secondary `Cargo.lock` are checked below with that lockfile.
//! 2. It finds the merge base of `HEAD` and the branch the change will be merged into (`origin/<GITHUB_BASE_REF>` in a pull request on GitHub Actions, otherwise `origin/main`, or the branch named with `--base`).
//! 3. It compares each `Cargo.lock` of the working tree with its own version at the merge base. A package version is added or changed if its name, version, source or checksum is not in the old file; if the file did not exist at the merge base, every version in it is. When nothing was added, nothing is looked up on crates.io.
//! 4. It finds the first commit since the merge base whose `Cargo.lock` contains each added version, and the time that commit was made (`judge`). A version that only the working tree contains (not committed yet) is measured against now.
//! 5. It looks up each added crates.io version in the crates.io index (`crates_io.rs`). It fails if the version was published less than 24 hours before the commit that added it, or less than 24 hours before now, and if the checksum in `Cargo.lock` is not the one crates.io publishes. Measuring against now as well means that no commit date, however it is set, can make a young version pass.
//!
//! A version from a Git repository has no crates.io publish time and no checksum that crates.io published, so this check cannot judge it, and it refuses every one that a change adds. `deny.toml` allows no Git repository either, but cargo-deny runs only after the build, when the dependency's build script, procedural macros and tests have already run; a work package that needs a Git dependency must first teach this check to judge its commit. The workspaces' own crates, and the members of the main workspace that a secondary workspace builds, are not checked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use super::cargo;
use super::cargo_lock::{self, Package};
use super::crates_io::{CratesIo, Curl};
use super::git::{self, Commit, Repository};
use super::json::{self, Value};
use super::time;
use super::workspace::{self, Members, Workspace};

/// The source string of crates.io in `Cargo.lock`. Cargo writes it for crates.io even when it uses the sparse protocol.
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// The other spelling of crates.io, for Cargo versions that record the sparse index itself.
const CRATES_IO_SPARSE: &str = "sparse+https://index.crates.io/";

/// The least time between two requests to the crates.io API (<https://crates.io/data-access>).
const API_INTERVAL: Duration = Duration::from_secs(1);

/// Runs the check for every workspace of the repository at `root`, comparing with `base` (or the default base branch), and checks that every `Cargo.toml` that Git tracks belongs to one of them (`workspace::check_manifests`), reporting progress through `report`. A workspace that fails does not stop the others from being checked; the problems of all of them are returned together.
pub fn check(root: &Path, base: Option<&str>, report: &mut dyn FnMut(&str)) -> Result<(), String> {
    let base = base.map_or_else(git::default_base, str::to_owned);
    let now = time::now()?;
    // Fails once, before anything else runs, without Git or in a shallow clone; each workspace opens the repository again from its own folder.
    Repository::open(root)?;
    let main = Workspace::main(root);
    let secondary = workspace::secondary(root)?;
    let main_metadata = metadata(&main)?;
    let main_members = Members::from_metadata(&main_metadata)?;
    let mut curl = Curl::new(user_agent());
    let mut shared = Shared {
        root,
        base: &base,
        now,
        crates_io: CratesIo::new(&mut curl, API_INTERVAL),
    };
    let mut failures = Vec::new();
    if let Err(problem) = check_workspace(&mut shared, &main, &main_metadata, None, report) {
        failures.push(problem);
    }
    let mut secondary_members = Vec::new();
    for workspace in &secondary {
        let result = metadata(workspace).and_then(|metadata| {
            secondary_members.push(Members::from_metadata(&metadata)?);
            check_workspace(
                &mut shared,
                workspace,
                &metadata,
                Some(&main_members),
                report,
            )
        });
        if let Err(problem) = result {
            failures.push(problem);
        }
    }
    // The members of a workspace that Cargo cannot describe are not known, so the manifests are judged only once every workspace can be read; until then, that workspace's problem fails the check.
    if secondary_members.len() == secondary.len() {
        let members: Vec<&Members> = std::iter::once(&main_members)
            .chain(&secondary_members)
            .collect();
        if let Err(problem) = workspace::check_manifests(root, &secondary, &members, report) {
            failures.push(problem);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n\n"))
    }
}

/// What the checks of all workspaces share.
struct Shared<'a, 'b> {
    /// The repository's root, where the main workspace is.
    root: &'a Path,
    /// The branch the change will be merged into.
    base: &'a str,
    /// The current time.
    now: i64,
    /// The lookups on crates.io, so that each crate's index file is fetched once, whichever lockfiles add its versions.
    crates_io: CratesIo<'b>,
}

/// Cargo's description of `workspace` and of every package it builds, with every feature of the workspace's members switched on, as Cargo resolves `Cargo.lock`, after checking that Cargo reads its folder as the root of a workspace. Without `--all-features`, a package that only an optional feature of a member brings in would be in `Cargo.lock` but not in the resolve.
fn metadata(workspace: &Workspace) -> Result<Value, String> {
    let metadata = json::parse(&cargo::metadata(&workspace.folder, &["--all-features"])?)?;
    workspace::check_root(workspace, &metadata)?;
    Ok(metadata)
}

/// Checks `workspace`, which `metadata` describes: that Cargo builds exactly its `Cargo.lock`, and that every package version the change adds to it is old enough and what crates.io published. `main` holds the members of the main workspace when `workspace` is a secondary one.
fn check_workspace(
    shared: &mut Shared<'_, '_>,
    workspace: &Workspace,
    metadata: &Value,
    main: Option<&Members>,
    report: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let lockfile = workspace.file("Cargo.lock");
    let repository = Repository::open(&workspace.folder)?;
    let path = workspace.folder.join("Cargo.lock");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let packages =
        cargo_lock::parse(&text).map_err(|problem| workspace.locate("Cargo.lock", problem))?;
    let from_main = check_resolve(shared.root, &lockfile, &packages, metadata, main)?;
    let from_main = if from_main.is_empty() {
        String::new()
    } else {
        format!(
            ", except the members of the main workspace that it builds from their folders ({})",
            from_main.into_iter().collect::<Vec<_>>().join(", ")
        )
    };
    report(&format!(
        "Cargo builds exactly the {} packages of {lockfile}, and every package outside the workspace comes from a registry or a Git repository, not from a folder{from_main}",
        packages.len()
    ));
    let added = added_versions(&repository, &lockfile, &packages, shared.base, report)?;
    if added.is_empty() {
        return Ok(());
    }
    evaluate(&lockfile, &added, shared.now, &mut shared.crates_io, report)
}

/// The User-Agent of the requests to crates.io, which asks every tool to name itself and a way to reach its maintainers.
fn user_agent() -> String {
    format!(
        "BayanDocs-xtask/{} (check-lockfile-age; +{})",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_REPOSITORY")
    )
}

/// A package as Cargo resolves it: name, version and source.
type Resolved = (String, String, Option<String>);

/// Checks that the packages Cargo resolves (`metadata`, the output of `cargo metadata` without `--no-deps`) are exactly those of `locked`, read from the lockfile `lockfile`, and that every package that is not a workspace member comes from a registry or a Git repository whose code Cargo does not read from a folder of the repository at `root`. A secondary workspace may also build members of the main workspace, `main`, from their folders; their names are returned.
fn check_resolve(
    root: &Path,
    lockfile: &str,
    locked: &[Package],
    metadata: &Value,
    main: Option<&Members>,
) -> Result<BTreeSet<String>, String> {
    let members: BTreeSet<&str> = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .ok_or("`cargo metadata` lists no workspace members")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let packages = metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or("`cargo metadata` lists no packages")?;
    let repository = workspace::canonical(root);
    let mut problems = Vec::new();
    let mut from_main = BTreeSet::new();
    let mut resolved: BTreeSet<Resolved> = BTreeSet::new();
    for package in packages {
        let (Some(id), Some(name), Some(version), Some(manifest)) = (
            package.string("id"),
            package.string("name"),
            package.string("version"),
            package.string("manifest_path"),
        ) else {
            return Err("`cargo metadata` describes a package without an id, a name, a version or a manifest path".to_owned());
        };
        let source = package.string("source").map(str::to_owned);
        if !members.contains(id) {
            let folder =
                workspace::canonical(Path::new(manifest).parent().unwrap_or(Path::new(manifest)));
            match (&source, main) {
                (None, Some(main)) => match main.at(&folder) {
                    Some(member) => {
                        from_main.insert(member.to_owned());
                    }
                    None => problems.push(format!(
                        "{name} {version} is neither a member of this workspace nor of the main workspace, but Cargo builds it from the folder {}, through a path dependency or a `[patch]`, `[replace]` or `paths` override; a secondary workspace may build only members of the main workspace from their folders, dependencies come only from crates.io (ADR-0017), and code in any other folder passes none of the checks",
                        folder.display()
                    )),
                },
                (None, None) => problems.push(format!(
                    "{name} {version} is not a workspace member, but Cargo builds it from the folder {}, through a `[patch]`, `[replace]` or `paths` override; dependencies come only from crates.io (ADR-0017), and code in such a folder passes none of the checks",
                    folder.display()
                )),
                (Some(source), _) if folder.starts_with(&repository) => problems.push(format!(
                    "{name} {version} is listed as coming from {source}, but Cargo reads its code from {} in this repository (vendored sources or a `paths` override), which the checks of {source} do not cover",
                    folder.display()
                )),
                (Some(_), _) => {}
            }
        }
        resolved.insert((name.to_owned(), version.to_owned(), source));
    }
    let locked: BTreeSet<Resolved> = locked
        .iter()
        .map(|package| {
            (
                package.name.clone(),
                package.version.clone(),
                package.source.clone(),
            )
        })
        .collect();
    let describe = |(name, version, source): &Resolved| {
        format!(
            "{name} {version} ({})",
            source.as_deref().unwrap_or("a workspace member")
        )
    };
    for package in resolved.difference(&locked) {
        problems.push(format!(
            "Cargo resolves {}, which {lockfile} does not list",
            describe(package)
        ));
    }
    for package in locked.difference(&resolved) {
        problems.push(format!(
            "{lockfile} lists {}, which Cargo does not resolve",
            describe(package)
        ));
    }
    if problems.is_empty() {
        Ok(from_main)
    } else {
        Err(format!(
            "Cargo does not build exactly what {lockfile} lists, so its age cannot be checked from {lockfile}:\n  - {}\nCargo reads Cargo.lock at the root of the workspace unless `resolver.lockfile-path` (in a .cargo/config.toml) or CARGO_RESOLVER_LOCKFILE_PATH points elsewhere, and builds a dependency from a folder when `[patch]`, `[replace]`, `paths` or a source replacement says so; remove such settings.",
            problems.join("\n  - ")
        ))
    }
}

/// A package version that the change adds to a `Cargo.lock`, and the commit that added it (`None` if it is not committed yet).
#[derive(Debug, PartialEq, Eq)]
pub struct Added {
    /// The package version.
    pub package: Package,
    /// The first commit since the merge base whose `Cargo.lock` contains it.
    pub commit: Option<Commit>,
}

/// The package versions of `current` (the working tree's `Cargo.lock` of the workspace that `repository` opened, called `lockfile` in messages) that the same `Cargo.lock` at the merge base with `base` does not contain, each with the commit that added it.
fn added_versions(
    repository: &Repository,
    lockfile: &str,
    current: &[Package],
    base: &str,
    report: &mut dyn FnMut(&str),
) -> Result<Vec<Added>, String> {
    let merge_base = repository.merge_base(base)?;
    let before: BTreeSet<Package> = match repository.file_at(&merge_base, "Cargo.lock")? {
        Some(text) => cargo_lock::parse(&text)
            .map_err(|problem| format!("{lockfile} at {}: {problem}", merge_base.short()))?
            .into_iter()
            .collect(),
        None => {
            report(&format!(
                "{lockfile} did not exist at the merge base with {base} ({}), so every package version in it is new",
                merge_base.short()
            ));
            BTreeSet::new()
        }
    };
    let now: BTreeSet<&Package> = current.iter().collect();
    let new: Vec<&Package> = now
        .into_iter()
        .filter(|package| !before.contains(*package))
        .collect();
    if new.is_empty() {
        report(&format!(
            "{lockfile} adds or changes no package version since the merge base with {base} ({}), so there is nothing to look up",
            merge_base.short()
        ));
        return Ok(Vec::new());
    }
    report(&format!(
        "{lockfile} adds or changes {} package version(s) since the merge base with {base} ({})",
        new.len(),
        merge_base.short()
    ));
    let mut found: BTreeMap<&Package, Commit> = BTreeMap::new();
    for commit in repository.commits_since(&merge_base)? {
        if found.len() == new.len() {
            break;
        }
        let Some(text) = repository.file_at(&commit, "Cargo.lock")? else {
            continue;
        };
        let packages: BTreeSet<Package> = cargo_lock::parse(&text)
            .map_err(|problem| format!("{lockfile} at {}: {problem}", commit.short()))?
            .into_iter()
            .collect();
        for package in &new {
            if !found.contains_key(package) && packages.contains(*package) {
                found.insert(package, commit.clone());
            }
        }
    }
    Ok(new
        .into_iter()
        .map(|package| Added {
            package: package.clone(),
            commit: found.get(package).cloned(),
        })
        .collect())
}

/// Looks up each version that the change adds to the lockfile `lockfile` on crates.io and checks its age and checksum; `now` is the current time.
fn evaluate(
    lockfile: &str,
    added: &[Added],
    now: i64,
    crates_io: &mut CratesIo<'_>,
    report: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let mut problems = Vec::new();
    for item in added {
        let package = &item.package;
        let label = package.label();
        match package.source.as_deref() {
            // The workspace's own crates, and the members of the main workspace that a secondary workspace builds, whose code is in this repository (`check_resolve` makes sure nothing else lacks a source).
            None => {}
            Some(CRATES_IO | CRATES_IO_SPARSE) => {
                let published = match crates_io.published(&package.name, &package.version) {
                    Ok(published) => published,
                    Err(problem) => {
                        problems.push(format!("{label}: cannot find it on crates.io: {problem}"));
                        continue;
                    }
                };
                if package.checksum.as_deref() != Some(published.checksum.as_str()) {
                    report(&format!("WRONG CHECKSUM: {label}"));
                    problems.push(format!(
                        "{label}: {lockfile} records the checksum {}, but crates.io published {}; {lockfile} must record exactly the crate file crates.io serves, so regenerate it with Cargo",
                        package.checksum.as_deref().unwrap_or("none"),
                        published.checksum
                    ));
                }
                match judge(published.time, item.commit.as_ref(), now) {
                    Ok(line) => report(&format!("ok: {label}: {line}")),
                    Err(line) => {
                        report(&format!("TOO NEW: {label}: {line}"));
                        problems.push(format!("{label}: {line}"));
                    }
                }
            }
            Some(source) if source.starts_with("git+") => {
                report(&format!("REFUSED: {label}: from a Git repository"));
                problems.push(format!(
                    "{label} comes from a Git repository ({source}), which has no crates.io publish time or checksum to check (ADR-0017 rule 4); this check refuses every Git dependency until a work package teaches it to judge one"
                ));
            }
            Some(source) => problems.push(format!(
                "{label} comes from `{source}`; dependencies come only from crates.io (ADR-0017)"
            )),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} problem(s) with package versions in {lockfile}: the 24-hour minimum age of ADR-0017 rule 4, or what crates.io published:\n  - {}\nPin versions that were published at least 24 hours before your commit (and before now). If a security fix is younger than that, follow the security-alert procedure in the docs repository's developer/dependency-update-runbook.md instead of installing it early.",
            problems.len(),
            problems.join("\n  - ")
        ))
    }
}

/// Whether a version published at `published` was at least a day old when `commit` added it (or now, if it is not committed yet) and is at least a day old now, described for people either way.
///
/// A commit has two times: Git keeps the author time when the commit is amended or rebased, and gives it a new committer time. The earlier of the two counts, so that rewriting a commit cannot make its change look younger. An author time before the version was published shows that the commit was rewritten after it was first written, so then the committer time, when the commit was last made, is the earliest the version can have been added. A commit whose time is still before the publish time, or after now, cannot be right, so then the version is measured against now: no commit date can make a young version pass.
fn judge(published: i64, commit: Option<&Commit>, now: i64) -> Result<String, String> {
    let (reference, when) = match commit {
        None => (now, "now (it is not committed yet)".to_owned()),
        Some(commit) => {
            let made = if commit.author >= published {
                commit.time()
            } else {
                commit.committer
            };
            let dated = format!("{}, {}", commit.short(), time::display(made));
            if made > now {
                (
                    now,
                    format!("now (its commit {dated} is dated in the future)"),
                )
            } else if made < published {
                (
                    now,
                    format!("now (its commit {dated} is dated before the version was published)"),
                )
            } else {
                (made, format!("it was added (commit {dated})"))
            }
        }
    };
    let age = reference - published;
    let published_at = time::display(published);
    if age >= time::DAY {
        Ok(format!(
            "published {published_at}, {} before {when}",
            time::duration(age.unsigned_abs())
        ))
    } else if age >= 0 {
        Err(format!(
            "published {published_at}, only {} before {when}; it may be added from {} on",
            time::duration(age.unsigned_abs()),
            time::display(published + time::DAY)
        ))
    } else {
        Err(format!(
            "published {published_at}, {} after {when}; it may be added from {} on",
            time::duration(age.unsigned_abs()),
            time::display(published + time::DAY)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::super::crates_io::testing::Recorded;
    use super::super::git::testing::TestRepository;
    use super::*;

    /// When the fixtures were recorded (fixtures/README.md): 2026-10-07 02:38:23 UTC.
    const RECORDED_AT: i64 = 1_791_340_703;

    fn time(text: &str) -> i64 {
        time::parse(text).unwrap()
    }

    /// A `Cargo.lock` of the workspace member `app` (as in `metadata` and `locked`) and the given crates.io packages (name, version, checksum).
    fn lockfile(packages: &[(&str, &str, &str)]) -> String {
        let mut text = String::from(
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\ndependencies = [\n",
        );
        for (name, _, _) in packages {
            text.push_str(&format!(" \"{name}\",\n"));
        }
        text.push_str("]\n");
        for (name, version, checksum) in packages {
            text.push_str(&format!(
                "\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"{CRATES_IO}\"\nchecksum = \"{checksum}\"\n"
            ));
        }
        text
    }

    const HYPER_1_11_1: (&str, &str, &str) = (
        "hyper",
        "1.11.1",
        "27b501faa50e7a26c3d3560ca625132f4078a17771f4810baf70475ae48cbe43",
    );
    const HYPER_1_12_0: (&str, &str, &str) = (
        "hyper",
        "1.12.0",
        "2c3e324da4c95177d6291d4c8730197c0d1822f8a9766814a4a44fa5ab797c9c",
    );
    const ZEROCOPY_0_8_59: (&str, &str, &str) = (
        "zerocopy",
        "0.8.59",
        "6df92bf3d9227be3d53173901ddbffac2babc27ae50f397776ffd6dc33f800cb",
    );
    const ZEROCOPY_0_8_60: (&str, &str, &str) = (
        "zerocopy",
        "0.8.60",
        "6400fc4bb7426f6faac3fb07566122bb18622e3022bd2c36426a92ed489b1dbe",
    );

    /// Runs the check on a repository whose `main` holds `base` and whose current branch made `commits` (lockfile text and commit time, in order), with `working` as the uncommitted `Cargo.lock` if given, answering from the recorded fixtures at time `now`.
    fn run(
        base: &str,
        commits: &[(&str, i64)],
        working: Option<&str>,
        now: i64,
    ) -> (Result<(), String>, Vec<String>, Vec<String>) {
        run_in("", &[("Cargo.lock", base)], commits, working, now)
    }

    /// Runs the check of the workspace in the folder `folder` (empty for the main workspace) on a repository whose `main` holds the files `base` (path and text), and whose current branch made `commits` (the text of that workspace's `Cargo.lock` and the commit time, in order), with `working` as its uncommitted `Cargo.lock` if given, answering from the recorded fixtures at time `now`.
    fn run_in(
        folder: &str,
        base: &[(&str, &str)],
        commits: &[(&str, i64)],
        working: Option<&str>,
        now: i64,
    ) -> (Result<(), String>, Vec<String>, Vec<String>) {
        let test = TestRepository::new();
        for (path, text) in base {
            test.write(path, text);
        }
        test.commit("base", time("2026-09-01T00:00:00Z"));
        test.git(&["checkout", "--quiet", "-b", "change"], None);
        let workspace = if folder.is_empty() {
            Workspace::main(test.path())
        } else {
            Workspace {
                folder: test.path().join(folder),
                name: folder.to_owned(),
            }
        };
        let lockfile = workspace.file("Cargo.lock");
        for (text, at) in commits {
            test.write(&lockfile, text);
            test.commit("change", *at);
        }
        if let Some(text) = working {
            test.write(&lockfile, text);
        }
        let repository = Repository::open(&workspace.folder).unwrap();
        let current = std::fs::read_to_string(test.path().join(&lockfile)).unwrap();
        let current = cargo_lock::parse(&current).unwrap();
        let mut lines = Vec::new();
        let mut report = |line: &str| lines.push(line.to_owned());
        let added = added_versions(&repository, &lockfile, &current, "main", &mut report).unwrap();
        let mut recorded = Recorded::crates_io();
        let result = {
            let mut crates_io = CratesIo::new(&mut recorded, Duration::ZERO);
            evaluate(&lockfile, &added, now, &mut crates_io, &mut report)
        };
        (result, lines, recorded.requests)
    }

    #[test]
    fn passes_an_unchanged_lockfile_without_using_the_network() {
        let base = lockfile(&[HYPER_1_11_1]);
        let (result, lines, requests) = run(
            &base,
            &[(&base, time("2026-10-07T01:00:00Z"))],
            None,
            RECORDED_AT,
        );
        assert_eq!(result, Ok(()));
        assert!(requests.is_empty(), "{requests:?}");
        assert!(
            lines[0].contains("adds or changes no package version"),
            "{lines:?}"
        );
    }

    #[test]
    fn fails_a_version_published_less_than_a_day_before_its_commit() {
        // The sample diff of AC-2: hyper 1.11.1 to 1.12.0 (published 2026-10-06 15:57 UTC, 10 h 41 min before the recording) and zerocopy 0.8.59 to 0.8.60 (published 2026-10-05 23:02 UTC, 27 h 36 min before it).
        let base = lockfile(&[HYPER_1_11_1, ZEROCOPY_0_8_59]);
        let change = lockfile(&[HYPER_1_12_0, ZEROCOPY_0_8_60]);
        let (result, lines, requests) = run(&base, &[(&change, RECORDED_AT)], None, RECORDED_AT);
        let problem = result.unwrap_err();
        assert!(
            problem.starts_with("1 problem(s) with package versions in Cargo.lock"),
            "{problem}"
        );
        assert!(
            problem.contains(
                "hyper 1.12.0: published 2026-10-06 15:57 UTC, only 10 h 41 min before it was added"
            ),
            "{problem}"
        );
        assert!(
            problem.contains("it may be added from 2026-10-07 15:57 UTC on"),
            "{problem}"
        );
        assert!(!problem.contains("zerocopy"), "{problem}");
        assert!(
            lines.iter().any(|line| line.starts_with(
                "ok: zerocopy 0.8.60: published 2026-10-05 23:02 UTC, 1 d 3 h before it was added"
            )),
            "{lines:?}"
        );
        assert_eq!(
            requests,
            [
                "https://index.crates.io/hy/pe/hyper",
                "https://index.crates.io/ze/ro/zerocopy"
            ]
        );
    }

    #[test]
    fn measures_against_the_first_commit_that_added_a_version() {
        // hyper 1.12.0 was added a day after its release, then the lockfile changed again later: the version is old enough.
        let base = lockfile(&[HYPER_1_11_1, ZEROCOPY_0_8_59]);
        let first = lockfile(&[HYPER_1_12_0, ZEROCOPY_0_8_59]);
        let second = lockfile(&[HYPER_1_12_0, ZEROCOPY_0_8_60]);
        let day_later = time("2026-10-07T16:00:00Z");
        let (result, lines, _) = run(
            &base,
            &[(&first, day_later), (&second, day_later + 60)],
            None,
            day_later + 120,
        );
        assert_eq!(result, Ok(()), "{lines:?}");
        // The same versions, but the first commit was made too early: a later commit does not make them old enough.
        let early = time("2026-10-06T20:00:00Z");
        let (result, _, _) = run(
            &base,
            &[(&first, early), (&second, day_later)],
            None,
            day_later + 120,
        );
        assert!(result.unwrap_err().contains(
            "hyper 1.12.0: published 2026-10-06 15:57 UTC, only 4 h 2 min before it was added"
        ));
    }

    #[test]
    fn measures_uncommitted_versions_against_now() {
        let base = lockfile(&[HYPER_1_11_1]);
        let working = lockfile(&[HYPER_1_12_0]);
        let (result, _, _) = run(&base, &[], Some(&working), RECORDED_AT);
        assert!(
            result
                .unwrap_err()
                .contains("before now (it is not committed yet)")
        );
        let (result, _, _) = run(&base, &[], Some(&working), time("2026-10-07T16:00:00Z"));
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn measures_against_now_when_a_commit_claims_a_later_time() {
        // A commit dated in the future cannot make a version look old enough.
        let base = lockfile(&[HYPER_1_11_1]);
        let change = lockfile(&[HYPER_1_12_0]);
        let (result, _, _) = run(
            &base,
            &[(&change, time("2026-10-09T00:00:00Z"))],
            None,
            RECORDED_AT,
        );
        assert!(result.unwrap_err().contains("is dated in the future"));
    }

    #[test]
    fn fails_a_checksum_that_crates_io_did_not_publish() {
        // zerocopy 0.8.59 is old enough, but its checksum in the lockfile is not the one crates.io published.
        let base = lockfile(&[]);
        let change = lockfile(&[("zerocopy", "0.8.59", &"0".repeat(64))]);
        let (result, lines, _) = run(&base, &[(&change, RECORDED_AT)], None, RECORDED_AT);
        let problem = result.unwrap_err();
        assert!(
            problem.contains("zerocopy 0.8.59: Cargo.lock records the checksum 0000"),
            "{problem}"
        );
        assert!(problem.contains(&format!("but crates.io published {}", ZEROCOPY_0_8_59.2)));
        assert!(
            lines
                .iter()
                .any(|line| line == "WRONG CHECKSUM: zerocopy 0.8.59")
        );
        // A changed checksum of a version already in the lockfile counts as a change, so it is looked up too.
        let base = lockfile(&[ZEROCOPY_0_8_59]);
        let tampered = base.replace(ZEROCOPY_0_8_59.2, &"0".repeat(64));
        let (result, lines, _) = run(&base, &[(&tampered, RECORDED_AT)], None, RECORDED_AT);
        assert!(
            lines[0].contains("adds or changes 1 package version(s)"),
            "{lines:?}"
        );
        assert!(
            result
                .unwrap_err()
                .contains("Cargo.lock records the checksum")
        );
    }

    /// A Git dependency used to be reported as "not checked" and let through, so that only cargo-deny refused it, after the build had compiled it and run its build script (found in the review of X-003).
    #[test]
    fn refuses_git_and_other_sources() {
        let base = lockfile(&[]);
        let change = format!(
            "{base}\n[[package]]\nname = \"gitdep\"\nversion = \"0.1.0\"\nsource = \"git+https://example.org/gitdep?rev=eb23095592359c454a586d16d08b2bd3af44b551#eb23095592359c454a586d16d08b2bd3af44b551\"\n\n[[package]]\nname = \"elsewhere\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.org/index\"\nchecksum = \"{}\"\n",
            "1".repeat(64)
        );
        let (result, lines, requests) = run(&base, &[(&change, RECORDED_AT)], None, RECORDED_AT);
        assert!(
            lines
                .iter()
                .any(|line| line == "REFUSED: gitdep 0.1.0: from a Git repository"),
            "{lines:?}"
        );
        let error = result.unwrap_err();
        assert!(
            error.contains(
                "gitdep 0.1.0 comes from a Git repository (git+https://example.org/gitdep?rev="
            ),
            "{error}"
        );
        assert!(error.contains("elsewhere 1.0.0 comes from `registry+https://example.org/index`"));
        assert!(requests.is_empty());
    }

    #[test]
    fn fails_when_crates_io_does_not_know_a_version() {
        let base = lockfile(&[]);
        let change = lockfile(&[("hyper", "9.9.9", HYPER_1_12_0.2)]);
        let (result, _, _) = run(&base, &[(&change, RECORDED_AT)], None, RECORDED_AT);
        assert!(
            result
                .unwrap_err()
                .contains("hyper 9.9.9: cannot find it on crates.io")
        );
    }

    fn commit(author: i64, committer: i64) -> Commit {
        Commit {
            id: "1".repeat(40),
            author,
            committer,
        }
    }

    #[test]
    fn judges_ages_at_the_boundary() {
        let made = commit(2 * time::DAY, 2 * time::DAY);
        assert!(judge(time::DAY, Some(&made), 3 * time::DAY).is_ok());
        assert!(judge(time::DAY + 1, Some(&made), 3 * time::DAY).is_err());
        assert!(judge(time::DAY, None, 2 * time::DAY).is_ok());
        assert!(judge(time::DAY + 1, None, 2 * time::DAY).is_err());
    }

    #[test]
    fn judges_rewritten_commits_by_when_they_were_made() {
        let published = time("2026-09-16T00:00:00Z");
        let now = time("2026-10-01T00:00:00Z");
        // Written on 2026-09-15 and amended on 2026-09-20 to add a version published on 2026-09-16: the version was added on 2026-09-20 at the earliest, four days after it was published.
        let amended = commit(time("2026-09-15T00:00:00Z"), time("2026-09-20T00:00:00Z"));
        let line = judge(published, Some(&amended), now).unwrap();
        assert!(line.contains("4 d 0 h before it was added"), "{line}");
        // The same, amended only twelve hours after the version was published: too young when it was added.
        let early = commit(time("2026-09-15T00:00:00Z"), time("2026-09-16T12:00:00Z"));
        assert!(judge(published, Some(&early), now).is_err());
        // Both times before the version was published cannot be right: it is measured against now, which is old enough here...
        let impossible = commit(time("2026-09-14T00:00:00Z"), time("2026-09-15T00:00:00Z"));
        let line = judge(published, Some(&impossible), now).unwrap();
        assert!(
            line.contains("dated before the version was published"),
            "{line}"
        );
        // ...and too young here.
        let soon = time("2026-09-16T06:00:00Z");
        let problem = judge(published, Some(&impossible), soon).unwrap_err();
        assert!(problem.contains("only 6 h 0 min before now"), "{problem}");
    }

    #[test]
    fn names_itself_to_crates_io() {
        let agent = user_agent();
        assert!(agent.starts_with("BayanDocs-xtask/"), "{agent}");
        assert!(
            agent.contains("check-lockfile-age; +https://github.com/BayanDocs/"),
            "{agent}"
        );
    }

    /// `cargo metadata` output for a workspace with the member `app` and the given other packages (name, version, source or `null`, folder).
    fn metadata(root: &Path, others: &[(&str, &str, Option<&str>, &Path)]) -> Value {
        let quote = |text: &str| format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""));
        let package = |id: &str, name: &str, version: &str, source: Option<&str>, folder: &Path| {
            format!(
                "{{\"id\":{},\"name\":{},\"version\":{},\"source\":{},\"manifest_path\":{}}}",
                quote(id),
                quote(name),
                quote(version),
                source.map_or_else(|| "null".to_owned(), quote),
                quote(&folder.join("Cargo.toml").to_string_lossy())
            )
        };
        let mut packages = vec![package(
            "path+file:///app#0.1.0",
            "app",
            "0.1.0",
            None,
            &root.join("app"),
        )];
        for (name, version, source, folder) in others {
            packages.push(package(
                &format!(
                    "{}#{name}@{version}",
                    source.unwrap_or("path+file:///elsewhere")
                ),
                name,
                version,
                *source,
                folder,
            ));
        }
        json::parse(&format!(
            "{{\"packages\":[{}],\"workspace_members\":[\"path+file:///app#0.1.0\"]}}",
            packages.join(",")
        ))
        .unwrap()
    }

    fn locked(packages: &[(&str, &str, Option<&str>)]) -> Vec<Package> {
        let mut all = vec![Package {
            name: "app".to_owned(),
            version: "0.1.0".to_owned(),
            source: None,
            checksum: None,
        }];
        all.extend(packages.iter().map(|(name, version, source)| Package {
            name: (*name).to_owned(),
            version: (*version).to_owned(),
            source: source.map(str::to_owned),
            checksum: None,
        }));
        all
    }

    #[test]
    fn accepts_a_resolve_that_matches_the_lockfile() {
        let root = std::env::temp_dir().join("bayandocs-xtask-resolve");
        let cache = std::env::temp_dir()
            .join("bayandocs-xtask-cache")
            .join("hyper-1.11.1");
        let resolve = metadata(&root, &[("hyper", "1.11.1", Some(CRATES_IO), &cache)]);
        assert_eq!(
            check_resolve(
                &root,
                "Cargo.lock",
                &locked(&[("hyper", "1.11.1", Some(CRATES_IO))]),
                &resolve,
                None
            ),
            Ok(BTreeSet::new())
        );
    }

    /// Cargo reads another lockfile (`resolver.lockfile-path`), so it builds a version that `Cargo.lock` at the root does not list.
    #[test]
    fn fails_when_cargo_builds_another_lockfile() {
        let root = std::env::temp_dir().join("bayandocs-xtask-resolve");
        let cache = std::env::temp_dir()
            .join("bayandocs-xtask-cache")
            .join("hyper-1.12.0");
        let resolve = metadata(&root, &[("hyper", "1.12.0", Some(CRATES_IO), &cache)]);
        let problem = check_resolve(
            &root,
            "Cargo.lock",
            &locked(&[("hyper", "1.11.1", Some(CRATES_IO))]),
            &resolve,
            None,
        )
        .unwrap_err();
        assert!(
            problem.contains("Cargo resolves hyper 1.12.0 (registry+https://github.com/rust-lang/crates.io-index), which Cargo.lock does not list"),
            "{problem}"
        );
        assert!(
            problem.contains("Cargo.lock lists hyper 1.11.1"),
            "{problem}"
        );
        assert!(problem.contains("resolver.lockfile-path"), "{problem}");
    }

    /// A `[patch]` with a path: the lockfile lists the crate without a source, like a member, but it is not one.
    #[test]
    fn fails_when_a_crate_is_built_from_a_folder() {
        let root = std::env::temp_dir().join("bayandocs-xtask-resolve");
        let patched = root.join("third_party").join("cfg-if");
        let resolve = metadata(&root, &[("cfg-if", "1.0.5", None, &patched)]);
        let problem = check_resolve(
            &root,
            "Cargo.lock",
            &locked(&[("cfg-if", "1.0.5", None)]),
            &resolve,
            None,
        )
        .unwrap_err();
        assert!(
            problem.contains(
                "cfg-if 1.0.5 is not a workspace member, but Cargo builds it from the folder"
            ),
            "{problem}"
        );
    }

    /// Vendored sources or a `paths` override: the crate keeps its crates.io source, but Cargo reads its code from the repository.
    #[test]
    fn fails_when_a_registry_crate_is_read_from_the_repository() {
        let root = std::env::temp_dir().join("bayandocs-xtask-resolve");
        let vendored = root.join("vendor").join("hyper");
        let resolve = metadata(&root, &[("hyper", "1.11.1", Some(CRATES_IO), &vendored)]);
        let problem = check_resolve(
            &root,
            "Cargo.lock",
            &locked(&[("hyper", "1.11.1", Some(CRATES_IO))]),
            &resolve,
            None,
        )
        .unwrap_err();
        assert!(
            problem.contains("but Cargo reads its code from"),
            "{problem}"
        );
    }

    /// A secondary workspace, such as bayan-core's `fuzz/`, may build members of the main workspace from their folders; their dependencies are in its `Cargo.lock` and are checked with it (AC-1 of bayan-core#22).
    #[test]
    fn a_secondary_workspace_may_build_members_of_the_main_workspace() {
        let root = std::env::temp_dir().join("bayandocs-xtask-resolve");
        let own = root.join("crates").join("own");
        let main = Members::of(&[("own", &own)]);
        let cache = std::env::temp_dir()
            .join("bayandocs-xtask-cache")
            .join("hyper-1.11.1");
        let resolve = metadata(
            &root,
            &[
                ("own", "0.1.0", None, &own),
                ("hyper", "1.11.1", Some(CRATES_IO), &cache),
            ],
        );
        let lockfile = locked(&[("own", "0.1.0", None), ("hyper", "1.11.1", Some(CRATES_IO))]);
        assert_eq!(
            check_resolve(&root, "fuzz/Cargo.lock", &lockfile, &resolve, Some(&main)),
            Ok(BTreeSet::from(["own".to_owned()]))
        );
        // The main workspace's rule does not change: Cargo lists its members as its own, so it builds nothing else from a folder.
        let problem = check_resolve(&root, "Cargo.lock", &lockfile, &resolve, None).unwrap_err();
        assert!(
            problem.contains(
                "own 0.1.0 is not a workspace member, but Cargo builds it from the folder"
            ),
            "{problem}"
        );
    }

    /// A secondary workspace may build nothing else from a folder: neither a crate elsewhere in the repository nor one outside it, and no vendored crate (AC-1 of bayan-core#22).
    #[test]
    fn fails_when_a_secondary_workspace_builds_a_folder_outside_the_main_workspace() {
        let root = std::env::temp_dir().join("bayandocs-xtask-resolve");
        let main = Members::of(&[("own", &root.join("crates").join("own"))]);
        for folder in [
            root.join("elsewhere"),
            std::env::temp_dir().join("bayandocs-xtask-outside"),
        ] {
            let resolve = metadata(&root, &[("x", "1.0.0", None, &folder)]);
            let problem = check_resolve(
                &root,
                "fuzz/Cargo.lock",
                &locked(&[("x", "1.0.0", None)]),
                &resolve,
                Some(&main),
            )
            .unwrap_err();
            assert!(
                problem.starts_with("Cargo does not build exactly what fuzz/Cargo.lock lists"),
                "{problem}"
            );
            assert!(
                problem.contains("x 1.0.0 is neither a member of this workspace nor of the main workspace, but Cargo builds it from the folder"),
                "{problem}"
            );
        }
        let vendored = root.join("vendor").join("hyper");
        let resolve = metadata(&root, &[("hyper", "1.11.1", Some(CRATES_IO), &vendored)]);
        let problem = check_resolve(
            &root,
            "fuzz/Cargo.lock",
            &locked(&[("hyper", "1.11.1", Some(CRATES_IO))]),
            &resolve,
            Some(&main),
        )
        .unwrap_err();
        assert!(
            problem.contains("but Cargo reads its code from"),
            "{problem}"
        );
    }

    /// A secondary workspace's lockfile that did not exist at the merge base is checked in full: every package version in it is new (AC-1 of bayan-core#22).
    #[test]
    fn checks_a_secondary_lockfile_in_full_when_the_merge_base_has_none() {
        let main = lockfile(&[HYPER_1_11_1]);
        let fuzz = lockfile(&[HYPER_1_12_0, ZEROCOPY_0_8_60]);
        let (result, lines, requests) = run_in(
            "fuzz",
            &[("Cargo.lock", &main)],
            &[(&fuzz, RECORDED_AT)],
            None,
            RECORDED_AT,
        );
        assert!(
            lines[0].starts_with("fuzz/Cargo.lock did not exist at the merge base with main"),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("fuzz/Cargo.lock adds or changes 3 package version(s)"),
            "{lines:?}"
        );
        assert_eq!(
            requests,
            [
                "https://index.crates.io/hy/pe/hyper",
                "https://index.crates.io/ze/ro/zerocopy"
            ]
        );
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("ok: zerocopy 0.8.60: published")),
            "{lines:?}"
        );
        let problem = result.unwrap_err();
        assert!(
            problem.starts_with("1 problem(s) with package versions in fuzz/Cargo.lock"),
            "{problem}"
        );
        assert!(
            problem.contains(
                "hyper 1.12.0: published 2026-10-06 15:57 UTC, only 10 h 41 min before it was added"
            ),
            "{problem}"
        );
    }

    /// A secondary workspace's lockfile is compared with its own version at the merge base, not with the main workspace's lockfile, which may already list a version that it adds.
    #[test]
    fn compares_a_secondary_lockfile_with_its_own_version_at_the_merge_base() {
        let main = lockfile(&[HYPER_1_12_0]);
        let before = lockfile(&[HYPER_1_11_1, ZEROCOPY_0_8_60]);
        let after = lockfile(&[HYPER_1_12_0, ZEROCOPY_0_8_60]);
        let base = [
            ("Cargo.lock", main.as_str()),
            ("fuzz/Cargo.lock", before.as_str()),
        ];
        let (result, lines, requests) =
            run_in("fuzz", &base, &[(&after, RECORDED_AT)], None, RECORDED_AT);
        assert!(
            lines[0].starts_with("fuzz/Cargo.lock adds or changes 1 package version(s)"),
            "{lines:?}"
        );
        assert_eq!(requests, ["https://index.crates.io/hy/pe/hyper"]);
        assert!(result.unwrap_err().contains(
            "hyper 1.12.0: published 2026-10-06 15:57 UTC, only 10 h 41 min before it was added"
        ));
        // Unchanged, it has nothing to look up, whatever the main workspace's lockfile holds.
        let (result, lines, requests) =
            run_in("fuzz", &base, &[(&before, RECORDED_AT)], None, RECORDED_AT);
        assert_eq!(result, Ok(()));
        assert!(requests.is_empty(), "{requests:?}");
        assert!(
            lines[0].starts_with("fuzz/Cargo.lock adds or changes no package version"),
            "{lines:?}"
        );
    }

    /// `check_workspace` as `check` calls it, from Cargo's description of each workspace: for a secondary workspace, with the members of the main workspace, which it may build from their folders, and with the repository opened in its own folder, so that its `Cargo.lock` is compared with its own version at the merge base, although the main workspace's lockfile there already lists the version it adds; for the main workspace, without them.
    #[test]
    fn checks_each_workspace_against_its_own_lockfile() {
        let test = TestRepository::new();
        let root = test.path();
        // The fuzz workspace's lockfile also lists the member of the main workspace that it builds from its folder.
        let with_own =
            |text: String| format!("{text}\n[[package]]\nname = \"own\"\nversion = \"0.1.0\"\n");
        test.write("Cargo.lock", &lockfile(&[HYPER_1_12_0]));
        test.write("crates/own/Cargo.toml", "");
        test.write(
            "fuzz/Cargo.lock",
            &with_own(lockfile(&[HYPER_1_11_1, ZEROCOPY_0_8_60])),
        );
        let base = test.commit("base", time("2026-09-01T00:00:00Z"));
        test.git(&["checkout", "--quiet", "-b", "change"], None);
        test.write(
            "fuzz/Cargo.lock",
            &with_own(lockfile(&[HYPER_1_12_0, ZEROCOPY_0_8_60])),
        );
        test.commit("change", RECORDED_AT);
        let own = root.join("crates").join("own");
        let cache = std::env::temp_dir().join("bayandocs-xtask-cache");
        let hyper = cache.join("hyper-1.12.0");
        let zerocopy = cache.join("zerocopy-0.8.60");
        let main_metadata = metadata(root, &[("hyper", "1.12.0", Some(CRATES_IO), &hyper)]);
        let fuzz_metadata = metadata(
            &root.join("fuzz"),
            &[
                ("own", "0.1.0", None, &own),
                ("hyper", "1.12.0", Some(CRATES_IO), &hyper),
                ("zerocopy", "0.8.60", Some(CRATES_IO), &zerocopy),
            ],
        );
        let main_members = Members::of(&[("own", &own)]);
        let fuzz = Workspace {
            folder: root.join("fuzz"),
            name: "fuzz".to_owned(),
        };
        let mut recorded = Recorded::crates_io();
        let mut lines = Vec::new();
        let (main_result, fuzz_result) = {
            let mut shared = Shared {
                root,
                base: "main",
                now: RECORDED_AT,
                crates_io: CratesIo::new(&mut recorded, Duration::ZERO),
            };
            let mut report = |line: &str| lines.push(line.to_owned());
            let main_result = check_workspace(
                &mut shared,
                &Workspace::main(root),
                &main_metadata,
                None,
                &mut report,
            );
            let fuzz_result = check_workspace(
                &mut shared,
                &fuzz,
                &fuzz_metadata,
                Some(&main_members),
                &mut report,
            );
            (main_result, fuzz_result)
        };
        assert_eq!(main_result, Ok(()));
        let base = &base[..12];
        assert_eq!(
            lines[..4],
            [
                "Cargo builds exactly the 2 packages of Cargo.lock, and every package outside the workspace comes from a registry or a Git repository, not from a folder".to_owned(),
                format!(
                    "Cargo.lock adds or changes no package version since the merge base with main ({base}), so there is nothing to look up"
                ),
                "Cargo builds exactly the 4 packages of fuzz/Cargo.lock, and every package outside the workspace comes from a registry or a Git repository, not from a folder, except the members of the main workspace that it builds from their folders (own)".to_owned(),
                format!(
                    "fuzz/Cargo.lock adds or changes 1 package version(s) since the merge base with main ({base})"
                ),
            ]
        );
        assert!(lines[4].starts_with("TOO NEW: hyper 1.12.0"), "{lines:?}");
        assert_eq!(recorded.requests, ["https://index.crates.io/hy/pe/hyper"]);
        let problem = fuzz_result.unwrap_err();
        assert!(
            problem.starts_with("1 problem(s) with package versions in fuzz/Cargo.lock"),
            "{problem}"
        );
        assert!(
            problem.contains(
                "hyper 1.12.0: published 2026-10-06 15:57 UTC, only 10 h 41 min before it was added"
            ),
            "{problem}"
        );
    }
}
