//! Ordered JSON reader for package metadata.
//!
//! `serde_json::Value` objects collapse duplicate keys and can reorder object members. The checker
//! needs source order for `exports` conditions and JS-like handling of escaped lone surrogates, so
//! this small grammar reader stores object pairs directly and delegates ordinary string decoding
//! to serde_json, with a WTF-8-preserving fallback for lone `\u` surrogates.
use crate::engine::json::Json;

const MAX_DEPTH: usize = 256;

pub fn parse(input: &[u8]) -> Option<Json> {
    let mut parser = Parser { input, at: 0 };
    parser.whitespace();
    let value = parser.value(0)?;
    parser.whitespace();
    (parser.at == input.len()).then_some(value)
}

struct Parser<'a> {
    input: &'a [u8],
    at: usize,
}
impl Parser<'_> {
    fn whitespace(&mut self) {
        while matches!(self.input.get(self.at), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.at += 1;
        }
    }
    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.whitespace();
        match self.input.get(self.at).copied()? {
            b'n' => {
                self.keyword(b"null")?;
                Some(Json::Null)
            }
            b't' => {
                self.keyword(b"true")?;
                Some(Json::Bool(true))
            }
            b'f' => {
                self.keyword(b"false")?;
                Some(Json::Bool(false))
            }
            b'"' => self.string().map(Json::String),
            b'[' => self.array(depth + 1),
            b'{' => self.object(depth + 1),
            b'-' | b'0'..=b'9' => self.number().map(Json::Number),
            _ => None,
        }
    }
    fn keyword(&mut self, word: &[u8]) -> Option<()> {
        let end = self.at.checked_add(word.len())?;
        (self.input.get(self.at..end)? == word).then_some(())?;
        self.at = end;
        Some(())
    }
    fn array(&mut self, depth: usize) -> Option<Json> {
        self.at += 1;
        self.whitespace();
        let mut items = Vec::new();
        if self.take(b']') {
            return Some(Json::Array(items));
        }
        loop {
            items.push(self.value(depth)?);
            self.whitespace();
            if self.take(b']') {
                return Some(Json::Array(items));
            }
            if !self.take(b',') {
                return None;
            }
            self.whitespace();
        }
    }
    fn object(&mut self, depth: usize) -> Option<Json> {
        self.at += 1;
        self.whitespace();
        let mut fields = Vec::new();
        if self.take(b'}') {
            return Some(Json::Object(fields));
        }
        loop {
            if self.input.get(self.at) != Some(&b'"') {
                return None;
            }
            let key = self.string()?;
            self.whitespace();
            if !self.take(b':') {
                return None;
            }
            let value = self.value(depth)?;
            fields.push((key, value));
            self.whitespace();
            if self.take(b'}') {
                return Some(Json::Object(fields));
            }
            if !self.take(b',') {
                return None;
            }
            self.whitespace();
        }
    }
    fn number(&mut self) -> Option<f64> {
        let start = self.at;
        if self.take(b'-') && self.at == self.input.len() {
            return None;
        }
        match self.input.get(self.at).copied()? {
            b'0' => {
                self.at += 1;
                if self.input.get(self.at).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
            }
            b'1'..=b'9' => {
                self.at += 1;
                while self.input.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
            }
            _ => return None,
        }
        if self.take(b'.') {
            let digits = self.at;
            while self.input.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
            if self.at == digits {
                return None;
            }
        }
        if matches!(self.input.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.input.get(self.at), Some(b'+' | b'-')) {
                self.at += 1;
            }
            let digits = self.at;
            while self.input.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
            if self.at == digits {
                return None;
            }
        }
        Some(kiln_number_core::string_to_number(
            &self.input[start..self.at],
        ))
    }
    fn string(&mut self) -> Option<Vec<u8>> {
        let start = self.at;
        self.at += 1;
        let mut escaped = false;
        while let Some(&b) = self.input.get(self.at) {
            match b {
                b'"' => {
                    self.at += 1;
                    let token = &self.input[start..self.at];
                    if let Ok(value) = serde_json::from_slice::<String>(token) {
                        return Some(value.into_bytes());
                    }
                    return decode_wtf8_string(token);
                }
                b'\\' => {
                    escaped = true;
                    self.at += 1;
                    let escape = *self.input.get(self.at)?;
                    self.at += 1;
                    match escape {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        b'u' => {
                            for _ in 0..4 {
                                if !self.input.get(self.at)?.is_ascii_hexdigit() {
                                    return None;
                                }
                                self.at += 1;
                            }
                        }
                        _ => return None,
                    }
                }
                0x00..=0x1f => return None,
                _ => self.at += 1,
            }
        }
        let _ = escaped;
        None
    }
    fn take(&mut self, byte: u8) -> bool {
        if self.input.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }
}

/// Decode the JSON string grammar while retaining lone UTF-16 surrogates as WTF-8.
fn decode_wtf8_string(token: &[u8]) -> Option<Vec<u8>> {
    if token.first() != Some(&b'"') || token.last() != Some(&b'"') {
        return None;
    }
    let mut out = Vec::with_capacity(token.len());
    let mut at = 1;
    let end = token.len() - 1;
    while at < end {
        match token[at] {
            b'\\' => {
                at += 1;
                match *token.get(at)? {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let unit = hex4(token.get(at + 1..at + 5)?)?;
                        at += 4;
                        let cp = if (0xd800..=0xdbff).contains(&unit)
                            && token.get(at + 1..at + 3) == Some(&b"\\u"[..])
                        {
                            let low = hex4(token.get(at + 3..at + 7)?)?;
                            if (0xdc00..=0xdfff).contains(&low) {
                                at += 6;
                                0x10000
                                    + ((u32::from(unit) - 0xd800) << 10)
                                    + (u32::from(low) - 0xdc00)
                            } else {
                                u32::from(unit)
                            }
                        } else {
                            u32::from(unit)
                        };
                        encode_wtf8(&mut out, cp);
                    }
                    _ => return None,
                }
                at += 1;
            }
            0x00..=0x1f => return None,
            _ => {
                let tail = token.get(at..end)?;
                let ch = core::str::from_utf8(tail).ok()?.chars().next()?;
                let size = ch.len_utf8();
                out.extend_from_slice(token.get(at..at + size)?);
                at += size;
            }
        }
    }
    Some(out)
}
fn hex4(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    let mut value = 0u16;
    for &b in bytes {
        let digit = (b as char).to_digit(16)? as u16;
        value = value.checked_mul(16)?.checked_add(digit)?;
    }
    Some(value)
}
fn encode_wtf8(out: &mut Vec<u8>, cp: u32) {
    match cp {
        0..=0x7f => out.push(cp as u8),
        0x80..=0x7ff => {
            out.push((0xc0 | (cp >> 6)) as u8);
            out.push((0x80 | (cp & 0x3f)) as u8);
        }
        0x800..=0xffff => {
            out.push((0xe0 | (cp >> 12)) as u8);
            out.push((0x80 | ((cp >> 6) & 0x3f)) as u8);
            out.push((0x80 | (cp & 0x3f)) as u8);
        }
        _ => {
            out.push((0xf0 | (cp >> 18)) as u8);
            out.push((0x80 | ((cp >> 12) & 0x3f)) as u8);
            out.push((0x80 | ((cp >> 6) & 0x3f)) as u8);
            out.push((0x80 | (cp & 0x3f)) as u8);
        }
    }
}
