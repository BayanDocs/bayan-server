//! `cargo xtask check-lockfile-age`: every package version that a change adds to `Cargo.lock` must have been published on crates.io at least 24 hours before the commit that added it, and before now (ADR-0017 rule 4).
//!
//! Cargo enforces the minimum age itself only from Rust 1.100 (`global-min-publish-age` in `.cargo/config.toml`), and even then only when it resolves new versions: it never re-checks versions that are already in `Cargo.lock`. This check closes that gap for every pull request:
//!
//! 1. It finds the merge base of `HEAD` and the branch the change will be merged into (`origin/<GITHUB_BASE_REF>` in a pull request on GitHub Actions, otherwise `origin/main`, or the branch named with `--base`).
//! 2. It compares the `Cargo.lock` of the working tree with the one at the merge base. A package version is added or changed if its name, version, source or checksum is not in the old file. When nothing was added, the check is done without using the network, so it works offline for unchanged lockfiles.
//! 3. It finds the first commit since the merge base whose `Cargo.lock` contains each added version, and takes the earlier of that commit's author and committer times. A version that only the working tree contains (not committed yet) is measured against now.
//! 4. It looks up when each added crates.io version was published (`crates_io.rs`) and fails if that was less than 24 hours before the commit that added it, or less than 24 hours before now. Measuring against now as well means that a commit dated in the future cannot make a young version pass.
//!
//! Versions from Git repositories have no crates.io publish time; they are listed as not checked, and the pull request that adds one states the age of its commit (cargo-deny allows only the repositories listed in `deny.toml`). The workspace's own crates are not checked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use super::cargo_lock::{self, Package};
use super::crates_io::{CratesIo, Curl};
use super::git::{self, Commit, Repository};
use super::time;

/// The source string of crates.io in `Cargo.lock`. Cargo writes it for crates.io even when it uses the sparse protocol.
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// The other spelling of crates.io, for Cargo versions that record the sparse index itself.
const CRATES_IO_SPARSE: &str = "sparse+https://index.crates.io/";

/// The least time between two requests to the crates.io API (<https://crates.io/data-access>).
const API_INTERVAL: Duration = Duration::from_secs(1);

/// Runs the check for the workspace at `root`, comparing with `base` (or the default base branch), and reports progress through `report`.
pub fn check(root: &Path, base: Option<&str>, report: &mut dyn FnMut(&str)) -> Result<(), String> {
    let base = base.map_or_else(git::default_base, str::to_owned);
    let now = time::now()?;
    let repository = Repository::open(root)?;
    let added = added_versions(&repository, root, &base, report)?;
    if added.is_empty() {
        return Ok(());
    }
    let mut curl = Curl::new(user_agent());
    let mut crates_io = CratesIo::new(&mut curl, API_INTERVAL);
    evaluate(&added, now, &mut crates_io, report)
}

/// The User-Agent of the requests to crates.io, which asks every tool to name itself and a way to reach its maintainers.
fn user_agent() -> String {
    format!(
        "BayanDocs-xtask/{} (check-lockfile-age; +{})",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_REPOSITORY")
    )
}

/// A package version that the change adds to `Cargo.lock`, and the commit that added it (`None` if it is not committed yet).
#[derive(Debug, PartialEq, Eq)]
pub struct Added {
    /// The package version.
    pub package: Package,
    /// The first commit since the merge base whose `Cargo.lock` contains it.
    pub commit: Option<Commit>,
}

/// The package versions in the working tree's `Cargo.lock` that the one at the merge base with `base` does not contain, each with the commit that added it.
fn added_versions(
    repository: &Repository,
    root: &Path,
    base: &str,
    report: &mut dyn FnMut(&str),
) -> Result<Vec<Added>, String> {
    let merge_base = repository.merge_base(base)?;
    let before: BTreeSet<Package> = match repository.file_at(&merge_base, "Cargo.lock")? {
        Some(text) => cargo_lock::parse(&text)
            .map_err(|problem| format!("Cargo.lock at {}: {problem}", merge_base.short()))?
            .into_iter()
            .collect(),
        None => BTreeSet::new(),
    };
    let path = root.join("Cargo.lock");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let now: BTreeSet<Package> = cargo_lock::parse(&text)?.into_iter().collect();
    let new: Vec<&Package> = now.difference(&before).collect();
    if new.is_empty() {
        report(&format!(
            "Cargo.lock adds or changes no package version since the merge base with {base} ({}), so there is nothing to look up",
            merge_base.short()
        ));
        return Ok(Vec::new());
    }
    report(&format!(
        "Cargo.lock adds or changes {} package version(s) since the merge base with {base} ({})",
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
            .map_err(|problem| format!("Cargo.lock at {}: {problem}", commit.short()))?
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

/// Looks up when each added version was published and checks its age; `now` is the current time.
fn evaluate(
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
            // The workspace's own crates, whose code is in this repository.
            None => {}
            Some(CRATES_IO | CRATES_IO_SPARSE) => {
                let published = match crates_io.published(&package.name, &package.version) {
                    Ok(published) => published,
                    Err(problem) => {
                        problems.push(format!("{label}: cannot find its publish time: {problem}"));
                        continue;
                    }
                };
                match judge(published, item.commit.as_ref(), now) {
                    Ok(line) => report(&format!("ok: {label}: {line}")),
                    Err(line) => {
                        report(&format!("TOO NEW: {label}: {line}"));
                        problems.push(format!("{label}: {line}"));
                    }
                }
            }
            Some(source) if source.starts_with("git+") => report(&format!(
                "not checked: {label} comes from a Git repository ({source}), which has no crates.io publish time; the pull request must state the age of that commit (ADR-0017 rule 4)"
            )),
            Some(source) => problems.push(format!(
                "{label} comes from `{source}`; dependencies come only from crates.io (ADR-0017)"
            )),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} package version(s) in Cargo.lock break the 24-hour minimum age of ADR-0017 rule 4 or could not be checked:\n  - {}\nPin versions that were published at least 24 hours before your commit (and before now). If a security fix is younger than that, follow the security-alert procedure in the docs repository's developer/dependency-update-runbook.md instead of installing it early.",
            problems.len(),
            problems.join("\n  - ")
        ))
    }
}

/// Whether a version published at `published` was at least a day old when `commit` added it (or now, if it is not committed yet) and is at least a day old now, described for people either way.
fn judge(published: i64, commit: Option<&Commit>, now: i64) -> Result<String, String> {
    let (reference, when) = match commit {
        Some(commit) if commit.time <= now => (
            commit.time,
            format!(
                "it was added (commit {}, {})",
                commit.short(),
                time::display(commit.time)
            ),
        ),
        Some(commit) => (
            now,
            format!(
                "now (its commit {} is dated in the future, {})",
                commit.short(),
                time::display(commit.time)
            ),
        ),
        None => (now, "now (it is not committed yet)".to_owned()),
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

    fn lockfile(packages: &[(&str, &str, &str)]) -> String {
        let mut text = String::from(
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"bayan-server\"\nversion = \"0.1.0\"\ndependencies = [\n",
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
        let test = TestRepository::new();
        test.write("Cargo.lock", base);
        test.commit("base", time("2026-09-01T00:00:00Z"));
        test.git(&["checkout", "--quiet", "-b", "change"], None);
        for (text, at) in commits {
            test.write("Cargo.lock", text);
            test.commit("change", *at);
        }
        if let Some(text) = working {
            test.write("Cargo.lock", text);
        }
        let repository = Repository::open(test.path()).unwrap();
        let mut lines = Vec::new();
        let mut report = |line: &str| lines.push(line.to_owned());
        let added = added_versions(&repository, test.path(), "main", &mut report).unwrap();
        let mut recorded = Recorded::crates_io();
        let result = {
            let mut crates_io = CratesIo::new(&mut recorded, Duration::ZERO);
            evaluate(&added, now, &mut crates_io, &mut report)
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
            problem.starts_with("1 package version(s) in Cargo.lock break"),
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
    fn treats_a_changed_checksum_as_a_change() {
        let base = lockfile(&[HYPER_1_12_0]);
        let tampered = base.replace(HYPER_1_12_0.2, &"0".repeat(64));
        let (result, lines, _) = run(&base, &[(&tampered, RECORDED_AT)], None, RECORDED_AT);
        assert!(
            lines[0].contains("adds or changes 1 package version(s)"),
            "{lines:?}"
        );
        assert!(result.is_err());
    }

    #[test]
    fn reports_git_and_other_sources() {
        let base = lockfile(&[]);
        let change = format!(
            "{base}\n[[package]]\nname = \"gitdep\"\nversion = \"0.1.0\"\nsource = \"git+https://example.org/gitdep?rev=eb23095592359c454a586d16d08b2bd3af44b551#eb23095592359c454a586d16d08b2bd3af44b551\"\n\n[[package]]\nname = \"elsewhere\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.org/index\"\nchecksum = \"{}\"\n",
            "1".repeat(64)
        );
        let (result, lines, requests) = run(&base, &[(&change, RECORDED_AT)], None, RECORDED_AT);
        assert!(
            lines
                .iter()
                .any(|line| line
                    .starts_with("not checked: gitdep 0.1.0 comes from a Git repository")),
            "{lines:?}"
        );
        assert!(
            result
                .unwrap_err()
                .contains("elsewhere 1.0.0 comes from `registry+https://example.org/index`")
        );
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
                .contains("hyper 9.9.9: cannot find its publish time")
        );
    }

    #[test]
    fn judges_ages_at_the_boundary() {
        let commit = Commit {
            id: "1".repeat(40),
            time: 2 * time::DAY,
        };
        assert!(judge(time::DAY, Some(&commit), 3 * time::DAY).is_ok());
        assert!(judge(time::DAY + 1, Some(&commit), 3 * time::DAY).is_err());
        assert!(judge(time::DAY, None, 2 * time::DAY).is_ok());
        assert!(judge(time::DAY + 1, None, 2 * time::DAY).is_err());
        assert!(
            judge(3 * time::DAY, Some(&commit), 3 * time::DAY)
                .unwrap_err()
                .contains("after it was added")
        );
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
}
