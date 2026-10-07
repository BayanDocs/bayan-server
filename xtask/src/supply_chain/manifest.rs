//! Reads the `[workspace.dependencies]` table of the root `Cargo.toml`.
//!
//! It understands the forms a dependency is written in: `name = "=1.2.3"`, an inline table `name = { version = "=1.2.3", features = […] }` (whose arrays may span several lines), dotted keys such as `name.version = "=1.2.3"`, and a table of its own, `[workspace.dependencies.name]`. Anything it cannot read with certainty, such as multi-line strings, is an error rather than a guess, so that no entry can slip past the check unread.

/// One dependency declared in `[workspace.dependencies]`.
#[derive(Debug, PartialEq, Eq)]
pub struct Dependency {
    /// The name it is declared under.
    pub name: String,
    /// The line where it is declared first, counting from 1.
    pub line: usize,
    /// Its fields, in order: the key, and the value if it is a string (`None` for arrays, booleans and the like). A dependency written as a plain string is the field `version`.
    pub fields: Vec<(String, Option<String>)>,
}

impl Dependency {
    /// The value of the string field `key`, if there is one.
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, value)| value.as_deref())
    }

    /// Whether the field `key` is present, whatever its value.
    pub fn has(&self, key: &str) -> bool {
        self.fields.iter().any(|(name, _)| name == key)
    }
}

/// The table that declares the workspace's dependencies.
const TABLE: [&str; 2] = ["workspace", "dependencies"];

/// The dependencies declared in `[workspace.dependencies]` of `text`, in the order they are first declared.
pub fn workspace_dependencies(text: &str) -> Result<Vec<Dependency>, String> {
    let mut dependencies: Vec<Dependency> = Vec::new();
    // The current table's name, split into its parts.
    let mut table: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut index = 0;
    while index < lines.len() {
        let number = index + 1;
        let mut statement = strip_comment(lines[index])
            .map_err(|problem| format!("Cargo.toml line {number}: {problem}"))?;
        index += 1;
        // A value continues on the next lines while one of its arrays or inline tables is open.
        while open_brackets(&statement)
            .map_err(|problem| format!("Cargo.toml line {number}: {problem}"))?
            > 0
        {
            let next = lines
                .get(index)
                .ok_or_else(|| format!("Cargo.toml line {number}: a value is never closed"))?;
            statement.push(' ');
            statement.push_str(
                &strip_comment(next)
                    .map_err(|problem| format!("Cargo.toml line {}: {problem}", index + 1))?,
            );
            index += 1;
        }
        let statement = statement.trim();
        if statement.is_empty() {
            continue;
        }
        if let Some(header) = statement.strip_prefix('[') {
            let inner = header
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix("]]"))
                .or_else(|| header.strip_suffix(']'))
                .ok_or_else(|| {
                    format!("Cargo.toml line {number}: cannot read the table header `{statement}`")
                })?;
            table = key_parts(inner)
                .map_err(|problem| format!("Cargo.toml line {number}: {problem}"))?;
            continue;
        }
        let in_table = table.len() >= TABLE.len() && table[..TABLE.len()] == TABLE;
        if !in_table {
            continue;
        }
        let (key, value) = split_key_value(statement).ok_or_else(|| {
            format!("Cargo.toml line {number}: expected `key = value`, found `{statement}`")
        })?;
        let mut path: Vec<String> = table[TABLE.len()..].to_vec();
        path.extend(
            key_parts(key).map_err(|problem| format!("Cargo.toml line {number}: {problem}"))?,
        );
        let value = value.trim();
        let (name, fields) = match path.as_slice() {
            [name] => {
                let fields = if value.starts_with('{') {
                    inline_table(value)
                        .map_err(|problem| format!("Cargo.toml line {number}: {problem}"))?
                } else {
                    let version = string(value).ok_or_else(|| {
                        format!("Cargo.toml line {number}: the dependency `{name}` is neither a version string nor an inline table")
                    })?;
                    vec![("version".to_owned(), Some(version))]
                };
                (name.clone(), fields)
            }
            [name, field] => (name.clone(), vec![(field.clone(), string(value))]),
            _ => {
                return Err(format!(
                    "Cargo.toml line {number}: cannot read `{}` in [workspace.dependencies]",
                    path.join(".")
                ));
            }
        };
        match dependencies
            .iter_mut()
            .find(|dependency| dependency.name == name)
        {
            Some(dependency) => dependency.fields.extend(fields),
            None => dependencies.push(Dependency {
                name,
                line: number,
                fields,
            }),
        }
    }
    Ok(dependencies)
}

/// Removes a comment from one line, leaving `#` characters inside strings alone.
fn strip_comment(line: &str) -> Result<String, String> {
    let mut quote = None;
    let mut escaped = false;
    for (position, character) in line.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None if character == '#' => return Ok(line[..position].to_owned()),
            None if character == '"' || character == '\'' => {
                if line[position..].starts_with("\"\"\"") || line[position..].starts_with("'''") {
                    return Err("multi-line strings are not supported here".to_owned());
                }
                quote = Some(character);
            }
            None => {}
        }
    }
    if quote.is_some() {
        return Err("a string is not closed on this line".to_owned());
    }
    Ok(line.to_owned())
}

/// How many arrays and inline tables are still open at the end of `text`, outside strings.
fn open_brackets(text: &str) -> Result<usize, String> {
    let mut open: usize = 0;
    for (character, quoted) in characters(text) {
        if quoted {
            continue;
        }
        match character {
            '[' | '{' => open += 1,
            ']' | '}' => {
                open = open
                    .checked_sub(1)
                    .ok_or_else(|| format!("unbalanced brackets in `{}`", text.trim()))?;
            }
            _ => {}
        }
    }
    Ok(open)
}

/// The characters of `text`, each with whether it is inside (or delimits) a string.
fn characters(text: &str) -> Vec<(char, bool)> {
    let mut result = Vec::with_capacity(text.len());
    let mut quote = None;
    let mut escaped = false;
    for character in text.chars() {
        let inside = quote.is_some();
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(close) if character == close => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None => {}
        }
        result.push((character, inside || quote.is_some()));
    }
    result
}

/// Splits `key = value` at the first `=` outside quotes.
fn split_key_value(text: &str) -> Option<(&str, &str)> {
    let mut offset = 0;
    for (character, quoted) in characters(text) {
        if character == '=' && !quoted {
            return Some((&text[..offset], &text[offset + 1..]));
        }
        offset += character.len_utf8();
    }
    None
}

/// Splits a dotted key or table name into its parts, removing quotes: `"serde".version` gives `serde` and `version`.
fn key_parts(key: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut rest = key.trim();
    loop {
        let (part, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted
                .find('"')
                .ok_or_else(|| format!("cannot read the key `{key}`"))?;
            if quoted[..end].contains('\\') {
                return Err(format!("escapes in the key `{key}` are not supported"));
            }
            (quoted[..end].to_owned(), &quoted[end + 1..])
        } else if let Some(quoted) = rest.strip_prefix('\'') {
            let end = quoted
                .find('\'')
                .ok_or_else(|| format!("cannot read the key `{key}`"))?;
            (quoted[..end].to_owned(), &quoted[end + 1..])
        } else {
            let end = rest.find(['.', ' ', '\t']).unwrap_or(rest.len());
            let bare = &rest[..end];
            if bare.is_empty()
                || !bare
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(format!("cannot read the key `{key}`"));
            }
            (bare.to_owned(), &rest[end..])
        };
        parts.push(part);
        let after = after.trim_start();
        if after.is_empty() {
            return Ok(parts);
        }
        rest = after
            .strip_prefix('.')
            .ok_or_else(|| format!("cannot read the key `{key}`"))?
            .trim_start();
    }
}

/// The fields of an inline table, `{ key = value, … }`, with the values that are strings.
fn inline_table(text: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let inner = text
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .ok_or_else(|| format!("cannot read the inline table `{text}`"))?;
    let mut fields = Vec::new();
    for part in split_top_level(inner) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) =
            split_key_value(part).ok_or_else(|| format!("expected `key = value` in `{text}`"))?;
        let parts = key_parts(key)?;
        let [key] = parts.as_slice() else {
            return Err(format!(
                "dotted keys inside the inline table `{text}` are not supported"
            ));
        };
        fields.push((key.clone(), string(value.trim())));
    }
    Ok(fields)
}

/// Splits the inside of an inline table at the commas that are not inside a string, an array or a nested table.
fn split_top_level(inner: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut offset = 0;
    let mut depth: usize = 0;
    for (character, quoted) in characters(inner) {
        if !quoted {
            match character {
                '[' | '{' => depth += 1,
                ']' | '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    parts.push(&inner[start..offset]);
                    start = offset + 1;
                }
                _ => {}
            }
        }
        offset += character.len_utf8();
    }
    parts.push(&inner[start..]);
    parts
}

/// The contents of a string value, basic (`"…"`, without escapes) or literal (`'…'`), or `None` if the value is not such a string.
fn string(value: &str) -> Option<String> {
    let inner = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .filter(|inner| !inner.contains(['"', '\\']))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
                .filter(|inner| !inner.contains('\''))
        })?;
    Some(inner.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(dependency: &Dependency) -> Vec<(&str, Option<&str>)> {
        dependency
            .fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_deref()))
            .collect()
    }

    #[test]
    fn reads_every_form_of_declaration() {
        let text = r#"
[workspace]
members = ["a"]

[workspace.dependencies]
plain = "=1.2.3" # a comment
table = { version = "=0.8.9", default-features = false, features = ["a", "b"] }
spread = { version = "=1.0.0", features = [
  "x", # a comment inside the array
  "y",
] }
local = { path = "crates/local" }
dotted.version = "=2.0.0"
dotted.features = ["z"]
"quoted" = '=3.0.0'

[workspace.dependencies.own-table]
version = "=4.0.0"
optional = true

[workspace.lints.rust]
unsafe_code = "forbid"
"#;
        let dependencies = workspace_dependencies(text).unwrap();
        let names: Vec<&str> = dependencies.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "plain",
                "table",
                "spread",
                "local",
                "dotted",
                "quoted",
                "own-table"
            ]
        );
        assert_eq!(fields(&dependencies[0]), [("version", Some("=1.2.3"))]);
        assert_eq!(
            fields(&dependencies[1]),
            [
                ("version", Some("=0.8.9")),
                ("default-features", None),
                ("features", None)
            ]
        );
        assert_eq!(dependencies[2].field("version"), Some("=1.0.0"));
        assert_eq!(dependencies[3].field("path"), Some("crates/local"));
        assert_eq!(dependencies[3].field("version"), None);
        assert_eq!(
            fields(&dependencies[4]),
            [("version", Some("=2.0.0")), ("features", None)]
        );
        assert_eq!(dependencies[5].field("version"), Some("=3.0.0"));
        assert_eq!(
            fields(&dependencies[6]),
            [("version", Some("=4.0.0")), ("optional", None)]
        );
        assert!(dependencies[6].has("optional"));
        assert_eq!(dependencies[1].line, 7);
    }

    #[test]
    fn reads_an_empty_or_missing_table() {
        assert_eq!(
            workspace_dependencies("[workspace.dependencies]\n"),
            Ok(Vec::new())
        );
        assert_eq!(
            workspace_dependencies("[package]\nname = \"x\"\n"),
            Ok(Vec::new())
        );
    }

    #[test]
    fn rejects_what_it_cannot_read() {
        for text in [
            "[workspace.dependencies]\na = \"\"\"=1.0.0\"\"\"\n",
            "[workspace.dependencies]\na = { version = \"=1.0.0\"\n",
            "[workspace.dependencies]\na = 1\n",
            "[workspace.dependencies]\na.b.c = \"x\"\n",
            "[workspace.dependencies]\njust a line\n",
            "[workspace.dependencies]\na = { b.c = \"x\" }\n",
            "[workspace.dependencies\na = \"=1.0.0\"\n",
            "[workspace.dependencies]\na = \"open\n",
        ] {
            assert!(workspace_dependencies(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn leaves_strings_with_escapes_unread() {
        // A version with an escape is reported as "not a string" by the check rather than decoded here.
        let dependencies = workspace_dependencies(
            "[workspace.dependencies]\na = { version = \"=1.0\\u002e0\" }\n",
        )
        .unwrap();
        assert_eq!(fields(&dependencies[0]), [("version", None)]);
    }
}
