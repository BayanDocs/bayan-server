//! Repository automation for bayan-server, run as `cargo xtask <command>`.
//!
//! - `verify`: the verification gate that CI runs and that must pass before every push.
//! - `sqlx-prepare [--check]`: regenerates (or, with `--check`, verifies) the committed sqlx query metadata against scratch SQLite and PostgreSQL databases.
//! - `test-postgres`: runs the PostgreSQL integration test against the server named by `BAYAN_TEST_POSTGRES_URL`.
//!
//! It uses only the standard library and runs the real tools with [`std::process::Command`].

#![expect(
    clippy::print_stderr,
    reason = "a command-line tool reports progress on standard error"
)]

use std::borrow::BorrowMut;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "usage: cargo xtask <verify | sqlx-prepare [--check] | test-postgres>";

/// The backend crates whose queries sqlx checks at compile time. `sqlx-prepare` compiles each one on its own against its own database, so a single `DATABASE_URL` at a time is enough.
const SQLITE: &str = "bayan-db-sqlite";
const POSTGRES: &str = "bayan-db-postgres";
/// The PostgreSQL server `sqlx-prepare` uses; its user must be allowed to create databases.
const POSTGRES_URL_VAR: &str = "BAYAN_SQLX_POSTGRES_URL";
/// Name of the scratch database `sqlx-prepare` creates and drops on the PostgreSQL server.
const SCRATCH_DATABASE: &str = "bayan_sqlx_prepare";

type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["verify"] => verify(),
        ["sqlx-prepare"] => sqlx_prepare(false),
        ["sqlx-prepare", "--check"] => sqlx_prepare(true),
        ["test-postgres"] => test_postgres(),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The verification gate, in order. Every step must pass.
fn verify() -> Result {
    step("format", cargo(&["fmt", "--all", "--check"]))?;
    step(
        "lint",
        cargo(&[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ]),
    )?;
    step(
        "test",
        cargo(&["test", "--workspace", "--all-features", "--locked"]),
    )?;
    step(
        "documentation",
        cargo(&[
            "doc",
            "--workspace",
            "--all-features",
            "--no-deps",
            "--locked",
        ])
        .env("RUSTDOCFLAGS", "-D warnings"),
    )?;
    step(
        "dependency policy (cargo deny)",
        cargo(&["deny", "--locked", "--all-features", "check"]),
    )?;
    supply_chain_checks()?;
    eprintln!("xtask: verify passed");
    Ok(())
}

/// Hook for the supply-chain checks of work package X-003 (Cargo.lock publish-age check, update-bot ban). X-003 implements them here so that `verify` runs them everywhere.
fn supply_chain_checks() -> Result {
    eprintln!(
        "xtask: supply-chain checks: not implemented yet (work package X-003 adds them here)"
    );
    Ok(())
}

/// Regenerates the sqlx query metadata of every backend crate, or with `check` verifies the committed metadata is current.
///
/// Needs `python3` (its built-in sqlite3 module applies the SQLite migrations), `psql`, and `BAYAN_SQLX_POSTGRES_URL` pointing to a PostgreSQL server where the user may create databases.
fn sqlx_prepare(check: bool) -> Result {
    let root = workspace_root()?;
    let admin_url = std::env::var(POSTGRES_URL_VAR).map_err(|_| {
        format!("set {POSTGRES_URL_VAR} to a PostgreSQL URL whose user may create databases")
    })?;
    // The build directory inside is kept between runs to save time; the databases and outputs are recreated.
    let work = root.join("target").join("sqlx-prepare");
    std::fs::create_dir_all(&work)
        .map_err(|error| format!("cannot create {}: {error}", work.display()))?;

    // A scratch SQLite database with the current migrations applied.
    let sqlite_file = work.join("prepare.sqlite3");
    for stale in [
        "prepare.sqlite3",
        "prepare.sqlite3-wal",
        "prepare.sqlite3-shm",
    ] {
        remove_file_if_exists(&work.join(stale))?;
    }
    let mut apply = Command::new("python3");
    apply.args([
        "-c",
        "import sqlite3, sys\ncon = sqlite3.connect(sys.argv[1])\nfor path in sys.argv[2:]:\n    con.executescript(open(path, encoding='utf-8').read())\ncon.commit()\ncon.close()",
    ]);
    apply.arg(&sqlite_file).args(migrations(&root, SQLITE)?);
    step("apply SQLite migrations", &mut apply)?;
    let sqlite_url = format!("sqlite://{}", sqlite_file.display());

    // A scratch PostgreSQL database with the current migrations applied.
    let scratch_url = with_database(&admin_url, SCRATCH_DATABASE)?;
    let drop_scratch = format!("DROP DATABASE IF EXISTS {SCRATCH_DATABASE} WITH (FORCE)");
    step(
        "drop old scratch database",
        psql(&admin_url).args(["-c", &drop_scratch]),
    )?;
    step(
        "create scratch database",
        psql(&admin_url).args(["-c", &format!("CREATE DATABASE {SCRATCH_DATABASE}")]),
    )?;
    let result = (|| {
        for migration in migrations(&root, POSTGRES)? {
            step(
                "apply PostgreSQL migration",
                psql(&scratch_url)
                    .arg("--single-transaction")
                    .arg("-f")
                    .arg(migration),
            )?;
        }
        prepare_backend(&root, &work, SQLITE, &sqlite_url, check)?;
        prepare_backend(&root, &work, POSTGRES, &scratch_url, check)
    })();
    let dropped = step(
        "drop scratch database",
        psql(&admin_url).args(["-c", &drop_scratch]),
    );
    result.and(dropped)?;
    eprintln!(
        "xtask: sqlx query metadata {}",
        if check {
            "is up to date"
        } else {
            "regenerated"
        }
    );
    Ok(())
}

/// Recompiles one backend crate against a live database, collecting its query metadata, and compares or installs it.
fn prepare_backend(root: &Path, work: &Path, krate: &str, url: &str, check: bool) -> Result {
    let target_dir = work.join("target");
    let out_dir = work.join(krate);
    remove_if_exists(&out_dir)?;
    std::fs::create_dir_all(&out_dir)
        .map_err(|error| format!("cannot create {}: {error}", out_dir.display()))?;
    // Cleaning the crate forces its query macros to run again.
    step(
        "clean",
        cargo(&["clean", "--package", krate])
            .arg("--target-dir")
            .arg(&target_dir),
    )?;
    step(
        &format!("check {krate} against a live database"),
        cargo(&["check", "--locked", "--package", krate])
            .arg("--target-dir")
            .arg(&target_dir)
            .env("SQLX_OFFLINE", "false")
            .env("SQLX_OFFLINE_DIR", &out_dir)
            .env("DATABASE_URL", url),
    )?;
    let committed_dir = root.join("crates").join(krate).join(".sqlx");
    let generated = read_dir_files(&out_dir)?;
    if generated.is_empty() {
        return Err(format!("{krate}: sqlx wrote no query metadata"));
    }
    if check {
        let committed = read_dir_files(&committed_dir)?;
        if committed != generated {
            return Err(format!(
                "{} is out of date with the migrations and queries; run `cargo xtask sqlx-prepare` and commit the result",
                committed_dir
                    .strip_prefix(root)
                    .unwrap_or(&committed_dir)
                    .display()
            ));
        }
    } else {
        remove_if_exists(&committed_dir)?;
        std::fs::create_dir_all(&committed_dir)
            .map_err(|error| format!("cannot create {}: {error}", committed_dir.display()))?;
        for (name, contents) in &generated {
            let path = committed_dir.join(name);
            std::fs::write(&path, contents)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

/// Runs the PostgreSQL integration test (it is `#[ignore]`d in normal test runs because it needs a server).
fn test_postgres() -> Result {
    if std::env::var_os("BAYAN_TEST_POSTGRES_URL").is_none() {
        return Err(
            "set BAYAN_TEST_POSTGRES_URL to the URL of an empty PostgreSQL database".to_owned(),
        );
    }
    step(
        "PostgreSQL integration test",
        cargo(&[
            "test",
            "--locked",
            "--package",
            "bayan-server",
            "--test",
            "postgres",
            "--",
            "--ignored",
            "--nocapture",
        ]),
    )
}

fn cargo(args: &[&str]) -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.args(args);
    command
}

fn psql(url: &str) -> Command {
    let mut command = Command::new("psql");
    command.args([
        "--no-psqlrc",
        "--quiet",
        "-v",
        "ON_ERROR_STOP=1",
        "--dbname",
        url,
    ]);
    command
}

/// Runs `command`, failing with `name` if it does not succeed.
fn step(name: &str, mut command: impl BorrowMut<Command>) -> Result {
    let command = command.borrow_mut();
    eprintln!("xtask: {name}");
    let status = command
        .status()
        .map_err(|error| format!("{name}: cannot run {:?}: {error}", command.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} failed ({status})"))
    }
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir =
        std::env::var_os("CARGO_MANIFEST_DIR").ok_or("run this through `cargo xtask`")?;
    Path::new(&manifest_dir)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "cannot find the workspace root".to_owned())
}

/// The migration files of a backend crate, in the order sqlx applies them.
fn migrations(root: &Path, krate: &str) -> Result<Vec<PathBuf>> {
    let dir = root.join("crates").join(krate).join("migrations");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|error| format!("cannot read {}: {error}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension() == Some(OsStr::new("sql")))
        .collect();
    files.sort();
    Ok(files)
}

/// Replaces the database name in a PostgreSQL URL, keeping user, password, host, port and parameters.
fn with_database(url: &str, database: &str) -> Result<String> {
    let scheme_end = url.find("://").ok_or("the PostgreSQL URL has no scheme")? + 3;
    let rest = &url[scheme_end..];
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let query = tail.find('?').map_or("", |index| &tail[index..]);
    Ok(format!(
        "{}{authority}/{database}{query}",
        &url[..scheme_end]
    ))
}

fn read_dir_files(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(files);
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let contents = std::fs::read(entry.path())
            .map_err(|error| format!("cannot read {}: {error}", entry.path().display()))?;
        files.insert(name, contents);
    }
    Ok(files)
}

fn remove_file_if_exists(file: &Path) -> Result {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", file.display())),
    }
}

fn remove_if_exists(dir: &Path) -> Result {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::with_database;

    #[test]
    fn replaces_the_database_name_in_postgres_urls() {
        assert_eq!(
            with_database("postgres://u:p@h:5432/postgres", "x").as_deref(),
            Ok("postgres://u:p@h:5432/x")
        );
        assert_eq!(
            with_database("postgres://u@h", "x").as_deref(),
            Ok("postgres://u@h/x")
        );
        assert_eq!(
            with_database("postgresql://u@h/db?sslmode=disable", "x").as_deref(),
            Ok("postgresql://u@h/x?sslmode=disable")
        );
        assert_eq!(
            with_database("postgres://h?sslmode=disable", "x").as_deref(),
            Ok("postgres://h/x?sslmode=disable")
        );
    }
}
