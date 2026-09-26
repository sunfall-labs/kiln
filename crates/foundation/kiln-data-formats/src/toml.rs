//! Pure-Rust parser for TOML data formats compiling to `serde_json::Value`.
//!
//! Enforces strict security bounds (`MAX_NESTING_DEPTH = 128`).

use std::fmt;

pub const MAX_NESTING_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub col: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.message)
    }
}

impl std::error::Error for ParseError {}

pub fn parse_toml(input: &str) -> Result<serde_json::Value, ParseError> {
    let mut parser = Parser::new(input);
    parser.parse_document()
}

struct Parser<'a> {
    chars: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn error(&self, msg: impl Into<String>) -> ParseError {
        ParseError {
            message: msg.into(),
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> Option<u8> {
        if self.pos < self.chars.len() {
            Some(self.chars[self.pos])
        } else {
            None
        }
    }

    fn advance(&mut self) -> Option<u8> {
        if self.pos < self.chars.len() {
            let b = self.chars[self.pos];
            self.pos += 1;
            if b == b'\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            Some(b)
        } else {
            None
        }
    }

    fn skip_whitespace_inline(&mut self) {
        while let Some(b' ' | b'\t') = self.peek() {
            self.advance();
        }
    }

    fn skip_comment_if_any(&mut self) {
        if self.peek() == Some(b'#') {
            while let Some(ch) = self.advance() {
                if ch == b'\n' {
                    break;
                }
            }
        }
    }

    fn parse_document(&mut self) -> Result<serde_json::Value, ParseError> {
        let mut root = serde_json::Map::new();
        let mut current_table_path: Vec<String> = Vec::new();

        while self.pos < self.chars.len() {
            self.skip_whitespace_inline();
            self.skip_comment_if_any();

            let Some(b) = self.peek() else {
                break;
            };

            if b == b'\r' || b == b'\n' {
                self.advance();
                continue;
            }

            if b == b'[' {
                self.advance();
                let is_array_table = if self.peek() == Some(b'[') {
                    self.advance();
                    true
                } else {
                    false
                };

                let path = self.parse_table_header(is_array_table)?;
                current_table_path = path.clone();

                if is_array_table {
                    Self::insert_array_table(&mut root, &path, 0)?;
                } else {
                    Self::ensure_table(&mut root, &path, 0)?;
                }
                continue;
            }

            // Key-value pair
            let key_path = self.parse_key_path()?;
            self.skip_whitespace_inline();

            if self.advance() != Some(b'=') {
                return Err(self.error("expected '=' after key in TOML"));
            }

            self.skip_whitespace_inline();
            let value = self.parse_value(0)?;
            self.skip_whitespace_inline();
            self.skip_comment_if_any();

            if let Some(ch) = self.peek() {
                if ch != b'\r' && ch != b'\n' {
                    return Err(self.error("expected newline after key-value pair in TOML"));
                }
                self.advance();
            }

            // Insert into current table
            let mut full_path = current_table_path.clone();
            full_path.extend(key_path);
            Self::insert_value(&mut root, &full_path, value, 0)?;
        }

        Ok(serde_json::Value::Object(root))
    }

    fn parse_table_header(&mut self, is_array_table: bool) -> Result<Vec<String>, ParseError> {
        self.skip_whitespace_inline();
        let path = self.parse_key_path()?;
        self.skip_whitespace_inline();

        if is_array_table {
            if self.advance() != Some(b']') || self.advance() != Some(b']') {
                return Err(self.error("expected ']]' to close array table header"));
            }
        } else if self.advance() != Some(b']') {
            return Err(self.error("expected ']' to close table header"));
        }

        self.skip_whitespace_inline();
        self.skip_comment_if_any();
        if let Some(ch) = self.peek() {
            if ch != b'\r' && ch != b'\n' {
                return Err(self.error("expected newline after table header"));
            }
            self.advance();
        }

        Ok(path)
    }

    fn parse_key_path(&mut self) -> Result<Vec<String>, ParseError> {
        let mut keys = Vec::new();
        loop {
            self.skip_whitespace_inline();
            let key = self.parse_single_key()?;
            keys.push(key);
            self.skip_whitespace_inline();
            if self.peek() == Some(b'.') {
                self.advance();
            } else {
                break;
            }
        }
        Ok(keys)
    }

    fn parse_single_key(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(b'"' | b'\'') => self.parse_basic_string(),
            Some(_) => {
                let start = self.pos;
                while let Some(b) = self.peek() {
                    if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
                        self.advance();
                    } else {
                        break;
                    }
                }
                if self.pos == start {
                    return Err(self.error("expected bare key or quoted key in TOML"));
                }
                let s = std::str::from_utf8(&self.chars[start..self.pos])
                    .map_err(|_| self.error("invalid UTF-8 in key"))?;
                Ok(s.to_string())
            }
            None => Err(self.error("unexpected end of input when parsing key")),
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<serde_json::Value, ParseError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self.error(format!(
                "exceeded maximum nesting depth of {MAX_NESTING_DEPTH}"
            )));
        }

        self.skip_whitespace_inline();
        let Some(b) = self.peek() else {
            return Err(self.error("unexpected end of input, expected value"));
        };

        match b {
            b'"' | b'\'' => self.parse_string().map(serde_json::Value::String),
            b'[' => self.parse_array(depth + 1),
            b'{' => self.parse_inline_table(depth + 1),
            b't' if self.starts_with(b"true") => {
                self.consume_exact(b"true");
                Ok(serde_json::Value::Bool(true))
            }
            b'f' if self.starts_with(b"false") => {
                self.consume_exact(b"false");
                Ok(serde_json::Value::Bool(false))
            }
            b'+' | b'-' | b'0'..=b'9' => self.parse_number_or_date(),
            _ => Err(self.error(format!(
                "unexpected character '{}' in TOML value",
                b as char
            ))),
        }
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.chars[self.pos..].starts_with(prefix)
    }

    fn consume_exact(&mut self, token: &[u8]) {
        for _ in 0..token.len() {
            self.advance();
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        if self.starts_with(b"\"\"\"") {
            self.parse_multiline_basic_string()
        } else if self.starts_with(b"'''") {
            self.parse_multiline_literal_string()
        } else {
            self.parse_basic_string()
        }
    }

    fn parse_basic_string(&mut self) -> Result<String, ParseError> {
        let quote = self.advance().unwrap(); // '"' or '\''
        let is_literal = quote == b'\'';
        let mut s = String::new();

        while let Some(b) = self.advance() {
            if b == quote {
                return Ok(s);
            }
            if b == b'\\' && !is_literal {
                match self.advance() {
                    Some(b'n') => s.push('\n'),
                    Some(b'r') => s.push('\r'),
                    Some(b't') => s.push('\t'),
                    Some(b'\\') => s.push('\\'),
                    Some(b'"') => s.push('"'),
                    Some(other) => s.push(other as char),
                    None => return Err(self.error("unterminated string escape")),
                }
            } else {
                s.push(b as char);
            }
        }
        Err(self.error("unterminated string literal"))
    }

    fn parse_multiline_basic_string(&mut self) -> Result<String, ParseError> {
        self.consume_exact(b"\"\"\"");
        // A newline immediately following the opening delimiter will be trimmed.
        if self.peek() == Some(b'\r') {
            self.advance();
        }
        if self.peek() == Some(b'\n') {
            self.advance();
        }

        let mut s = String::new();
        while self.pos < self.chars.len() {
            if self.starts_with(b"\"\"\"") {
                self.consume_exact(b"\"\"\"");
                return Ok(s);
            }
            let b = self.advance().unwrap();
            if b == b'\\' {
                match self.advance() {
                    Some(b'n') => s.push('\n'),
                    Some(b'r') => s.push('\r'),
                    Some(b't') => s.push('\t'),
                    Some(b'\\') => s.push('\\'),
                    Some(b'"') => s.push('"'),
                    Some(b'\n') => {
                        // Line continuation: trim all subsequent whitespace
                        self.skip_whitespace_and_newlines();
                    }
                    Some(other) => s.push(other as char),
                    None => return Err(self.error("unterminated multiline string")),
                }
            } else {
                s.push(b as char);
            }
        }
        Err(self.error("unterminated multiline string"))
    }

    fn parse_multiline_literal_string(&mut self) -> Result<String, ParseError> {
        self.consume_exact(b"'''");
        if self.peek() == Some(b'\r') {
            self.advance();
        }
        if self.peek() == Some(b'\n') {
            self.advance();
        }

        let mut s = String::new();
        while self.pos < self.chars.len() {
            if self.starts_with(b"'''") {
                self.consume_exact(b"'''");
                return Ok(s);
            }
            s.push(self.advance().unwrap() as char);
        }
        Err(self.error("unterminated multiline literal string"))
    }

    fn skip_whitespace_and_newlines(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.advance();
                }
                b'#' => {
                    self.skip_comment_if_any();
                }
                _ => break,
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<serde_json::Value, ParseError> {
        self.advance(); // consume '['
        let mut elements = Vec::new();

        loop {
            self.skip_whitespace_and_newlines();
            if self.peek() == Some(b']') {
                self.advance();
                break;
            }
            if self.peek().is_none() {
                return Err(self.error("unterminated array in TOML"));
            }

            let val = self.parse_value(depth)?;
            elements.push(val);

            self.skip_whitespace_and_newlines();
            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b']') => {
                    self.advance();
                    break;
                }
                _ => return Err(self.error("expected ',' or ']' in TOML array")),
            }
        }

        Ok(serde_json::Value::Array(elements))
    }

    fn parse_inline_table(&mut self, depth: usize) -> Result<serde_json::Value, ParseError> {
        self.advance(); // consume '{'
        let mut map = serde_json::Map::new();

        loop {
            self.skip_whitespace_inline();
            if self.peek() == Some(b'}') {
                self.advance();
                break;
            }
            if self.peek().is_none() {
                return Err(self.error("unterminated inline table in TOML"));
            }

            let key = self.parse_single_key()?;
            self.skip_whitespace_inline();
            if self.advance() != Some(b'=') {
                return Err(self.error("expected '=' after key in inline table"));
            }
            self.skip_whitespace_inline();
            let val = self.parse_value(depth)?;
            map.insert(key, val);

            self.skip_whitespace_inline();
            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b'}') => {
                    self.advance();
                    break;
                }
                _ => return Err(self.error("expected ',' or '}' in inline table")),
            }
        }

        Ok(serde_json::Value::Object(map))
    }

    fn parse_number_or_date(&mut self) -> Result<serde_json::Value, ParseError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            match b {
                b'0'..=b'9'
                | b'+'
                | b'-'
                | b'.'
                | b'e'
                | b'E'
                | b'_'
                | b':'
                | b'T'
                | b'Z'
                | b'x'
                | b'o'
                | b'b' => {
                    self.advance();
                }
                _ => break,
            }
        }

        let raw = std::str::from_utf8(&self.chars[start..self.pos])
            .map_err(|_| self.error("invalid UTF-8 in scalar"))?;
        let cleaned = raw.replace('_', "");

        // Check if date/time string (contains '-' and 'T' or ':')
        if (cleaned.contains('-') && cleaned.contains(':'))
            || (cleaned.contains(':') && cleaned.len() >= 5)
        {
            return Ok(serde_json::Value::String(raw.to_string()));
        }

        if cleaned.starts_with("0x") || cleaned.starts_with("0X") {
            let i = i64::from_str_radix(&cleaned[2..], 16)
                .map_err(|_| self.error("invalid hex literal"))?;
            return Ok(serde_json::Value::Number(i.into()));
        }

        if cleaned.contains('.') || cleaned.contains('e') || cleaned.contains('E') {
            let f: f64 = cleaned
                .parse()
                .map_err(|_| self.error(format!("invalid float literal '{raw}'")))?;
            let n = serde_json::Number::from_f64(f)
                .ok_or_else(|| self.error("non-finite float not allowed in JSON"))?;
            Ok(serde_json::Value::Number(n))
        } else {
            let i: i64 = cleaned
                .parse()
                .map_err(|_| self.error(format!("invalid integer literal '{raw}'")))?;
            Ok(serde_json::Value::Number(i.into()))
        }
    }

    fn ensure_table(
        root: &mut serde_json::Map<String, serde_json::Value>,
        path: &[String],
        depth: usize,
    ) -> Result<(), ParseError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(ParseError {
                message: format!("exceeded maximum nesting depth of {MAX_NESTING_DEPTH}"),
                line: 1,
                col: 1,
            });
        }
        if path.is_empty() {
            return Ok(());
        }

        let key = &path[0];
        if path.len() == 1 {
            root.entry(key)
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            return Ok(());
        }

        let child = root
            .entry(key)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        match child {
            serde_json::Value::Object(map) => Self::ensure_table(map, &path[1..], depth + 1),
            serde_json::Value::Array(arr) => {
                if let Some(serde_json::Value::Object(map)) = arr.last_mut() {
                    return Self::ensure_table(map, &path[1..], depth + 1);
                }
                Err(ParseError {
                    message: format!("cannot enter table into non-object {key}"),
                    line: 1,
                    col: 1,
                })
            }
            _ => Err(ParseError {
                message: format!("cannot re-open key '{key}' as table"),
                line: 1,
                col: 1,
            }),
        }
    }

    fn insert_array_table(
        root: &mut serde_json::Map<String, serde_json::Value>,
        path: &[String],
        depth: usize,
    ) -> Result<(), ParseError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(ParseError {
                message: format!("exceeded maximum nesting depth of {MAX_NESTING_DEPTH}"),
                line: 1,
                col: 1,
            });
        }
        if path.is_empty() {
            return Ok(());
        }

        let key = &path[0];
        if path.len() == 1 {
            let arr = root
                .entry(key)
                .or_insert_with(|| serde_json::Value::Array(Vec::new()));
            if let serde_json::Value::Array(list) = arr {
                list.push(serde_json::Value::Object(serde_json::Map::new()));
                return Ok(());
            }
            return Err(ParseError {
                message: format!("key '{key}' is not an array of tables"),
                line: 1,
                col: 1,
            });
        }

        let child = root
            .entry(key)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        match child {
            serde_json::Value::Object(map) => Self::insert_array_table(map, &path[1..], depth + 1),
            serde_json::Value::Array(arr) => {
                if let Some(serde_json::Value::Object(map)) = arr.last_mut() {
                    Self::insert_array_table(map, &path[1..], depth + 1)
                } else {
                    Err(ParseError {
                        message: format!("array table {key} element is not an object"),
                        line: 1,
                        col: 1,
                    })
                }
            }
            _ => Err(ParseError {
                message: format!("key '{key}' is already a scalar"),
                line: 1,
                col: 1,
            }),
        }
    }

    fn insert_value(
        root: &mut serde_json::Map<String, serde_json::Value>,
        path: &[String],
        value: serde_json::Value,
        depth: usize,
    ) -> Result<(), ParseError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(ParseError {
                message: format!("exceeded maximum nesting depth of {MAX_NESTING_DEPTH}"),
                line: 1,
                col: 1,
            });
        }
        if path.is_empty() {
            return Ok(());
        }

        let key = &path[0];
        if path.len() == 1 {
            root.insert(key.clone(), value);
            return Ok(());
        }

        let child = root
            .entry(key)
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        match child {
            serde_json::Value::Object(map) => Self::insert_value(map, &path[1..], value, depth + 1),
            serde_json::Value::Array(arr) => {
                if let Some(serde_json::Value::Object(map)) = arr.last_mut() {
                    Self::insert_value(map, &path[1..], value, depth + 1)
                } else {
                    Err(ParseError {
                        message: format!("array {key} element is not an object"),
                        line: 1,
                        col: 1,
                    })
                }
            }
            _ => Err(ParseError {
                message: format!("cannot set subkey on scalar '{key}'"),
                line: 1,
                col: 1,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_toml_tables_and_primitives() {
        let toml = r#"
        title = "TOML Example"

        [owner]
        name = "Tom Preston-Werner"
        dob = 1979-05-27T07:32:00Z

        [database]
        enabled = true
        ports = [ 8000, 8001, 8002 ]
        data = [ ["delta", "phi"], [3.14] ]
        temp_targets = { cpu = 79.5, case = 72.0 }

        [[products]]
        name = "Hammer"
        sku = 738594937

        [[products]]
        name = "Nail"
        sku = 284758393
        color = "gray"
        "#;

        let val = parse_toml(toml).unwrap();
        assert_eq!(val["title"], "TOML Example");
        assert_eq!(val["owner"]["name"], "Tom Preston-Werner");
        assert_eq!(val["database"]["enabled"], true);
        assert_eq!(val["database"]["ports"][0], 8000);
        assert_eq!(val["database"]["data"][0][0], "delta");
        assert_eq!(val["database"]["temp_targets"]["cpu"], 79.5);
        assert_eq!(val["products"][0]["name"], "Hammer");
        assert_eq!(val["products"][1]["name"], "Nail");
    }
}
