//! Reads the list of packages from `Cargo.lock`.
//!
//! Cargo writes the file in one fixed layout but reads it as TOML, which can write the same data in many other ways (`[[ package ]]`, `[["package"]]`, `package = [{ … }]`, quoted keys, other spacing). A reader that understood only the usual layout could be handed a file that Cargo reads differently, so this one accepts exactly the layout Cargo writes and reports anything else as an error:
//!
//! - comment lines at the top, then `version = 3` or `version = 4`;
//! - `[[package]]` tables, each with `name`, `version` and, for packages that do not belong to the workspace, `source` and `checksum`, as `key = "text"` lines, and with `dependencies = [`, one ` "…",` line per dependency and `]`;
//! - blank lines between them.
//!
//! The age check also compares what this reader finds with the packages Cargo itself resolves (`lockfile_age.rs`), so that a difference this reader missed would still fail the check.

/// One `[[package]]` entry of `Cargo.lock`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Package {
    /// The crate name.
    pub name: String,
    /// The exact version.
    pub version: String,
    /// Where Cargo gets it, such as `registry+https://github.com/rust-lang/crates.io-index`; `None` for the workspace's own crates.
    pub source: Option<String>,
    /// The SHA-256 of the downloaded crate, for packages from a registry.
    pub checksum: Option<String>,
}

impl Package {
    /// The package for people: `hyper 1.12.0`.
    pub fn label(&self) -> String {
        format!("{} {}", self.name, self.version)
    }
}

/// The packages of a `Cargo.lock`, in the file's order.
pub fn parse(text: &str) -> Result<Vec<Package>, String> {
    let mut packages = Vec::new();
    // The `[[package]]` being read, or `None` before the first.
    let mut current: Option<Fields> = None;
    let mut in_list = false;
    let mut format_version = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let unexpected = || {
            format!(
                "Cargo.lock line {number}: `{line}` is not part of the layout Cargo writes, so this check cannot be sure how Cargo reads it; regenerate the file with Cargo"
            )
        };
        if !line.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
            return Err(format!(
                "Cargo.lock line {number}: a tab or a character outside printable ASCII"
            ));
        }
        if in_list {
            if line == "]" {
                in_list = false;
            } else if line
                .strip_prefix(' ')
                .and_then(|item| item.strip_suffix(','))
                .and_then(string)
                .is_none()
            {
                return Err(unexpected());
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            // Cargo's own comment comes first; a comment anywhere else is not Cargo's.
            if format_version || current.is_some() {
                return Err(unexpected());
            }
            continue;
        }
        if line == "[[package]]" {
            if !format_version {
                return Err(format!(
                    "Cargo.lock line {number}: a [[package]] before the `version = …` line"
                ));
            }
            if let Some(fields) = current.take() {
                packages.push(fields.finish()?);
            }
            current = Some(Fields::new(number));
            continue;
        }
        let (key, value) = line.split_once(" = ").ok_or_else(unexpected)?;
        let Some(fields) = current.as_mut() else {
            // Before the first [[package]], only the format version.
            if key == "version" && !format_version && matches!(value, "3" | "4") {
                format_version = true;
                continue;
            }
            return Err(unexpected());
        };
        match key {
            "dependencies" if value == "[" && !fields.dependencies => {
                fields.dependencies = true;
                in_list = true;
            }
            "name" | "version" | "source" | "checksum" => {
                let value = string(value).ok_or_else(unexpected)?;
                fields.set(key, value, number)?;
            }
            _ => return Err(unexpected()),
        }
    }
    if in_list {
        return Err("Cargo.lock ends inside a list".to_owned());
    }
    if !format_version {
        return Err("Cargo.lock has no `version = 3` or `version = 4` line".to_owned());
    }
    if let Some(fields) = current {
        packages.push(fields.finish()?);
    }
    Ok(packages)
}

/// The contents of a string as Cargo writes it (`"…"`, printable ASCII without quotes or backslashes), or `None` for anything else.
fn string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    if inner.contains(['"', '\\']) {
        return None;
    }
    Some(inner.to_owned())
}

/// The fields of one `[[package]]` while it is read.
struct Fields {
    line: usize,
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
    checksum: Option<String>,
    dependencies: bool,
}

impl Fields {
    fn new(line: usize) -> Self {
        Self {
            line,
            name: None,
            version: None,
            source: None,
            checksum: None,
            dependencies: false,
        }
    }

    fn set(&mut self, key: &str, value: String, number: usize) -> Result<(), String> {
        let slot = match key {
            "name" => &mut self.name,
            "version" => &mut self.version,
            "source" => &mut self.source,
            _ => &mut self.checksum,
        };
        if slot.replace(value).is_some() {
            return Err(format!(
                "Cargo.lock line {number}: `{key}` appears twice in one [[package]]"
            ));
        }
        Ok(())
    }

    fn finish(self) -> Result<Package, String> {
        match (self.name, self.version) {
            (Some(name), Some(version)) => Ok(Package {
                name,
                version,
                source: self.source,
                checksum: self.checksum,
            }),
            _ => Err(format!(
                "Cargo.lock line {}: a [[package]] without a name or a version",
                self.line
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCKFILE: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "bayan-server"
version = "0.1.0"
dependencies = [
 "hyper",
 "serde 1.0.229",
]

[[package]]
name = "hyper"
version = "1.12.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2c3e324da4c95177d6291d4c8730197c0d1822f8a9766814a4a44fa5ab797c9c"

[[package]]
name = "gitdep"
version = "0.1.0"
source = "git+https://example.org/gitdep?rev=eb23095592359c454a586d16d08b2bd3af44b551#eb23095592359c454a586d16d08b2bd3af44b551"
"#;

    #[test]
    fn reads_every_package() {
        let packages = parse(LOCKFILE).unwrap();
        let labels: Vec<String> = packages.iter().map(Package::label).collect();
        assert_eq!(
            labels,
            ["bayan-server 0.1.0", "hyper 1.12.0", "gitdep 0.1.0"]
        );
        assert_eq!(packages[0].source, None);
        assert_eq!(
            packages[1].source.as_deref(),
            Some("registry+https://github.com/rust-lang/crates.io-index")
        );
        assert_eq!(
            packages[1].checksum.as_deref(),
            Some("2c3e324da4c95177d6291d4c8730197c0d1822f8a9766814a4a44fa5ab797c9c")
        );
        assert!(packages[2].source.as_deref().unwrap().starts_with("git+"));
    }

    #[test]
    fn reads_windows_line_endings() {
        let packages = parse(&LOCKFILE.replace('\n', "\r\n")).unwrap();
        assert_eq!(packages.len(), 3);
    }

    #[test]
    fn rejects_what_cargo_does_not_write() {
        let package = "version = 4\n\n[[package]]\n";
        for text in [
            format!("{package}name = \"a\"\n"),
            format!("{package}version = \"1.0.0\"\n"),
            format!("{package}name = \"a\"\nname = \"b\"\nversion = \"1.0.0\"\n"),
            format!("{package}name = \"a\"\nversion = \"1.0.0\"\nfeatures = \"x\"\n"),
            format!("{package}name = 'a'\nversion = \"1.0.0\"\n"),
            format!("{package}name = \"a\\u0062\"\nversion = \"1.0.0\"\n"),
            format!("{package}name=\"a\"\nversion = \"1.0.0\"\n"),
            format!("{package} name = \"a\"\nversion = \"1.0.0\"\n"),
            format!("{package}\"name\" = \"a\"\nversion = \"1.0.0\"\n"),
            format!("{package}name = \"a\" # a comment\nversion = \"1.0.0\"\n"),
            format!("{package}# a comment\nname = \"a\"\nversion = \"1.0.0\"\n"),
            format!("{package}name = \"a\"\nversion = \"1.0.0\"\nreplace = \"b 1.0.0\"\n"),
            format!("{package}name = \"a\"\nversion = \"1.0.0\"\ndependencies = [\n \"b\",\n"),
            format!("{package}name = \"a\"\nversion = \"1.0.0\"\ndependencies = [\n b,\n]\n"),
            format!("{package}name = \"a\"\nversion = \"1.0.0\"\ndependencies = [\"b\"]\n"),
            format!("{package}name = \"a\"\nversion = \"1.0.0\"\tx\n"),
            "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n".to_owned(),
            "version = 5\n".to_owned(),
            String::new(),
        ] {
            assert!(parse(&text).is_err(), "{text:?}");
        }
    }

    /// TOML spellings that Cargo reads like `[[package]]` tables (it compares lockfiles by meaning, not text, so `--locked` accepts them), which an earlier version of this reader skipped as other tables, hiding the package from the age check.
    #[test]
    fn rejects_other_spellings_of_a_package() {
        let hidden = "name = \"hyper\"\nversion = \"1.12.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"2c3e324da4c95177d6291d4c8730197c0d1822f8a9766814a4a44fa5ab797c9c\"\n";
        for text in [
            format!("version = 4\n\n[[ package ]]\n{hidden}"),
            format!("version = 4\n\n[[\"package\"]]\n{hidden}"),
            "version = 4\npackage = [{ name = \"hyper\", version = \"1.12.0\" }]\n".to_owned(),
            format!(
                "version = 4\n\n[metadata]\n\"checksum a 1.0.0\" = \"x\"\n\n[[package]]\n{hidden}"
            ),
            format!("version = 4\n\n[[patch.unused]]\n{hidden}"),
        ] {
            assert!(parse(&text).is_err(), "{text:?}");
        }
    }
}
