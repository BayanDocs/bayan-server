//! Reads the list of packages from `Cargo.lock`.
//!
//! Cargo writes the file itself, in a fixed layout: `[[package]]` tables of `key = "string"` lines, with `dependencies` as a list over several lines. This reader understands exactly that and reports anything else as an error, so that a hand-edited file cannot hide a package from the checks.

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
    // The `[[package]]` being read, or `None` while reading another table (or the lines before the first).
    let mut current: Option<Fields> = None;
    let mut in_list = false;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if in_list {
            if line == "]" {
                in_list = false;
            } else if !(line.starts_with('"') && line.ends_with("\",")) {
                return Err(format!(
                    "Cargo.lock line {number}: unexpected `{line}` in a list"
                ));
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if let Some(fields) = current.take() {
                packages.push(fields.finish()?);
            }
            if line == "[[package]]" {
                current = Some(Fields::new(number));
            } else if !(line.ends_with(']') && line.len() > 2) {
                return Err(format!("Cargo.lock line {number}: unexpected `{line}`"));
            }
            continue;
        }
        let (key, value) = line.split_once(" = ").ok_or_else(|| {
            format!("Cargo.lock line {number}: expected `key = value`, found `{line}`")
        })?;
        if value == "[" {
            in_list = true;
            continue;
        }
        let Some(fields) = current.as_mut() else {
            // The lines before the first table (such as `version = 4`) and other tables do not describe packages.
            continue;
        };
        match key {
            "dependencies" if value == "[]" => {}
            "name" | "version" | "source" | "checksum" | "replace" => {
                let value = string(value).ok_or_else(|| {
                    format!("Cargo.lock line {number}: `{key}` is not a plain string")
                })?;
                fields.set(key, value, number)?;
            }
            _ => {
                return Err(format!(
                    "Cargo.lock line {number}: unexpected key `{key}` in a [[package]]"
                ));
            }
        }
    }
    if in_list {
        return Err("Cargo.lock ends inside a list".to_owned());
    }
    if let Some(fields) = current {
        packages.push(fields.finish()?);
    }
    Ok(packages)
}

/// The contents of a basic string as Cargo writes it (`"…"`, with `\\` and `\"` as the only escapes), or `None` for anything else.
fn string(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut text = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next()? {
                escaped @ ('\\' | '"') => text.push(escaped),
                _ => return None,
            },
            '"' => return None,
            _ => text.push(character),
        }
    }
    Some(text)
}

/// The fields of one `[[package]]` while it is read.
struct Fields {
    line: usize,
    name: Option<String>,
    version: Option<String>,
    source: Option<String>,
    checksum: Option<String>,
}

impl Fields {
    fn new(line: usize) -> Self {
        Self {
            line,
            name: None,
            version: None,
            source: None,
            checksum: None,
        }
    }

    fn set(&mut self, key: &str, value: String, number: usize) -> Result<(), String> {
        let slot = match key {
            "name" => &mut self.name,
            "version" => &mut self.version,
            "source" => &mut self.source,
            "checksum" => &mut self.checksum,
            // `replace` (written by old versions of Cargo for `[replace]`) points to another entry, which is checked itself.
            _ => return Ok(()),
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
dependencies = []

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
        for text in [
            "[[package]]\nname = \"a\"\n",
            "[[package]]\nversion = \"1.0.0\"\n",
            "[[package]]\nname = \"a\"\nname = \"b\"\nversion = \"1.0.0\"\n",
            "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nfeatures = \"x\"\n",
            "[[package]]\nname = 'a'\nversion = \"1.0.0\"\n",
            "[[package]]\nname = \"a\\u0062\"\nversion = \"1.0.0\"\n",
            "[[package]]\nname=\"a\"\nversion = \"1.0.0\"\n",
            "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\ndependencies = [\n \"b\",\n",
            "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\ndependencies = [\n b,\n]\n",
            "[package\nname = \"a\"\n",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn skips_other_tables() {
        let text = "version = 4\n\n[metadata]\n\"checksum a 1.0.0\" = \"x\"\n\n[[package]]\nname = \"a\"\nversion = \"1.0.0\"\n";
        let packages = parse(text).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].label(), "a 1.0.0");
    }
}
