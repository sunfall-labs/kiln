//! Test262 metadata parsing and deterministic test discovery.
//!
//! Test262 calls the comment at the beginning of a test file "frontmatter".  It
//! is YAML-shaped, but the subset used by the runner is intentionally parsed here
//! without pulling a YAML implementation (or its transitive dependency tree) into
//! the compiler.  This module keeps the original path and source on each test case;
//! later stages can therefore report diagnostics against the exact input they saw.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The phase at which a negative Test262 test is expected to fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Failure while parsing the source text.
    Parse,
    /// Failure while resolving bindings or modules.
    Resolution,
    /// Failure while executing the test.
    Runtime,
}

impl Phase {
    fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            "parse" => Ok(Self::Parse),
            "resolution" => Ok(Self::Resolution),
            "runtime" => Ok(Self::Runtime),
            value => Err(format!(
                "negative.phase must be one of parse, resolution, or runtime (got {value:?})"
            )),
        }
    }
}

/// The expected error for a negative test.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NegativeExpectation {
    /// When the failure is expected.
    pub phase: Phase,
    /// The JavaScript error constructor name, for example `SyntaxError`.
    ///
    /// The JavaScript error constructor name, for example `SyntaxError`.
    pub type_name: String,
}

/// Parsed fields from a Test262 frontmatter block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Human-readable test description, if supplied.
    pub description: Option<String>,
    /// Test flags, including `onlyStrict`, `noStrict`, `module`, `raw`, and `async`.
    /// Unknown flags are retained for forward compatibility (for example `generated`).
    pub flags: Vec<String>,
    /// Feature tags declared by the test.
    pub features: Vec<String>,
    /// Harness include files declared by the test.
    pub includes: Vec<String>,
    /// Expected failure, when this is a negative test.
    pub negative: Option<NegativeExpectation>,
}

/// A source file and its parsed Test262 metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    /// The path supplied by the caller, joined with the discovered relative path.
    pub path: PathBuf,
    /// The unmodified source, including its frontmatter comment.
    pub source: String,
    /// Metadata parsed from `source`.
    pub metadata: Metadata,
}

impl TestCase {
    /// The original path, without normalizing or rewriting it.
    pub fn raw_path(&self) -> &Path {
        &self.path
    }

    /// The original source, including frontmatter and all line endings.
    pub fn raw_source(&self) -> &str {
        &self.source
    }
}

/// A location-aware frontmatter syntax error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// File in which the error occurred, when parsing through [`discover`].
    pub path: Option<PathBuf>,
    /// One-based line number within the source, when known.
    pub line: Option<usize>,
    /// Human-readable explanation.
    pub message: String,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            path: None,
            line: None,
            message: message.into(),
        }
    }

    fn at_line(message: impl Into<String>, line: usize) -> Self {
        Self {
            path: None,
            line: Some(line),
            message: message.into(),
        }
    }

    fn with_path(mut self, path: &Path) -> Self {
        self.path = Some(path.to_owned());
        self
    }
}

impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(f, "{}", path.display())?;
            if let Some(line) = self.line {
                write!(f, ":{line}")?;
            }
            write!(f, ": ")?;
        } else if let Some(line) = self.line {
            write!(f, "line {line}: ")?;
        }
        f.write_str(&self.message)
    }
}

impl Error for ParseError {}

/// An I/O or frontmatter error encountered while discovering tests.
#[derive(Debug)]
pub enum DiscoveryError {
    /// Reading a directory or source file failed.
    Io { path: PathBuf, source: io::Error },
    /// A source file had malformed or missing required frontmatter.
    Parse(ParseError),
}

impl Display for DiscoveryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Parse(error) => Display::fmt(error, f),
        }
    }
}

impl Error for DiscoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse(error) => Some(error),
        }
    }
}

/// Parse one source file into a [`TestCase`].
///
/// This is public primarily for callers that already have source text (and for
/// unit tests); [`discover`] is the normal entry point for a Test262 tree.
pub fn parse(path: impl AsRef<Path>, source: impl Into<String>) -> Result<TestCase, ParseError> {
    let path = path.as_ref().to_owned();
    let source = source.into();
    let metadata = parse_frontmatter(&source).map_err(|error| error.with_path(&path))?;
    Ok(TestCase {
        path,
        source,
        metadata,
    })
}

/// Discover all `.js` tests below `root` in deterministic path order.
///
/// Directories named `harness` and files whose name contains `_FIXTURE` are
/// excluded.  The latter are support sources in Test262 trees, not standalone
/// tests.  Every remaining JavaScript file must have a valid frontmatter block;
/// malformed or missing metadata returns an error rather than silently dropping
/// a test.
pub fn discover(root: impl AsRef<Path>) -> Result<Vec<TestCase>, DiscoveryError> {
    let root = root.as_ref();
    let mut paths = Vec::new();
    collect_js_paths(root, &mut paths)
        .map_err(|(path, source)| DiscoveryError::Io { path, source })?;
    paths.sort_by_key(|path| path_sort_key(path));

    paths
        .into_iter()
        .map(|path| {
            let source = fs::read_to_string(&path).map_err(|source| DiscoveryError::Io {
                path: path.clone(),
                source,
            })?;
            parse(path, source).map_err(DiscoveryError::Parse)
        })
        .collect()
}

/// Discover every test, reporting per-file metadata failures instead of abandoning the run.
///
/// [`discover`] stops at the first unreadable frontmatter, which is right for a small corpus the
/// project owns and wrong for an external suite: one malformed file out of fifty thousand ends a
/// whole run and hides every other result, so each fix reveals exactly one more problem. This
/// returns what it could read alongside what it could not, leaving the caller to decide how to
/// report the difference. A directory that cannot be walked is still an error, because that is a
/// broken invocation rather than a broken test.
pub fn discover_reporting(
    root: impl AsRef<Path>,
) -> Result<(Vec<TestCase>, Vec<ParseError>), DiscoveryError> {
    let root = root.as_ref();
    let mut paths = Vec::new();
    collect_js_paths(root, &mut paths)
        .map_err(|(path, source)| DiscoveryError::Io { path, source })?;
    paths.sort_by_key(|path| path_sort_key(path));

    let mut cases = Vec::with_capacity(paths.len());
    let mut failures = Vec::new();
    for path in paths {
        let source = fs::read_to_string(&path).map_err(|source| DiscoveryError::Io {
            path: path.clone(),
            source,
        })?;
        match parse(path, source) {
            Ok(case) => cases.push(case),
            Err(error) => failures.push(error),
        }
    }
    Ok((cases, failures))
}

fn path_sort_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn collect_js_paths(root: &Path, output: &mut Vec<PathBuf>) -> Result<(), (PathBuf, io::Error)> {
    let entries = fs::read_dir(root).map_err(|source| (root.to_owned(), source))?;
    for entry in entries {
        let entry = entry.map_err(|source| (root.to_owned(), source))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| (path.clone(), source))?;
        if file_type.is_dir() {
            if path.file_name().is_some_and(|name| name == "harness") {
                continue;
            }
            collect_js_paths(&path, output)?;
        } else if file_type.is_file()
            && path.extension().is_some_and(|extension| extension == "js")
            && !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains("_FIXTURE"))
        {
            output.push(path);
        }
    }
    Ok(())
}

fn parse_frontmatter(source: &str) -> Result<Metadata, ParseError> {
    let without_bom = source.strip_prefix('\u{feff}').unwrap_or(source);
    let Some(open_offset) = without_bom.find("/*---") else {
        return Err(ParseError::new(
            "missing Test262 frontmatter opening marker /*---",
        ));
    };
    // The marker must begin a line. "First construct in the file" was the wrong rule: it rejected
    // the entire suite this crate exists to read, because every upstream test opens with a
    // copyright header. Requiring only comments before it is closer but still rejects the twenty
    // hashbang negative tests, whose deliberately invalid source precedes their metadata.
    //
    // Every one of the suite's 53,580 frontmatter blocks starts a line, and none appears anywhere
    // else, so this admits all of them while still refusing a `/*---` embedded inside an
    // expression or a string. It is also how the suite's own runners locate the block.
    // Both terminators count: the line-terminator-normalisation tests use a bare carriage return
    // throughout, so looking only for `\n` would place the whole file on one line.
    let line_start = without_bom[..open_offset]
        .rfind(['\n', '\r'])
        .map_or(0, |terminator| terminator + 1);
    if without_bom[line_start..open_offset]
        .chars()
        .any(|character| !character.is_whitespace())
    {
        return Err(ParseError::new(
            "Test262 frontmatter marker /*--- must begin a line",
        ));
    }
    let body_start = open_offset + "/*---".len();
    let Some(close_relative) = without_bom[body_start..].find("---*/") else {
        return Err(ParseError::new(
            "unterminated Test262 frontmatter (missing ---*/)",
        ));
    };
    let body_end = body_start + close_relative;
    parse_body(&without_bom[body_start..body_end])
}

/// Split a frontmatter body on any JavaScript line terminator, counting `\r\n` as one.
///
/// `str::lines` splits only on `\n`, so the tests that deliberately use a bare carriage return —
/// the line-terminator-normalisation cases, among others — arrive as a single line whose
/// continuation then looks like stray indentation. Line numbers stay exact, which is why this
/// does not simply split on both characters and drop the empty pieces.
fn split_frontmatter_lines(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                lines.push(&body[start..index]);
                index += 1;
                start = index;
            }
            b'\r' => {
                lines.push(&body[start..index]);
                index += if bytes.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = index;
            }
            _ => index += 1,
        }
    }
    if start < bytes.len() {
        lines.push(&body[start..]);
    }
    lines
}

/// Remove the indentation shared by every content line of a frontmatter block.
///
/// One upstream test indents its whole block by a space. YAML reads that as an ordinary mapping,
/// so this parser should too: measuring indentation relative to the block rather than to column
/// zero costs nothing on the other 53,579 files, whose common indent is already zero.
fn strip_common_indent(lines: Vec<&str>) -> Vec<&str> {
    let common = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    if common == 0 {
        return lines;
    }
    lines
        .into_iter()
        .map(|line| line.get(common..).unwrap_or(""))
        .collect()
}

/// Whether a mapping value is a YAML block-scalar header — `|` or `>`, with optional chomping
/// (`-`, `+`) and explicit-indent digits — meaning the value lives on the following indented
/// lines rather than on this one.
fn is_block_scalar(value: &str) -> bool {
    let trimmed = value.trim();
    let Some(rest) = trimmed.strip_prefix(['|', '>']) else {
        return false;
    };
    rest.chars()
        .all(|character| character == '-' || character == '+' || character.is_ascii_digit())
}

fn parse_body(body: &str) -> Result<Metadata, ParseError> {
    let lines: Vec<&str> = strip_common_indent(split_frontmatter_lines(body));
    let mut metadata = Metadata::default();
    let mut seen_description = false;
    let mut seen_flags = false;
    let mut seen_features = false;
    let mut seen_includes = false;
    let mut seen_negative = false;
    let mut index = 0;

    while index < lines.len() {
        let line_number = index + 1;
        let raw = lines[index];
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            index += 1;
            continue;
        }
        if raw.chars().next().is_some_and(char::is_whitespace) {
            return Err(ParseError::at_line(
                "unexpected indented frontmatter line",
                line_number,
            ));
        }
        let (key, value) = split_mapping(raw).ok_or_else(|| {
            ParseError::at_line("expected a `key: value` frontmatter entry", line_number)
        })?;
        if key.is_empty() || !valid_key(key) {
            return Err(ParseError::at_line(
                format!("invalid frontmatter key {key:?}"),
                line_number,
            ));
        }

        match key {
            "description" => {
                if seen_description {
                    return Err(ParseError::at_line(
                        "duplicate description field",
                        line_number,
                    ));
                }
                seen_description = true;
                let (description, next) = parse_description(value, &lines, index)?;
                metadata.description = Some(description);
                index = next;
            }
            "flags" | "features" | "includes" => {
                let seen = match key {
                    "flags" => &mut seen_flags,
                    "features" => &mut seen_features,
                    _ => &mut seen_includes,
                };
                if *seen {
                    return Err(ParseError::at_line(
                        format!("duplicate {key} field"),
                        line_number,
                    ));
                }
                *seen = true;
                let (values, next) = parse_list_field(key, value, &lines, index)?;
                match key {
                    "flags" => metadata.flags = values,
                    "features" => metadata.features = values,
                    _ => metadata.includes = values,
                }
                index = next;
            }
            "negative" => {
                if seen_negative {
                    return Err(ParseError::at_line("duplicate negative field", line_number));
                }
                seen_negative = true;
                let (negative, next) = parse_negative(value, &lines, index)?;
                metadata.negative = Some(negative);
                index = next;
            }
            // The runner does not currently need locale metadata, but valid
            // scalar locale entries are deliberately accepted and ignored.
            "locale" | "localeCompare" => {
                // Locale metadata is not used by this crate.  It is still
                // common for it to be represented as a block list, so skip
                // the complete value rather than imposing a scalar shape.
                index = if value.trim().is_empty() {
                    skip_indented_block(&lines, index + 1)
                } else {
                    index + 1
                };
            }
            // Test262 has grown additional keys over time.  Keep accepting
            // scalar extension keys while still rejecting malformed structure
            // in fields that this parser owns.
            _ => {
                // `info: |` is the common shape upstream: a block scalar puts the value on the
                // following indented lines, so the key line is not the whole entry and the
                // continuation must be skipped rather than met as a stray indented line.
                if value.trim().is_empty() || is_block_scalar(value) {
                    index = skip_indented_block(&lines, index + 1);
                } else {
                    index += 1;
                }
            }
        }
    }

    Ok(metadata)
}

fn split_mapping(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    Some((line[..colon].trim(), line[colon + 1..].trim()))
}

fn valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars.next().is_some_and(|ch| ch.is_ascii_alphabetic())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn parse_description(
    value: &str,
    lines: &[&str],
    index: usize,
) -> Result<(String, usize), ParseError> {
    let value = strip_inline_comment(value).trim();
    if value.is_empty() {
        let (continuation, next) = consume_indented_text(lines, index + 1);
        if continuation.is_empty() {
            return Err(ParseError::at_line(
                "description must have a value",
                index + 1,
            ));
        }
        return Ok((continuation.join(" "), next));
    }
    // A YAML block scalar is the *whole* value -- `|` or `>` plus chomping and indent modifiers --
    // never a suffix. Reading a trailing `>` as an indicator misparsed descriptions that
    // legitimately end in one, such as "don't replace $<>", as an empty block: a wrong answer
    // about a real test rather than an unimplemented case.
    if is_block_scalar(value) {
        let (text, next) = consume_block_scalar(lines, index + 1, value.starts_with('|'));
        if text.is_empty() {
            return Err(ParseError::at_line(
                "description block must not be empty",
                index + 1,
            ));
        }
        return Ok((text, next));
    }
    let mut description = parse_scalar(value, "description")?;
    let (continuation, next) = consume_indented_text(lines, index + 1);
    if !continuation.is_empty() {
        description.push(' ');
        description.push_str(&continuation.join(" "));
    }
    Ok((description, next))
}

fn consume_indented_text<'a>(lines: &'a [&'a str], mut index: usize) -> (Vec<&'a str>, usize) {
    let mut values = Vec::new();
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if !line.chars().next().is_some_and(char::is_whitespace) {
            break;
        }
        values.push(line.trim());
        index += 1;
    }
    (values, index)
}

fn consume_block_scalar(lines: &[&str], mut index: usize, literal: bool) -> (String, usize) {
    let mut values = Vec::new();
    let mut saw_content = false;
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            values.push("");
            index += 1;
            continue;
        }
        if !line.chars().next().is_some_and(char::is_whitespace) {
            break;
        }
        saw_content = true;
        values.push(line.trim());
        index += 1;
    }
    if !saw_content {
        return (String::new(), index);
    }
    let text = if literal {
        values.join("\n")
    } else {
        let mut folded = String::new();
        let mut previous_blank = false;
        for value in values {
            if value.is_empty() {
                folded.push('\n');
                previous_blank = true;
            } else {
                if !folded.is_empty() && !previous_blank {
                    folded.push(' ');
                }
                folded.push_str(value);
                previous_blank = false;
            }
        }
        folded
    };
    (text, index)
}

fn parse_list_field(
    key: &str,
    value: &str,
    lines: &[&str],
    index: usize,
) -> Result<(Vec<String>, usize), ParseError> {
    let value = strip_inline_comment(value).trim();
    if value.starts_with('[') {
        return Ok((parse_inline_list(value, key, index + 1)?, index + 1));
    }
    if !value.is_empty() {
        return Err(ParseError::at_line(
            format!("{key} must be a bracket list or indented block list"),
            index + 1,
        ));
    }
    let mut values = Vec::new();
    let mut cursor = index + 1;
    while cursor < lines.len() {
        let line = lines[cursor];
        if line.trim().is_empty() {
            cursor += 1;
            continue;
        }
        if !line.chars().next().is_some_and(char::is_whitespace) {
            break;
        }
        let item = line.trim_start();
        let Some(item) = item.strip_prefix('-') else {
            return Err(ParseError::at_line(
                format!("{key} block list item must start with `-`"),
                cursor + 1,
            ));
        };
        let item = strip_inline_comment(item.trim()).trim();
        if item.is_empty() {
            return Err(ParseError::at_line(
                format!("{key} block list item must have a value"),
                cursor + 1,
            ));
        }
        values.push(parse_scalar(item, key).map_err(|mut error| {
            error.line = Some(cursor + 1);
            error
        })?);
        cursor += 1;
    }
    Ok((values, cursor))
}

fn parse_inline_list(value: &str, key: &str, line: usize) -> Result<Vec<String>, ParseError> {
    if !value.ends_with(']') {
        return Err(ParseError::at_line(
            format!("{key} bracket list is missing a closing `]`"),
            line,
        ));
    }
    let trailing = value[1..value.len() - 1].trim();
    if trailing.is_empty() {
        return Ok(Vec::new());
    }
    split_commas(trailing)
        .into_iter()
        .map(|item| {
            let item = strip_inline_comment(item.trim()).trim();
            if item.is_empty() {
                Err(ParseError::at_line(
                    format!("{key} bracket list contains an empty item"),
                    line,
                ))
            } else {
                parse_scalar(item, key).map_err(|mut error| {
                    error.line = Some(line);
                    error
                })
            }
        })
        .collect()
}

fn split_commas(value: &str) -> Vec<&str> {
    let mut values = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, character) {
            (Some(_), '\\') => escaped = true,
            (Some(current), character) if current == character => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, ',') => {
                values.push(&value[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    values.push(&value[start..]);
    values
}

fn parse_negative(
    value: &str,
    lines: &[&str],
    index: usize,
) -> Result<(NegativeExpectation, usize), ParseError> {
    let value = strip_inline_comment(value).trim();
    let mut phase = None;
    let mut type_name = None;
    let mut cursor = index + 1;
    if value.starts_with('{') {
        if !value.ends_with('}') {
            return Err(ParseError::at_line(
                "negative inline mapping is missing a closing `}`",
                index + 1,
            ));
        }
        for item in split_commas(value[1..value.len() - 1].trim()) {
            let Some((key, item_value)) = split_mapping(item.trim()) else {
                return Err(ParseError::at_line(
                    "negative inline mapping entries must be `key: value`",
                    index + 1,
                ));
            };
            assign_negative_field(key, item_value, index + 1, &mut phase, &mut type_name)?;
        }
    } else if !value.is_empty() {
        return Err(ParseError::at_line(
            "negative must be an indented mapping of phase and type",
            index + 1,
        ));
    } else {
        let mut saw_nested = false;
        while cursor < lines.len() {
            let line = lines[cursor];
            if line.trim().is_empty() {
                cursor += 1;
                continue;
            }
            if !line.chars().next().is_some_and(char::is_whitespace) {
                break;
            }
            saw_nested = true;
            let Some((key, item_value)) = split_mapping(line.trim()) else {
                return Err(ParseError::at_line(
                    "negative mapping entries must be `key: value`",
                    cursor + 1,
                ));
            };
            assign_negative_field(key, item_value, cursor + 1, &mut phase, &mut type_name)?;
            cursor += 1;
        }
        if !saw_nested {
            return Err(ParseError::at_line(
                "negative mapping must contain phase and type",
                index + 1,
            ));
        }
    }
    let phase =
        phase.ok_or_else(|| ParseError::at_line("negative mapping is missing phase", index + 1))?;
    let type_name = type_name
        .ok_or_else(|| ParseError::at_line("negative mapping is missing type", index + 1))?;
    Ok((NegativeExpectation { phase, type_name }, cursor))
}

fn assign_negative_field(
    key: &str,
    value: &str,
    line: usize,
    phase: &mut Option<Phase>,
    type_name: &mut Option<String>,
) -> Result<(), ParseError> {
    match key {
        "phase" => {
            if phase.is_some() {
                return Err(ParseError::at_line("duplicate negative.phase field", line));
            }
            let value = parse_scalar(value, "negative.phase").map_err(|mut error| {
                error.line = Some(line);
                error
            })?;
            *phase =
                Some(Phase::parse(&value).map_err(|message| ParseError::at_line(message, line))?);
        }
        "type" => {
            if type_name.is_some() {
                return Err(ParseError::at_line("duplicate negative.type field", line));
            }
            *type_name = Some(parse_scalar(value, "negative.type").map_err(|mut error| {
                error.line = Some(line);
                error
            })?);
        }
        _ => {
            // Future negative metadata (for example an error message pattern)
            // is valid and not needed by discovery.
        }
    }
    Ok(())
}

fn parse_scalar(value: &str, key: &str) -> Result<String, ParseError> {
    let value = strip_inline_comment(value).trim();
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        let last = value.as_bytes()[value.len() - 1] as char;
        if (first == '\'' && last == '\'') || (first == '"' && last == '"') {
            return Ok(unescape_quoted(&value[1..value.len() - 1], first));
        }
        if value.starts_with(['\'', '"']) {
            return Err(ParseError::new(format!(
                "{key} has an unterminated quoted scalar"
            )));
        }
    }
    if value.is_empty() {
        return Err(ParseError::new(format!("{key} must have a value")));
    }
    Ok(value.to_owned())
}

fn unescape_quoted(value: &str, quote: char) -> String {
    let mut output = String::with_capacity(value.len());
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            output.push(match character {
                'n' if quote == '"' => '\n',
                'r' if quote == '"' => '\r',
                't' if quote == '"' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' && quote == '"' {
            escaped = true;
        } else {
            output.push(character);
        }
    }
    if escaped {
        output.push('\\');
    }
    output
}

fn strip_inline_comment(value: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, character) {
            (Some('"'), '\\') => escaped = true,
            (Some(current), character) if current == character => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '#') if index == 0 || value[..index].ends_with(char::is_whitespace) => {
                return &value[..index];
            }
            _ => {}
        }
    }
    value
}

fn skip_indented_block(lines: &[&str], mut index: usize) -> usize {
    while index < lines.len() {
        if !lines[index].trim().is_empty()
            && !lines[index].chars().next().is_some_and(char::is_whitespace)
        {
            break;
        }
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn parse_source(source: &str) -> Metadata {
        parse("fixture.js", source).expect("valid fixture").metadata
    }

    #[test]
    fn parses_scalar_and_block_metadata() {
        let metadata = parse_source(
            "/*---
description: A scalar description
flags:
  - raw
  - module
features: [BigInt, class-fields]
includes: [assert.js]
---*/
assert(true);
",
        );
        assert_eq!(
            metadata.description.as_deref(),
            Some("A scalar description")
        );
        assert_eq!(metadata.flags, ["raw", "module"]);
        assert_eq!(metadata.features, ["BigInt", "class-fields"]);
        assert_eq!(metadata.includes, ["assert.js"]);
    }

    #[test]
    fn parses_multiline_descriptions_and_quoted_items() {
        let metadata = parse_source(
            "/*---
description: |
  first line
  second line
flags: ['onlyStrict', \"async\"]
---*/
",
        );
        assert_eq!(
            metadata.description.as_deref(),
            Some("first line\nsecond line")
        );
        assert_eq!(metadata.flags, ["onlyStrict", "async"]);
    }

    #[test]
    fn folds_multiline_description() {
        let metadata = parse_source(
            "/*---
description: >
  first line
  second line
---*/
",
        );
        assert_eq!(
            metadata.description.as_deref(),
            Some("first line second line")
        );
    }

    #[test]
    fn parses_negative_parse_and_runtime_expectations() {
        let parse_metadata = parse_source(
            "/*---
negative:
  phase: parse
  type: SyntaxError
---*/
",
        );
        assert_eq!(
            parse_metadata.negative,
            Some(NegativeExpectation {
                phase: Phase::Parse,
                type_name: "SyntaxError".into()
            })
        );

        let runtime_metadata = parse_source(
            "/*---
negative: { phase: runtime, type: TypeError }
---*/
",
        );
        assert_eq!(
            runtime_metadata.negative,
            Some(NegativeExpectation {
                phase: Phase::Runtime,
                type_name: "TypeError".into()
            })
        );
    }

    #[test]
    fn preserves_execution_flags_and_ignores_locale() {
        let metadata = parse_source(
            "/*---
flags: [raw, module, async, onlyStrict, noStrict, generated]
locale: en-US
localeCompare:
  - tr
---*/
",
        );
        assert_eq!(
            metadata.flags,
            [
                "raw",
                "module",
                "async",
                "onlyStrict",
                "noStrict",
                "generated"
            ]
        );
    }

    #[test]
    fn rejects_missing_or_malformed_frontmatter() {
        let missing = parse("missing.js", "let x = 1;").unwrap_err();
        assert!(missing.message.contains("missing Test262 frontmatter"));
        let malformed = parse("bad.js", "/*---\nflags: [async, module\n---*/\n").unwrap_err();
        assert!(malformed.message.contains("closing `]`"));
        let missing_negative_type = parse(
            "bad-negative.js",
            "/*---\nnegative:\n  phase: runtime\n---*/\n",
        )
        .unwrap_err();
        assert!(missing_negative_type.message.contains("missing type"));
    }

    #[test]
    fn accepts_the_copyright_header_every_upstream_test_actually_has() {
        // Byte-for-byte the opening of test/language/types/boolean/S8.3_A1_T1.js. Every file in
        // the upstream suite has this shape, and requiring the frontmatter to come first
        // rejected all 53,580 of them, so this case is the whole suite's admission ticket.
        let upstream = concat!(
            "// Copyright 2009 the Sputnik authors.  All rights reserved.\n",
            "// This code is governed by the BSD license found in the LICENSE file.\n",
            "\n",
            "/*---\n",
            "info: The Boolean type have two values, called true and false\n",
            "es5id: 8.3_A1_T1\n",
            "description: Assign true and false to variables\n",
            "---*/\n",
            "\n",
            "var x = true;\n",
        );
        let case = parse("S8.3_A1_T1.js", upstream).expect("upstream header parses");
        assert_eq!(
            case.metadata.description.as_deref(),
            Some("Assign true and false to variables")
        );

        // A block-comment header, and a header carrying both forms, are equally inert.
        assert!(parse("block.js", "/* c */\n/*---\ndescription: d\n---*/\n").is_ok());
        assert!(parse("both.js", "// a\n/* b */\n/*---\ndescription: d\n---*/\n").is_ok());
    }

    #[test]
    fn requires_the_marker_to_begin_a_line_but_allows_invalid_source_before_it() {
        // The rule the old check was protecting: a `/*---` after executable source is not this
        // file's metadata.
        let embedded =
            parse("embedded.js", "var x = 1; /*---\ndescription: d\n---*/\n").unwrap_err();
        assert!(embedded.message.contains("must begin a line"));

        // A hashbang negative test opens with deliberately invalid source and still declares
        // metadata. Twenty of those exist upstream and every one must be readable.
        let hashbang = parse(
            "escaped-bang.js",
            "#\\041\n\n/*---\ndescription: d\n---*/\n",
        );
        assert!(hashbang.is_ok());
    }

    #[test]
    fn discovers_sorted_js_and_excludes_harness_and_fixtures() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("kiln-test262-{stamp}"));
        fs::create_dir_all(root.join("nested")).expect("create root");
        fs::create_dir_all(root.join("harness")).expect("create harness");
        let source = "/*---\ndescription: test\n---*/\n";
        fs::write(root.join("z.js"), source).expect("write z");
        fs::write(root.join("nested").join("a.js"), source).expect("write a");
        fs::write(root.join("nested").join("support_FIXTURE.js"), "not a test")
            .expect("write fixture");
        fs::write(root.join("harness").join("harness.js"), "not a test").expect("write harness");
        fs::write(root.join("ignored.ts"), "not a test").expect("write ts");

        let tests = discover(&root).expect("discover");
        let paths: Vec<_> = tests
            .iter()
            .map(|test| test.path.strip_prefix(&root).expect("relative").to_owned())
            .collect();
        assert_eq!(paths, [PathBuf::from("nested/a.js"), PathBuf::from("z.js")]);
        assert_eq!(tests[0].source, source);
        fs::remove_dir_all(root).expect("remove temporary tree");
    }

    #[test]
    fn reports_file_path_for_discovery_parse_errors() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("kiln-test262-error-{stamp}"));
        fs::create_dir_all(&root).expect("create root");
        let path = root.join("bad.js");
        fs::write(&path, "const x = 1;").expect("write bad");
        let error = discover(&root).unwrap_err();
        match error {
            DiscoveryError::Parse(error) => assert_eq!(error.path, Some(path)),
            other => panic!("expected parse error, got {other:?}"),
        }
        fs::remove_dir_all(root).expect("remove temporary tree");
    }

    #[test]
    fn bundled_fixtures_are_discoverable_and_keep_raw_source() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let tests = discover(&root).expect("bundled fixtures are valid");
        let paths: Vec<_> = tests
            .iter()
            .map(|test| test.path.strip_prefix(&root).expect("relative").to_owned())
            .collect();
        assert_eq!(
            paths,
            [
                PathBuf::from("multiline.js"),
                PathBuf::from("negative-parse.js"),
                PathBuf::from("negative-runtime.js")
            ]
        );
        assert!(tests[0].source.starts_with("/*---"));
        assert_eq!(
            tests[1].metadata.negative.as_ref().unwrap().type_name,
            "SyntaxError"
        );
    }
}
