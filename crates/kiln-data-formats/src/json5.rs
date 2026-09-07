//! Pure-Rust parser for JSON5 and JSONC (JSON with Comments).
//!
//! Enforces strict security bounds (`MAX_NESTING_DEPTH = 128`) to prevent stack overflow DOS.

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

pub fn parse_json5(input: &str) -> Result<serde_json::Value, ParseError> {
    let mut parser = Parser::new(input);
    parser.skip_whitespace_and_comments()?;
    let val = parser.parse_value(0)?;
    parser.skip_whitespace_and_comments()?;
    if parser.peek().is_some() {
        return Err(parser.error("unexpected trailing character after JSON value"));
    }
    Ok(val)
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

    fn peek_next(&self) -> Option<u8> {
        if self.pos + 1 < self.chars.len() {
            Some(self.chars[self.pos + 1])
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

    fn skip_whitespace_and_comments(&mut self) -> Result<(), ParseError> {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.advance();
                }
                b'/' => {
                    match self.peek_next() {
                        Some(b'/') => {
                            // Line comment
                            self.advance();
                            self.advance();
                            while let Some(ch) = self.advance() {
                                if ch == b'\n' {
                                    break;
                                }
                            }
                        }
                        Some(b'*') => {
                            // Block comment
                            self.advance();
                            self.advance();
                            let mut terminated = false;
                            while let Some(ch) = self.advance() {
                                if ch == b'*' && self.peek() == Some(b'/') {
                                    self.advance();
                                    terminated = true;
                                    break;
                                }
                            }
                            if !terminated {
                                return Err(self.error("unterminated multiline comment"));
                            }
                        }
                        _ => break,
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<serde_json::Value, ParseError> {
        if depth > MAX_NESTING_DEPTH {
            return Err(self.error(format!(
                "exceeded maximum nesting depth of {MAX_NESTING_DEPTH}"
            )));
        }

        self.skip_whitespace_and_comments()?;
        let Some(b) = self.peek() else {
            return Err(self.error("unexpected end of input, expected value"));
        };

        match b {
            b'{' => self.parse_object(depth + 1),
            b'[' => self.parse_array(depth + 1),
            b'"' | b'\'' => self.parse_string().map(serde_json::Value::String),
            b't' if self.starts_with(b"true") => {
                self.consume_exact(b"true");
                Ok(serde_json::Value::Bool(true))
            }
            b'f' if self.starts_with(b"false") => {
                self.consume_exact(b"false");
                Ok(serde_json::Value::Bool(false))
            }
            b'n' if self.starts_with(b"null") => {
                self.consume_exact(b"null");
                Ok(serde_json::Value::Null)
            }
            b'I' if self.starts_with(b"Infinity") => {
                self.consume_exact(b"Infinity");
                Ok(serde_json::Value::Null)
            }
            b'N' if self.starts_with(b"NaN") => {
                self.consume_exact(b"NaN");
                Ok(serde_json::Value::Null)
            }
            b'+' if self.starts_with(b"+Infinity") => {
                self.consume_exact(b"+Infinity");
                Ok(serde_json::Value::Null)
            }
            b'-' if self.starts_with(b"-Infinity") => {
                self.consume_exact(b"-Infinity");
                Ok(serde_json::Value::Null)
            }
            b'+' | b'-' | b'0'..=b'9' | b'.' => self.parse_number(),
            _ => Err(self.error(format!(
                "unexpected character '{}' in JSON value",
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

    fn parse_object(&mut self, depth: usize) -> Result<serde_json::Value, ParseError> {
        self.advance(); // consume '{'
        let mut map = serde_json::Map::new();

        loop {
            self.skip_whitespace_and_comments()?;
            match self.peek() {
                Some(b'}') => {
                    self.advance();
                    break;
                }
                None => return Err(self.error("unterminated object, expected '}'")),
                _ => {}
            }

            let key = self.parse_key()?;
            self.skip_whitespace_and_comments()?;

            match self.peek() {
                Some(b':') => {
                    self.advance();
                }
                _ => return Err(self.error("expected ':' after property key")),
            }

            let val = self.parse_value(depth)?;
            map.insert(key, val);

            self.skip_whitespace_and_comments()?;
            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b'}') => {
                    self.advance();
                    break;
                }
                None => return Err(self.error("unterminated object, expected ',' or '}'")),
                _ => return Err(self.error("expected ',' or '}' after property value")),
            }
        }

        Ok(serde_json::Value::Object(map))
    }

    fn parse_key(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(b'"' | b'\'') => self.parse_string(),
            Some(b) if is_ident_start(b) => {
                let start = self.pos;
                while let Some(ch) = self.peek() {
                    if is_ident_part(ch) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                let key = std::str::from_utf8(&self.chars[start..self.pos])
                    .map_err(|_| self.error("invalid UTF-8 in identifier key"))?;
                Ok(key.to_string())
            }
            _ => Err(self.error("expected property key")),
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<serde_json::Value, ParseError> {
        self.advance(); // consume '['
        let mut elements = Vec::new();

        loop {
            self.skip_whitespace_and_comments()?;
            match self.peek() {
                Some(b']') => {
                    self.advance();
                    break;
                }
                None => return Err(self.error("unterminated array, expected ']'")),
                _ => {}
            }

            let val = self.parse_value(depth)?;
            elements.push(val);

            self.skip_whitespace_and_comments()?;
            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b']') => {
                    self.advance();
                    break;
                }
                None => return Err(self.error("unterminated array, expected ',' or ']'")),
                _ => return Err(self.error("expected ',' or ']' after array element")),
            }
        }

        Ok(serde_json::Value::Array(elements))
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        let quote = self.advance().unwrap(); // '"' or '\''
        let mut s = String::new();

        while let Some(b) = self.peek() {
            if b == quote {
                self.advance();
                return Ok(s);
            }
            if b == b'\\' {
                self.advance();
                match self.advance() {
                    Some(b'b') => s.push('\x08'),
                    Some(b'f') => s.push('\x0C'),
                    Some(b'n') => s.push('\n'),
                    Some(b'r') => s.push('\r'),
                    Some(b't') => s.push('\t'),
                    Some(b'v') => s.push('\x0B'),
                    Some(b'0') => s.push('\0'),
                    Some(b'\\') => s.push('\\'),
                    Some(b'"') => s.push('"'),
                    Some(b'\'') => s.push('\''),
                    Some(b'/') => s.push('/'),
                    Some(b'\n') => {
                        // Escaped newline line-continuation
                    }
                    Some(b'\r') => {
                        if self.peek() == Some(b'\n') {
                            self.advance();
                        }
                    }
                    Some(b'x') => {
                        let h1 = self
                            .advance()
                            .ok_or_else(|| self.error("incomplete \\x escape"))?;
                        let h2 = self
                            .advance()
                            .ok_or_else(|| self.error("incomplete \\x escape"))?;
                        let val = hex_val(h1)? * 16 + hex_val(h2)?;
                        let ch =
                            char::from_u32(val).ok_or_else(|| self.error("invalid hex escape"))?;
                        s.push(ch);
                    }
                    Some(b'u') => {
                        let h1 = self
                            .advance()
                            .ok_or_else(|| self.error("incomplete \\u escape"))?;
                        let h2 = self
                            .advance()
                            .ok_or_else(|| self.error("incomplete \\u escape"))?;
                        let h3 = self
                            .advance()
                            .ok_or_else(|| self.error("incomplete \\u escape"))?;
                        let h4 = self
                            .advance()
                            .ok_or_else(|| self.error("incomplete \\u escape"))?;
                        let code = hex_val(h1)? * 4096
                            + hex_val(h2)? * 256
                            + hex_val(h3)? * 16
                            + hex_val(h4)?;
                        let ch = char::from_u32(code)
                            .ok_or_else(|| self.error("invalid unicode escape"))?;
                        s.push(ch);
                    }
                    Some(other) => s.push(other as char),
                    None => return Err(self.error("unterminated string escape")),
                }
            } else {
                self.advance();
                s.push(b as char);
            }
        }

        Err(self.error("unterminated string literal"))
    }

    fn parse_number(&mut self) -> Result<serde_json::Value, ParseError> {
        let start = self.pos;
        let mut is_negative = false;

        if self.peek() == Some(b'+') {
            self.advance();
        } else if self.peek() == Some(b'-') {
            is_negative = true;
            self.advance();
        }

        // Hex number check
        if self.peek() == Some(b'0')
            && (self.peek_next() == Some(b'x') || self.peek_next() == Some(b'X'))
        {
            self.advance(); // '0'
            self.advance(); // 'x'
            let hex_start = self.pos;
            while let Some(ch) = self.peek() {
                if ch.is_ascii_hexdigit() {
                    self.advance();
                } else {
                    break;
                }
            }
            if self.pos == hex_start {
                return Err(self.error("expected hexadecimal digits after 0x"));
            }
            let hex_str = std::str::from_utf8(&self.chars[hex_start..self.pos])
                .map_err(|_| self.error("invalid hex string"))?;
            let mut val = i64::from_str_radix(hex_str, 16)
                .map_err(|_| self.error("hex literal out of range"))?;
            if is_negative {
                val = -val;
            }
            return Ok(serde_json::Value::Number(val.into()));
        }

        // Float or integer
        let mut is_float = false;
        while let Some(ch) = self.peek() {
            match ch {
                b'0'..=b'9' => {
                    self.advance();
                }
                b'.' => {
                    is_float = true;
                    self.advance();
                }
                b'e' | b'E' => {
                    is_float = true;
                    self.advance();
                    if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
                        self.advance();
                    }
                }
                _ => break,
            }
        }

        let num_str = std::str::from_utf8(&self.chars[start..self.pos])
            .map_err(|_| self.error("invalid UTF-8 in number"))?;

        if is_float
            || num_str.starts_with('.')
            || num_str.starts_with("+.")
            || num_str.starts_with("-.")
        {
            let f: f64 = num_str
                .parse()
                .map_err(|_| self.error(format!("invalid float literal '{num_str}'")))?;
            let n = serde_json::Number::from_f64(f).ok_or_else(|| {
                self.error("float value is non-finite or not representable in JSON")
            })?;
            Ok(serde_json::Value::Number(n))
        } else {
            let i: i64 = num_str
                .parse()
                .map_err(|_| self.error(format!("invalid integer literal '{num_str}'")))?;
            Ok(serde_json::Value::Number(i.into()))
        }
    }
}

fn hex_val(b: u8) -> Result<u32, ParseError> {
    match b {
        b'0'..=b'9' => Ok((b - b'0') as u32),
        b'a'..=b'f' => Ok((b - b'a' + 10) as u32),
        b'A'..=b'F' => Ok((b - b'A' + 10) as u32),
        _ => Err(ParseError {
            message: format!("invalid hex digit '{}'", b as char),
            line: 1,
            col: 1,
        }),
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident_part(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_with_comments_and_trailing_commas() {
        let src = r#"
        // Line comment
        {
            /* Multiline
               comment */
            unquotedKey: "value",
            'singleQuote': 'singleValue',
            hex: 0x2A,
            leadingDot: .5,
            trailingComma: [1, 2, 3,],
        }
        "#;
        let val = parse_json5(src).unwrap();
        assert_eq!(val["unquotedKey"], "value");
        assert_eq!(val["singleQuote"], "singleValue");
        assert_eq!(val["hex"], 42);
        assert_eq!(val["leadingDot"], 0.5);
        assert_eq!(val["trailingComma"][0], 1);
        assert_eq!(val["trailingComma"][2], 3);
    }

    #[test]
    fn enforces_nesting_depth_limit() {
        let mut deeply_nested = String::new();
        for _ in 0..150 {
            deeply_nested.push('[');
        }
        for _ in 0..150 {
            deeply_nested.push(']');
        }
        let err = parse_json5(&deeply_nested).unwrap_err();
        assert!(err.message.contains("maximum nesting depth"));
    }
}
