//! What the checks ask Git: the commit where the current branch left the branch it will be merged into (the merge base), the commits since then with their times, and files as they were at a commit.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A commit and its two times. Git keeps the author time when a commit is amended or rebased, and gives it a new committer time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The full commit hash.
    pub id: String,
    /// When its change was first written, in seconds since 1970.
    pub author: i64,
    /// When the commit itself was last made (amended, rebased or created), in seconds since 1970.
    pub committer: i64,
}

impl Commit {
    /// The first twelve characters of the hash, for people.
    pub fn short(&self) -> &str {
        self.id.get(..12).unwrap_or(&self.id)
    }

    /// The earlier of the author and committer times, so that rebasing or amending a change cannot make it look younger than it is.
    pub fn time(&self) -> i64 {
        self.author.min(self.committer)
    }
}

/// The branch a change is compared with: `origin/<branch>` for the base branch of the pull request that GitHub Actions is checking (`GITHUB_BASE_REF`), and otherwise `origin/main`.
pub fn default_base() -> String {
    match std::env::var("GITHUB_BASE_REF") {
        Ok(branch) if !branch.trim().is_empty() => format!("origin/{}", branch.trim()),
        _ => "origin/main".to_owned(),
    }
}

/// A Git working tree with its full history.
pub struct Repository {
    root: PathBuf,
    /// The path of the workspace root inside the repository, such as `""` at the top or `"rust/"` below it.
    prefix: String,
}

impl Repository {
    /// The repository that holds the workspace at `root`. Fails if Git is missing, if `root` is not in a Git working tree, or if the clone is shallow, because a shallow clone lacks the commits that the checks compare.
    pub fn open(root: &Path) -> Result<Self, String> {
        let repository = Self {
            root: root.to_path_buf(),
            prefix: String::new(),
        };
        let inside = repository
            .run(&["rev-parse", "--is-inside-work-tree"])
            .map_err(|problem| {
                format!("{problem}\nThis check needs Git and a Git checkout of the repository.")
            })?;
        if inside.trim() != "true" {
            return Err(format!(
                "{} is not inside a Git working tree",
                root.display()
            ));
        }
        if repository
            .run(&["rev-parse", "--is-shallow-repository"])?
            .trim()
            == "true"
        {
            return Err("this clone is shallow, so it lacks the commits the check compares. Fetch the full history (`git fetch --unshallow`); in GitHub Actions, check out with `fetch-depth: 0`.".to_owned());
        }
        let prefix = repository.run(&["rev-parse", "--show-prefix"])?;
        Ok(Self {
            prefix: prefix.trim_end_matches(['\r', '\n']).to_owned(),
            ..repository
        })
    }

    /// The merge base of `HEAD` and `base`.
    pub fn merge_base(&self, base: &str) -> Result<Commit, String> {
        let resolved = self
            .run(&[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{base}^{{commit}}"),
            ])
            .map_err(|_| {
                format!("cannot find the base branch `{base}` in this clone. Fetch it (for example `git fetch origin main`) or name another one with `--base`.")
            })?;
        let merge_base = self.run(&["merge-base", "HEAD", resolved.trim()])?;
        let mut commits = self.log(&["-1", merge_base.trim()])?;
        commits
            .pop()
            .ok_or_else(|| format!("cannot read the merge base of HEAD and {base}"))
    }

    /// The commits after `start` up to and including `HEAD`, oldest first: those that `git log start..HEAD` lists.
    pub fn commits_since(&self, start: &Commit) -> Result<Vec<Commit>, String> {
        self.log(&["--reverse", "--topo-order", &format!("{}..HEAD", start.id)])
    }

    /// The text of `path` (relative to the workspace root) at `commit`, or `None` if the file did not exist there.
    pub fn file_at(&self, commit: &Commit, path: &str) -> Result<Option<String>, String> {
        let object = format!("{}:{}{path}", commit.id, self.prefix);
        let exists = self
            .command(&["cat-file", "-e", &object])
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("could not run git: {error}"))?;
        if !exists.success() {
            return Ok(None);
        }
        self.run(&["cat-file", "blob", &object]).map(Some)
    }

    /// The commits that `git log <args>` lists, with their times. `--no-show-signature` keeps a `log.showSignature` setting from adding lines about signatures, and `--` ends the revisions.
    fn log(&self, args: &[&str]) -> Result<Vec<Commit>, String> {
        let mut all = vec!["log", "--no-show-signature", "--format=%H %at %ct"];
        all.extend_from_slice(args);
        all.push("--");
        let output = self.run(&all)?;
        output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let mut words = line.split_whitespace();
                let id = words.next().unwrap_or_default().to_owned();
                let times: Option<Vec<i64>> = words.map(|word| word.parse().ok()).collect();
                match times.as_deref() {
                    Some([author, committer]) if id.len() >= 40 => Ok(Commit {
                        id,
                        author: *author,
                        committer: *committer,
                    }),
                    _ => Err(format!("cannot read the line `{line}` of git log")),
                }
            })
            .collect()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .stdin(Stdio::null());
        command
    }

    /// Runs git with `args` in the workspace root and returns what it printed, or an error with its message.
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let output = self
            .command(args)
            .output()
            .map_err(|error| format!("could not run `git {}`: {error}", args.join(" ")))?;
        if output.status.success() {
            String::from_utf8(output.stdout)
                .map_err(|_| format!("`git {}` printed text that is not UTF-8", args.join(" ")))
        } else {
            Err(format!(
                "`git {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

#[cfg(test)]
pub mod testing {
    //! Small Git repositories with commits at chosen times, for the tests of the checks that read history.

    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A temporary Git repository, deleted when dropped.
    pub struct TestRepository {
        /// Its working tree.
        pub root: PathBuf,
    }

    impl TestRepository {
        /// A new, empty repository whose branch is `main`.
        pub fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "bayandocs-xtask-git-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            if root.exists() {
                std::fs::remove_dir_all(&root).unwrap();
            }
            std::fs::create_dir_all(&root).unwrap();
            let repository = Self { root };
            repository.git(&["init", "--quiet", "--initial-branch=main"], None);
            repository
        }

        /// Writes `text` to `path` in the working tree.
        pub fn write(&self, path: &str, text: &str) {
            let file = self.root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }

        /// Commits everything in the working tree, authored and committed at `time` (seconds since 1970), and returns the commit hash.
        pub fn commit(&self, message: &str, time: i64) -> String {
            self.git(&["add", "--all"], None);
            self.git(
                &["commit", "--quiet", "--allow-empty", "--message", message],
                Some(time),
            );
            self.git(&["rev-parse", "HEAD"], None).trim().to_owned()
        }

        /// Runs git in the repository, isolated from the configuration of the machine running the tests, and returns what it printed.
        pub fn git(&self, args: &[&str], time: Option<i64>) -> String {
            let config = self.root.with_extension("gitconfig");
            if !config.exists() {
                std::fs::write(&config, "").unwrap();
            }
            let mut command = Command::new("git");
            command
                .arg("-C")
                .arg(&self.root)
                .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", &config)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@bayandocs.invalid")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@bayandocs.invalid");
            if let Some(time) = time {
                let date = format!("@{time} +0000");
                command
                    .env("GIT_AUTHOR_DATE", &date)
                    .env("GIT_COMMITTER_DATE", &date);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        }

        /// The working tree.
        pub fn path(&self) -> &Path {
            &self.root
        }
    }

    impl Drop for TestRepository {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
            let _ = std::fs::remove_file(self.root.with_extension("gitconfig"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TestRepository;
    use super::*;

    #[test]
    fn finds_the_merge_base_the_commits_since_and_old_files() {
        let test = TestRepository::new();
        test.write("Cargo.lock", "one\n");
        let first = test.commit("first", 1_000_000_000);
        test.git(&["branch", "base"], None);
        test.git(&["checkout", "--quiet", "-b", "feature"], None);
        test.write("Cargo.lock", "two\n");
        let second = test.commit("second", 1_000_000_100);
        std::fs::remove_file(test.path().join("Cargo.lock")).unwrap();
        let third = test.commit("third", 1_000_000_200);

        let repository = Repository::open(test.path()).unwrap();
        let merge_base = repository.merge_base("base").unwrap();
        assert_eq!(merge_base.id, first);
        assert_eq!(merge_base.time(), 1_000_000_000);
        let commits = repository.commits_since(&merge_base).unwrap();
        let ids: Vec<&str> = commits.iter().map(|commit| commit.id.as_str()).collect();
        assert_eq!(ids, [second.as_str(), third.as_str()]);
        assert_eq!(commits[0].time(), 1_000_000_100);
        assert_eq!(commits[0].short(), &second[..12]);
        assert_eq!(
            repository
                .file_at(&merge_base, "Cargo.lock")
                .unwrap()
                .as_deref(),
            Some("one\n")
        );
        assert_eq!(repository.file_at(&commits[1], "Cargo.lock").unwrap(), None);
        assert!(
            repository
                .merge_base("no-such-branch")
                .unwrap_err()
                .contains("cannot find the base branch")
        );
    }

    #[test]
    fn takes_the_earlier_of_author_and_committer_time() {
        let test = TestRepository::new();
        test.commit("first", 1_000_000_000);
        // Amending a commit gives it a new committer time; here its author time is set earlier still, as `git commit --date` can.
        test.git(
            &[
                "commit",
                "--quiet",
                "--amend",
                "--allow-empty",
                "--no-edit",
                "--date",
                "@999999000 +0000",
            ],
            Some(1_000_000_900),
        );
        let repository = Repository::open(test.path()).unwrap();
        let commits = repository.log(&["-1", "HEAD"]).unwrap();
        assert_eq!(commits[0].author, 999_999_000);
        assert_eq!(commits[0].committer, 1_000_000_900);
        assert_eq!(commits[0].time(), 999_999_000);
    }

    #[test]
    fn reads_the_log_when_git_is_set_to_show_signatures() {
        let test = TestRepository::new();
        test.commit("first", 1_000_000_000);
        // A commit with an SSH signature that Git cannot verify here: with `log.showSignature`, `git log` prints lines such as "No signature" for it unless told not to.
        let tree = test.git(&["rev-parse", "HEAD^{tree}"], None);
        let object = test.root.with_extension("commit");
        std::fs::write(
            &object,
            format!(
                "tree {}\nauthor Test <test@bayandocs.invalid> 1000000100 +0000\ncommitter Test <test@bayandocs.invalid> 1000000100 +0000\ngpgsig -----BEGIN SSH SIGNATURE-----\n U1NIU0lHAAAAAQ==\n -----END SSH SIGNATURE-----\n\nsigned\n",
                tree.trim()
            ),
        )
        .unwrap();
        let signed = test.git(
            &[
                "hash-object",
                "-t",
                "commit",
                "-w",
                &object.to_string_lossy(),
            ],
            None,
        );
        std::fs::remove_file(&object).unwrap();
        test.git(&["update-ref", "HEAD", signed.trim()], None);
        test.git(&["config", "log.showSignature", "true"], None);
        let repository = Repository::open(test.path()).unwrap();
        let commits = repository.log(&["-1", "HEAD"]).unwrap();
        assert_eq!(commits[0].time(), 1_000_000_100);
    }

    #[test]
    fn refuses_a_folder_without_git() {
        let folder =
            std::env::temp_dir().join(format!("bayandocs-xtask-no-git-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        // A folder inside the system's temporary folder is normally not in a Git working tree; if it is, there is nothing to test.
        if Repository::open(&folder).is_ok() {
            std::fs::remove_dir_all(&folder).unwrap();
            return;
        }
        assert!(Repository::open(&folder).is_err());
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
