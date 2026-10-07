//! Ordered JSON reader for package metadata.
//!
//! `serde_json::Value` objects collapse duplicate keys and can reorder object members. The checker
//! needs source order for `exports` conditions and JS-like handling of escaped lone surrogates, so
//! this small grammar reader stores object pairs directly and delegates ordinary string decoding
//! to serde_json, with a WTF-8-preserving fallback for lone `\u` surrogates.
use crate::engine::atom::Intern;
use crate::engine::hir::{
    ExprId, ExprKind, File, FileBuilder, FileKind, NameKind, Prop, PropKey, PropKind, StmtKind,
};
use crate::engine::json::Json;
use crate::engine::session::{Arena, Session};
use std::borrow::Cow;

const MAX_DEPTH: usize = 256;

pub fn parse(input: &[u8]) -> Option<Json> {
    let mut parser = Parser { input, at: 0 };
    parser.whitespace();
    let value = parser.value(0)?;
    parser.whitespace();
    (parser.at == input.len()).then_some(value)
}

/// Parses TypeScript's JSONC config syntax directly into checker HIR. The reader retains ordered
/// object properties and exact byte spans, including duplicate keys, comments and trailing commas.
pub fn parse_source_file<'s>(
    arena: &'s Arena,
    session: &'s Session,
    atoms: &dyn Intern,
    text: &[u8],
) -> File<'s> {
    let mut builder = FileBuilder {
        kind: FileKind::Json,
        source_len: text.len() as u32,
        text: Cow::Owned(text.to_vec()),
        ..FileBuilder::default()
    };
    let at = usize::from(text.starts_with(&[0xef, 0xbb, 0xbf])) * 3;
    let parsed = JsoncParser {
        input: text,
        at,
        expected: None,
    }
    .parse();
    match parsed {
        Ok(value) => {
            let root = lower_jsonc(&mut builder, atoms, value);
            let stmt = builder.stmt(StmtKind::ExportAssign(root), 0);
            builder.body = builder.list(&[stmt]);
        }
        Err((at, expected)) => {
            builder.has_errors = true;
            builder.has_parse_diagnostics = true;
            builder.syntax_errors = 1;
            builder.error_pos = at as u32;
            builder
                .diagnostics
                .push(crate::engine::hir::Diagnostic::new(
                    crate::engine::hir::DiagnosticKind::Parse,
                    (at as u32, at as u32),
                    1005,
                    &[expected],
                ));
        }
    }
    builder.into_arena(arena, session).0
}

enum JsoncValue {
    Null(u32, u32),
    Bool(bool, u32, u32),
    Number(f64, u32, u32),
    String(Vec<u8>, u32, u32),
    Array(Vec<JsoncValue>, u32, u32),
    Object(Vec<(Vec<u8>, u32, JsoncValue)>, u32, u32),
}

fn lower_jsonc(builder: &mut FileBuilder, atoms: &dyn Intern, value: JsoncValue) -> ExprId {
    let (kind, start, end) = match value {
        JsoncValue::Null(start, end) => (ExprKind::Null, start, end),
        JsoncValue::Bool(true, start, end) => (ExprKind::True, start, end),
        JsoncValue::Bool(false, start, end) => (ExprKind::False, start, end),
        JsoncValue::Number(number, start, end) => {
            if number.is_sign_negative() {
                let number = builder.number(-number);
                let operand = builder.expr(ExprKind::Number(number), start + 1, end);
                (
                    ExprKind::Unary {
                        op: crate::engine::hir::UnOp::Minus,
                        operand,
                    },
                    start,
                    end,
                )
            } else {
                let number = builder.number(number);
                (ExprKind::Number(number), start, end)
            }
        }
        JsoncValue::String(text, start, end) => (ExprKind::String(atoms.intern(&text)), start, end),
        JsoncValue::Array(values, start, end) => {
            let values: Vec<_> = values
                .into_iter()
                .map(|v| lower_jsonc(builder, atoms, v))
                .collect();
            let list = builder.list(&values);
            (ExprKind::Array(list), start, end)
        }
        JsoncValue::Object(properties, start, end) => {
            let mut props = Vec::with_capacity(properties.len());
            for (key, key_start, value) in properties {
                let value = lower_jsonc(builder, atoms, value);
                props.push(Prop {
                    kind: PropKind::Init,
                    key: PropKey::Name(atoms.intern(&key)),
                    name_kind: NameKind::StringLiteral,
                    value,
                    pos: key_start,
                    start: key_start,
                    end: builder[value].end,
                    postfix_token: 0,
                });
            }
            let props = builder.add_props(&props);
            (ExprKind::Object(props), start, end)
        }
    };
    builder.expr(kind, start, end)
}

struct JsoncParser<'a> {
    input: &'a [u8],
    at: usize,
    expected: Option<&'static [u8]>,
}
impl JsoncParser<'_> {
    fn parse(mut self) -> Result<JsoncValue, (usize, &'static [u8])> {
        self.trivia();
        let value = self
            .value(0)
            .ok_or((self.at, self.expected.unwrap_or(b"}")))?;
        self.trivia();
        if self.at == self.input.len() {
            Ok(value)
        } else {
            Err((self.at, b"}"))
        }
    }
    fn trivia(&mut self) {
        loop {
            while matches!(self.input.get(self.at), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                self.at += 1;
            }
            if self.input.get(self.at..self.at + 2) == Some(b"//") {
                self.at += 2;
                while !matches!(self.input.get(self.at), None | Some(b'\r' | b'\n')) {
                    self.at += 1;
                }
            } else if self.input.get(self.at..self.at + 2) == Some(b"/*") {
                self.at += 2;
                while self.at + 1 < self.input.len()
                    && self.input.get(self.at..self.at + 2) != Some(b"*/")
                {
                    self.at += 1;
                }
                if self.at + 1 < self.input.len() {
                    self.at += 2;
                } else {
                    self.at = self.input.len();
                }
            } else {
                break;
            }
        }
    }
    fn value(&mut self, depth: usize) -> Option<JsoncValue> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.trivia();
        let start = self.at as u32;
        let Some(&token) = self.input.get(self.at) else {
            self.expected = Some(b"{");
            return None;
        };
        let value = match token {
            b'n' => {
                self.keyword(b"null")?;
                JsoncValue::Null(start, self.at as u32)
            }
            b't' => {
                self.keyword(b"true")?;
                JsoncValue::Bool(true, start, self.at as u32)
            }
            b'f' => {
                self.keyword(b"false")?;
                JsoncValue::Bool(false, start, self.at as u32)
            }
            b'"' => {
                let text = self.string()?;
                JsoncValue::String(text, start, self.at as u32)
            }
            b'[' => self.array(depth + 1, start)?,
            b'{' => self.object(depth + 1, start)?,
            b'-' | b'0'..=b'9' => {
                let n = self.number()?;
                JsoncValue::Number(n, start, self.at as u32)
            }
            _ => {
                self.expected = Some(b"{");
                return None;
            }
        };
        Some(value)
    }
    fn keyword(&mut self, word: &[u8]) -> Option<()> {
        let end = self.at.checked_add(word.len())?;
        (self.input.get(self.at..end)? == word).then_some(())?;
        self.at = end;
        Some(())
    }
    fn array(&mut self, depth: usize, start: u32) -> Option<JsoncValue> {
        self.at += 1;
        self.trivia();
        let mut items = Vec::new();
        if self.take(b']') {
            return Some(JsoncValue::Array(items, start, self.at as u32));
        }
        loop {
            items.push(self.value(depth)?);
            self.trivia();
            if self.take(b']') {
                return Some(JsoncValue::Array(items, start, self.at as u32));
            }
            if !self.take(b',') {
                self.expected = Some(if self.at == self.input.len() {
                    b"]"
                } else {
                    b","
                });
                return None;
            }
            self.trivia();
            if self.take(b']') {
                return Some(JsoncValue::Array(items, start, self.at as u32));
            }
        }
    }
    fn object(&mut self, depth: usize, start: u32) -> Option<JsoncValue> {
        self.at += 1;
        self.trivia();
        let mut fields = Vec::new();
        if self.take(b'}') {
            return Some(JsoncValue::Object(fields, start, self.at as u32));
        }
        loop {
            if self.input.get(self.at) != Some(&b'"') {
                self.expected = Some(b"\"");
                return None;
            }
            let key_start = self.at as u32;
            let key = self.string()?;
            self.trivia();
            if !self.take(b':') {
                self.expected = Some(b":");
                return None;
            }
            let value = self.value(depth)?;
            fields.push((key, key_start, value));
            self.trivia();
            if self.take(b'}') {
                return Some(JsoncValue::Object(fields, start, self.at as u32));
            }
            if !self.take(b',') {
                self.expected = Some(if self.at == self.input.len() {
                    b"}"
                } else {
                    b","
                });
                return None;
            }
            self.trivia();
            if self.take(b'}') {
                return Some(JsoncValue::Object(fields, start, self.at as u32));
            }
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
        while let Some(&b) = self.input.get(self.at) {
            match b {
                b'"' => {
                    self.at += 1;
                    let token = &self.input[start..self.at];
                    return serde_json::from_slice::<String>(token)
                        .map(String::into_bytes)
                        .ok()
                        .or_else(|| decode_wtf8_string(token));
                }
                b'\\' => {
                    self.at += 1;
                    let e = *self.input.get(self.at)?;
                    self.at += 1;
                    match e {
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
                0..=0x1f => return None,
                _ => self.at += 1,
            }
        }
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
