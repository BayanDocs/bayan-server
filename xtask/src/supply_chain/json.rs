//! A small JSON reader for the documents these checks read: lines of the crates.io index, answers of the crates.io API, and the output of `cargo metadata`. It keeps strings, arrays and objects; numbers, `true`, `false` and `null` are read but not kept, because no check needs them.

/// How deeply arrays and objects may nest. The documents read here nest about six levels deep; the limit stops a malformed document from exhausting the stack.
const MAX_DEPTH: usize = 32;

/// A JSON value.
#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    /// A string, decoded.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object's members, in order.
    Object(Vec<(String, Value)>),
    /// A number, `true`, `false` or `null`.
    Other,
}

impl Value {
    /// The value of the member `key`, if this is an object that has one.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The text, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(elements) => Some(elements),
            _ => None,
        }
    }

    /// The text of the member `key`, if this is an object whose member `key` is a string.
    pub fn string(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }
}

/// Reads one JSON document.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader {
        bytes: text.as_bytes(),
        position: 0,
    };
    let value = reader.value(0)?;
    reader.skip_whitespace();
    if reader.position == reader.bytes.len() {
        Ok(value)
    } else {
        Err(reader.error("unexpected text after the value"))
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn error(&self, problem: &str) -> String {
        format!("invalid JSON at byte {}: {problem}", self.position)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn eat(&mut self, token: &[u8]) -> bool {
        if self.bytes[self.position..].starts_with(token) {
            self.position += token.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => {
                self.position += 1;
                self.string().map(Value::String)
            }
            Some(b'[') => {
                self.position += 1;
                let mut elements = Vec::new();
                self.skip_whitespace();
                if self.eat(b"]") {
                    return Ok(Value::Array(elements));
                }
                loop {
                    elements.push(self.value(depth + 1)?);
                    self.skip_whitespace();
                    if self.eat(b"]") {
                        return Ok(Value::Array(elements));
                    }
                    if !self.eat(b",") {
                        return Err(self.error("expected `,` or `]`"));
                    }
                }
            }
            Some(b'{') => {
                self.position += 1;
                let mut members = Vec::new();
                self.skip_whitespace();
                if self.eat(b"}") {
                    return Ok(Value::Object(members));
                }
                loop {
                    self.skip_whitespace();
                    if !self.eat(b"\"") {
                        return Err(self.error("expected a member name"));
                    }
                    let name = self.string()?;
                    self.skip_whitespace();
                    if !self.eat(b":") {
                        return Err(self.error("expected `:`"));
                    }
                    members.push((name, self.value(depth + 1)?));
                    self.skip_whitespace();
                    if self.eat(b"}") {
                        return Ok(Value::Object(members));
                    }
                    if !self.eat(b",") {
                        return Err(self.error("expected `,` or `}`"));
                    }
                }
            }
            _ if self.eat(b"true") || self.eat(b"false") || self.eat(b"null") => Ok(Value::Other),
            _ => {
                let start = self.position;
                while matches!(
                    self.peek(),
                    Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
                ) {
                    self.position += 1;
                }
                if self.bytes[start..self.position]
                    .iter()
                    .any(u8::is_ascii_digit)
                {
                    Ok(Value::Other)
                } else {
                    Err(self.error("expected a value"))
                }
            }
        }
    }

    /// Reads a string whose opening quote has been consumed, up to and including its closing quote.
    fn string(&mut self) -> Result<String, String> {
        let mut text = String::new();
        loop {
            let start = self.position;
            while !matches!(self.peek(), None | Some(b'"' | b'\\' | 0..=0x1f)) {
                self.position += 1;
            }
            // The input is a `&str` and the loop stops only at ASCII bytes, so this slice is whole UTF-8 characters.
            text.push_str(
                std::str::from_utf8(&self.bytes[start..self.position])
                    .map_err(|_| self.error("invalid UTF-8"))?,
            );
            match self.peek() {
                Some(b'"') => {
                    self.position += 1;
                    return Ok(text);
                }
                Some(b'\\') => {
                    self.position += 1;
                    let escaped = self
                        .peek()
                        .ok_or_else(|| self.error("unterminated string"))?;
                    self.position += 1;
                    match escaped {
                        b'"' => text.push('"'),
                        b'\\' => text.push('\\'),
                        b'/' => text.push('/'),
                        b'b' => text.push('\u{8}'),
                        b'f' => text.push('\u{c}'),
                        b'n' => text.push('\n'),
                        b'r' => text.push('\r'),
                        b't' => text.push('\t'),
                        b'u' => {
                            let unit = self.hex4()?;
                            let code = if (0xd800..0xdc00).contains(&unit) && self.eat(b"\\u") {
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return Err(self.error("invalid surrogate pair"));
                                }
                                0x1_0000 + ((unit - 0xd800) << 10) + (low - 0xdc00)
                            } else {
                                unit
                            };
                            text.push(
                                char::from_u32(code)
                                    .ok_or_else(|| self.error("invalid \\u escape"))?,
                            );
                        }
                        _ => return Err(self.error("invalid escape")),
                    }
                }
                Some(_) => return Err(self.error("control character in a string")),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .bytes
            .get(self.position..self.position + 4)
            .filter(|digits| digits.iter().all(u8::is_ascii_hexdigit))
            .ok_or_else(|| self.error("invalid \\u escape"))?;
        let mut code = 0;
        for digit in digits {
            code = code * 16 + char::from(*digit).to_digit(16).unwrap_or(0);
        }
        self.position += 4;
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_line_of_the_crates_io_index() {
        let line = r#"{"name":"hyper","vers":"1.12.0","deps":[{"name":"bytes","req":"^1.2","features":[],"optional":false,"default_features":true,"target":null,"kind":"normal"}],"cksum":"2c3e","features":{"client":[]},"yanked":false,"rust_version":"1.63","pubtime":"2026-10-06T15:57:18Z","v":2}"#;
        let value = parse(line).unwrap();
        assert_eq!(value.string("vers"), Some("1.12.0"));
        assert_eq!(value.string("pubtime"), Some("2026-10-06T15:57:18Z"));
        assert_eq!(value.get("yanked"), Some(&Value::Other));
        let deps = value.get("deps").and_then(Value::as_array).unwrap();
        assert_eq!(deps[0].string("name"), Some("bytes"));
        assert_eq!(deps[0].get("target"), Some(&Value::Other));
        assert_eq!(value.string("missing"), None);
        assert_eq!(value.string("deps"), None, "an array is not a string");
    }

    #[test]
    fn decodes_escapes() {
        let value = parse(r#"{"a":"q\"b\\s\/n\nt\tu\u00e9 \ud83d\ude00"}"#).unwrap();
        assert_eq!(value.string("a"), Some("q\"b\\s/n\nt\tu\u{e9} \u{1f600}"));
        let value = parse("\"\u{e9}\u{1f600} plain\"").unwrap();
        assert_eq!(value.as_str(), Some("\u{e9}\u{1f600} plain"));
    }

    #[test]
    fn reads_numbers_literals_and_whitespace() {
        let value = parse(
            " { \"n\" : -1.5e3 , \"t\":true,\"f\":false,\"z\":null, \"e\": [ ] , \"o\":{} } ",
        )
        .unwrap();
        assert_eq!(value.get("n"), Some(&Value::Other));
        assert_eq!(value.get("e"), Some(&Value::Array(Vec::new())));
        assert_eq!(value.get("o"), Some(&Value::Object(Vec::new())));
    }

    #[test]
    fn rejects_invalid_documents() {
        for text in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "{a:1}",
            "[1] 2",
            "\"open",
            "nul",
            "\"tab\there\"",
            "\"\\x\"",
            "\"\\u12\"",
            "\"\\ud800\\u0041\"",
            "-",
            &"[".repeat(100),
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }
}
