//! A reader for the slice of TOML that `conformance/manifest.toml` is written in.
//!
//! The workspace has no TOML dependency and this is the only file in the tree that needs one, so
//! the subset is read here instead — the same trade `kiln_test262` makes for Test262's YAML-shaped
//! frontmatter, and for the same reason: a whole parser and its dependency tree is a poor price
//! for table headers and string values.
//!
//! Everything outside the subset is a hard error rather than a guess. A manifest is a claim about
//! what a corpus expects, and a reader that quietly skips a line it does not understand turns a
//! typo into a silently missing test — which is precisely the failure this corpus exists to make
//! impossible.

use std::fmt::{self, Display, Formatter};

/// A value in the supported subset.
///
/// `Integer` earns its place only because `[[fixture]]` records a byte count; no caller reads one.
/// Rejecting it would mean the reader could not read the repository's own manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    String(String),
    Array(Vec<String>),
    Integer(i64),
}

impl Value {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[String]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::String(_) => "string",
            Self::Array(_) => "array",
            Self::Integer(_) => "integer",
        }
    }
}

/// One key and its value, with the line it was written on so a schema complaint can point at the
/// key rather than at the table it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) key: String,
    pub(crate) value: Value,
    pub(crate) line: usize,
}

/// One table in the document, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table {
    /// The header name, for example `test` for `[[test]]` or `corpus.notes` for a sub-table.
    pub(crate) name: String,
    /// Whether the header was `[[name]]` (an array element) rather than `[name]`.
    pub(crate) is_array_element: bool,
    /// One-based line of the header, which locates an entry that is missing a key entirely.
    pub(crate) line: usize,
    pub(crate) entries: Vec<Entry>,
}

impl Table {
    pub(crate) fn get(&self, key: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.key == key)
    }
}

/// A syntax error, located by one-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TomlError {
    pub(crate) line: usize,
    pub(crate) message: String,
}

impl Display for TomlError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

fn error(line: usize, message: impl Into<String>) -> TomlError {
    TomlError {
        line,
        message: message.into(),
    }
}

/// Read a document into its tables, in source order.
///
/// Key/value pairs before the first header belong to no table and are rejected: the manifest has
/// no top-level keys, and accepting them would mean silently dropping them.
pub(crate) fn parse(text: &str) -> Result<Vec<Table>, TomlError> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tables: Vec<Table> = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line_number = index + 1;
        let line = lines[index].trim();
        index += 1;

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(header) = parse_header(line, line_number)? {
            tables.push(Table {
                name: header.0,
                is_array_element: header.1,
                line: line_number,
                entries: Vec::new(),
            });
            continue;
        }

        let (key, rest) = split_key(line, line_number)?;
        let value = read_value(rest, &lines, &mut index, line_number)?;
        let table = tables.last_mut().ok_or_else(|| {
            error(
                line_number,
                format!("key `{key}` appears before any table header"),
            )
        })?;
        if table.get(&key).is_some() {
            return Err(error(
                line_number,
                format!("duplicate key `{key}` in table `{}`", table.name),
            ));
        }
        table.entries.push(Entry {
            key,
            value,
            line: line_number,
        });
    }

    Ok(tables)
}

/// `Ok(None)` when the line is not a header at all.
fn parse_header(line: &str, line_number: usize) -> Result<Option<(String, bool)>, TomlError> {
    let Some(rest) = line.strip_prefix('[') else {
        return Ok(None);
    };
    let (body, is_array_element) = match rest.strip_prefix('[') {
        Some(inner) => (
            inner
                .strip_suffix("]]")
                .ok_or_else(|| error(line_number, "unterminated `[[table]]` header"))?,
            true,
        ),
        None => (
            rest.strip_suffix(']')
                .ok_or_else(|| error(line_number, "unterminated `[table]` header"))?,
            false,
        ),
    };
    let name = body.trim();
    if name.is_empty() {
        return Err(error(line_number, "empty table header"));
    }
    Ok(Some((name.to_owned(), is_array_element)))
}

fn split_key(line: &str, line_number: usize) -> Result<(String, &str), TomlError> {
    let (key, rest) = line
        .split_once('=')
        .ok_or_else(|| error(line_number, format!("expected `key = value`, got {line:?}")))?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(error(line_number, format!("invalid key {key:?}")));
    }
    Ok((key.to_owned(), rest.trim()))
}

/// Read the value starting at `rest`, consuming further lines for a multi-line string.
fn read_value(
    rest: &str,
    lines: &[&str],
    index: &mut usize,
    line_number: usize,
) -> Result<Value, TomlError> {
    if let Some(after_open) = rest.strip_prefix("\"\"\"") {
        return read_multiline_string(after_open, lines, index, line_number).map(Value::String);
    }
    if rest.starts_with('"') {
        return read_basic_string(rest, line_number).map(Value::String);
    }
    if rest.starts_with('[') {
        return read_array(rest, line_number).map(Value::Array);
    }
    rest.parse::<i64>().map(Value::Integer).map_err(|_| {
        error(
            line_number,
            format!("unsupported value {rest:?}: expected a string, string array, or integer"),
        )
    })
}

/// TOML trims a newline immediately after the opening delimiter, which is what lets the manifest
/// record an expected stdout as an indented block whose first line is the program's first line.
fn read_multiline_string(
    after_open: &str,
    lines: &[&str],
    index: &mut usize,
    open_line: usize,
) -> Result<String, TomlError> {
    let mut raw = String::new();
    if let Some(body) = after_open.strip_suffix("\"\"\"") {
        raw.push_str(body);
        return unescape(&raw, open_line);
    }
    if !after_open.is_empty() {
        raw.push_str(after_open);
        raw.push('\n');
    }
    loop {
        let line = lines.get(*index).ok_or_else(|| {
            error(
                open_line,
                "unterminated multi-line string (missing closing `\"\"\"`)",
            )
        })?;
        *index += 1;
        if let Some(body) = line.trim_end().strip_suffix("\"\"\"") {
            raw.push_str(body);
            return unescape(&raw, open_line);
        }
        raw.push_str(line);
        raw.push('\n');
    }
}

fn read_basic_string(rest: &str, line_number: usize) -> Result<String, TomlError> {
    let (raw, trailing) = scan_quoted(rest, line_number)?;
    if !trailing.trim().is_empty() && !trailing.trim_start().starts_with('#') {
        return Err(error(
            line_number,
            format!("unexpected text after string value: {trailing:?}"),
        ));
    }
    unescape(&raw, line_number)
}

fn read_array(rest: &str, line_number: usize) -> Result<Vec<String>, TomlError> {
    let body = rest
        .strip_prefix('[')
        .and_then(|rest| rest.trim_end().strip_suffix(']'))
        .ok_or_else(|| {
            error(
                line_number,
                "arrays must open and close on one line in this manifest subset",
            )
        })?;
    let mut values = Vec::new();
    let mut remainder = body.trim();
    while !remainder.is_empty() {
        if !remainder.starts_with('"') {
            return Err(error(
                line_number,
                format!("array elements must be strings, got {remainder:?}"),
            ));
        }
        let (raw, trailing) = scan_quoted(remainder, line_number)?;
        values.push(unescape(&raw, line_number)?);
        remainder = trailing.trim_start();
        remainder = match remainder.strip_prefix(',') {
            Some(next) => next.trim_start(),
            None if remainder.is_empty() => remainder,
            None => {
                return Err(error(
                    line_number,
                    format!("expected `,` between array elements, got {remainder:?}"),
                ));
            }
        };
    }
    Ok(values)
}

/// Split `"…"` into its raw (still escaped) body and whatever follows the closing quote.
fn scan_quoted(text: &str, line_number: usize) -> Result<(String, &str), TomlError> {
    let mut characters = text.char_indices();
    let Some((_, '"')) = characters.next() else {
        return Err(error(line_number, "expected a quoted string"));
    };
    let mut escaped = false;
    for (offset, character) in characters {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => {
                return Ok((
                    text[1..offset].to_owned(),
                    &text[offset + character.len_utf8()..],
                ));
            }
            _ => {}
        }
    }
    Err(error(line_number, "unterminated string"))
}

/// Apply the basic-string escapes the manifest can contain.
///
/// The line-ending backslash matters most: the manifest wraps its prose fields to a column with
/// it, so without this a `catches` field would come back full of hard newlines.
fn unescape(raw: &str, line_number: usize) -> Result<String, TomlError> {
    let mut output = String::with_capacity(raw.len());
    let mut characters = raw.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match characters.next() {
            Some('n') => output.push('\n'),
            Some('t') => output.push('\t'),
            Some('r') => output.push('\r'),
            Some('"') => output.push('"'),
            Some('\\') => output.push('\\'),
            Some('\n') => {
                while characters.as_str().starts_with([' ', '\t', '\n', '\r']) {
                    characters.next();
                }
            }
            Some(other) => {
                return Err(error(
                    line_number,
                    format!("unsupported escape `\\{other}`"),
                ));
            }
            None => return Err(error(line_number, "string ends with a dangling backslash")),
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(table: &'a Table, key: &str) -> Option<&'a Value> {
        table.get(key).map(|entry| &entry.value)
    }

    #[test]
    fn reads_headers_scalars_arrays_and_multiline_strings() {
        let tables = parse(
            "# leading comment\n\
             [corpus]\n\
             name = \"probe\"\n\
             \n\
             [[test]]\n\
             id = \"a\"\n\
             features = [\"one\", \"two\"]\n\
             expected_stdout = \"\"\"\n\
             hello\n\
             \"\"\"\n\
             [[fixture]]\n\
             bytes = 12\n",
        )
        .expect("document is in the supported subset");

        assert_eq!(tables.len(), 3);
        assert_eq!(tables[0].name, "corpus");
        assert!(!tables[0].is_array_element);
        assert_eq!(value(&tables[1], "id").and_then(Value::as_str), Some("a"));
        assert!(tables[1].is_array_element);
        assert_eq!(
            value(&tables[1], "features").and_then(Value::as_array),
            Some(["one".to_owned(), "two".to_owned()].as_slice())
        );
        assert_eq!(
            value(&tables[1], "expected_stdout").and_then(Value::as_str),
            Some("hello\n"),
            "the newline after the opening delimiter is trimmed, the final one is kept"
        );
        assert_eq!(
            tables[1].get("expected_stdout").map(|entry| entry.line),
            Some(8),
            "a value is located by the line its key is written on"
        );
        assert_eq!(value(&tables[2], "bytes"), Some(&Value::Integer(12)));
    }

    #[test]
    fn a_line_ending_backslash_joins_wrapped_prose() {
        let tables = parse("[[test]]\ncatches = \"\"\"\nfirst \\\n  second\n\"\"\"\n")
            .expect("document is in the supported subset");
        assert_eq!(
            value(&tables[0], "catches").and_then(Value::as_str),
            Some("first second\n")
        );
    }

    #[test]
    fn malformed_documents_are_rejected_with_a_line() {
        let cases = [
            ("[[test]]\nid = \"a\"\nid = \"b\"\n", 3, "duplicate key"),
            ("id = \"a\"\n", 1, "before any table header"),
            ("[[test]]\nid = \"unterminated\n", 2, "unterminated string"),
            (
                "[[test]]\nbody = \"\"\"\nno end\n",
                2,
                "unterminated multi-line",
            ),
            ("[[test]]\nid true\n", 2, "expected `key = value`"),
            ("[test\n", 1, "unterminated `[table]` header"),
            ("[[test]\n", 1, "unterminated `[[table]]` header"),
            ("[[test]]\nflag = true\n", 2, "unsupported value"),
            ("[[test]]\nfeatures = [1, 2]\n", 2, "must be strings"),
            ("[[test]]\nid = \"a\" \"b\"\n", 2, "unexpected text"),
        ];
        for (source, line, needle) in cases {
            let error = parse(source).expect_err("document is outside the subset");
            assert_eq!(error.line, line, "for {source:?}");
            assert!(
                error.message.contains(needle),
                "expected {needle:?} in {:?}",
                error.message
            );
        }
    }
}
