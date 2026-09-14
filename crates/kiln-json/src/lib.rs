//! One JSON grammar shared by the interpreter and native provider.
//!
//! The scanner consumes JavaScript UTF-16 units. UTF-8 is only an input adapter; parsed strings
//! are stored as units, so escaped and raw lone surrogates never pass through a lossy scalar or
//! WTF-8 representation.

use kiln_string::StringUnits;
use std::cell::RefCell;

mod scalar;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub tag: u32,
    pub len: u32,
    pub value: u64,
}

pub const TAG_NULL: u32 = 0;
pub const TAG_FALSE: u32 = 1;
pub const TAG_TRUE: u32 = 2;
pub const TAG_NUMBER: u32 = 3;
pub const TAG_STRING: u32 = 4;
pub const TAG_ARRAY: u32 = 5;
pub const TAG_OBJECT: u32 = 6;

impl Node {
    pub fn as_number(&self) -> Option<f64> {
        (self.tag == TAG_NUMBER).then(|| f64::from_bits(self.value))
    }
}

/// A source of JavaScript UTF-16 units. Implement this for a borrowed runtime string view to use
/// the same grammar without allocating or converting one unit at a time across an FFI seam.
pub trait UnitSource {
    fn unit_len(&self) -> usize;
    fn unit_at(&self, index: usize) -> Option<u16>;
    fn latin1_slice(&self, _start: usize, _end: usize) -> Option<&[u8]> {
        None
    }
}

impl<T: UnitSource + ?Sized> UnitSource for &T {
    fn unit_len(&self) -> usize {
        (*self).unit_len()
    }
    fn unit_at(&self, index: usize) -> Option<u16> {
        (*self).unit_at(index)
    }
    fn latin1_slice(&self, start: usize, end: usize) -> Option<&[u8]> {
        (*self).latin1_slice(start, end)
    }
}

impl UnitSource for StringUnits {
    fn unit_len(&self) -> usize {
        self.unit_len()
    }
    fn unit_at(&self, index: usize) -> Option<u16> {
        self.unit_at(index)
    }
}
impl UnitSource for [u8] {
    fn unit_len(&self) -> usize {
        self.len()
    }
    fn unit_at(&self, index: usize) -> Option<u16> {
        self.get(index).copied().map(u16::from)
    }
    fn latin1_slice(&self, start: usize, end: usize) -> Option<&[u8]> {
        self.get(start..end)
    }
}
impl UnitSource for [u16] {
    fn unit_len(&self) -> usize {
        self.len()
    }
    fn unit_at(&self, index: usize) -> Option<u16> {
        self.get(index).copied()
    }
}

/// The storage backing one tape's decoded string spans. Latin1 is retained for the common ASCII
/// path and upgraded once, on the first unit above 0xff, with offsets unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextEncoding {
    Latin1,
    Utf16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextArena {
    latin1: Vec<u8>,
    utf16: Vec<u16>,
    encoding: TextEncoding,
}

impl Default for TextArena {
    fn default() -> Self {
        Self {
            latin1: Vec::new(),
            utf16: Vec::new(),
            encoding: TextEncoding::Latin1,
        }
    }
}

impl TextArena {
    pub(crate) fn clear(&mut self) {
        self.latin1.clear();
        self.utf16.clear();
        self.encoding = TextEncoding::Latin1;
    }
    pub(crate) fn len(&self) -> usize {
        match self.encoding {
            TextEncoding::Latin1 => self.latin1.len(),
            TextEncoding::Utf16 => self.utf16.len(),
        }
    }
    pub(crate) fn push_unit(&mut self, unit: u16) {
        if self.encoding == TextEncoding::Latin1 && unit > 0xff {
            self.upgrade();
        }
        match self.encoding {
            TextEncoding::Latin1 => self.latin1.push(unit as u8),
            TextEncoding::Utf16 => self.utf16.push(unit),
        }
    }
    pub(crate) fn extend_latin1(&mut self, bytes: &[u8]) {
        if self.encoding == TextEncoding::Latin1 {
            self.latin1.extend_from_slice(bytes);
        } else {
            self.utf16.extend(bytes.iter().copied().map(u16::from));
        }
    }
    fn upgrade(&mut self) {
        if self.encoding == TextEncoding::Utf16 {
            return;
        }
        self.utf16
            .extend(self.latin1.iter().copied().map(u16::from));
        self.encoding = TextEncoding::Utf16;
    }
    pub(crate) fn slice(&self, start: usize, len: usize) -> Option<TextSlice<'_>> {
        let end = start.checked_add(len)?;
        match self.encoding {
            TextEncoding::Latin1 => Some(TextSlice::Latin1(self.latin1.get(start..end)?)),
            TextEncoding::Utf16 => Some(TextSlice::Utf16(self.utf16.get(start..end)?)),
        }
    }
}

/// A decoded string span, addressed in JavaScript units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSlice<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
}
impl<'a> TextSlice<'a> {
    pub fn len(&self) -> usize {
        match self {
            Self::Latin1(bytes) => bytes.len(),
            Self::Utf16(units) => units.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn unit_at(&self, index: usize) -> Option<u16> {
        match self {
            Self::Latin1(bytes) => bytes.get(index).copied().map(u16::from),
            Self::Utf16(units) => units.get(index).copied(),
        }
    }
    pub fn units(self) -> impl Iterator<Item = u16> + 'a {
        match self {
            Self::Latin1(bytes) => EitherUnits::Latin1(bytes.iter()),
            Self::Utf16(units) => EitherUnits::Utf16(units.iter()),
        }
    }
    pub fn to_units(self) -> Box<[u16]> {
        self.units().collect()
    }
}
enum EitherUnits<'a> {
    Latin1(std::slice::Iter<'a, u8>),
    Utf16(std::slice::Iter<'a, u16>),
}
impl Iterator for EitherUnits<'_> {
    type Item = u16;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Latin1(iter) => iter.next().copied().map(u16::from),
            Self::Utf16(iter) => iter.next().copied(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Tape<'a> {
    nodes: &'a [Node],
    text: &'a TextArena,
}
impl<'a> Tape<'a> {
    pub fn nodes(&self) -> &'a [Node] {
        self.nodes
    }
    pub fn text(&self) -> TextSlice<'a> {
        self.text
            .slice(0, self.text.len())
            .unwrap_or(TextSlice::Latin1(&[]))
    }
    pub fn string(&self, node: &Node) -> Option<TextSlice<'a>> {
        if node.tag != TAG_STRING {
            return None;
        }
        self.text
            .slice(usize::try_from(node.value).ok()?, node.len as usize)
    }
    pub fn string_units(&self, node: &Node) -> Option<StringUnits> {
        self.string(node)
            .map(|slice| StringUnits::from_units(&slice.to_units()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    nodes: Vec<Node>,
    text: TextArena,
}
impl Document {
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn text(&self) -> TextSlice<'_> {
        self.text
            .slice(0, self.text.len())
            .unwrap_or(TextSlice::Latin1(&[]))
    }
    pub fn string(&self, node: &Node) -> Option<TextSlice<'_>> {
        if node.tag != TAG_STRING {
            return None;
        }
        self.text
            .slice(usize::try_from(node.value).ok()?, node.len as usize)
    }
    pub fn string_units(&self, node: &Node) -> Option<StringUnits> {
        self.string(node)
            .map(|slice| StringUnits::from_units(&slice.to_units()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Syntax { offset: usize },
    TooLarge,
}

#[derive(Default)]
struct Scratch {
    text: TextArena,
    nodes: Vec<Node>,
    stack: Vec<scalar::Frame>,
    token: Vec<u8>,
}
thread_local! { static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default()); }

pub fn parse_source<S: UnitSource, R>(
    source: &S,
    walk: impl FnOnce(&Tape<'_>) -> R,
) -> Result<R, ParseError> {
    if source.unit_len() > u32::MAX as usize {
        return Err(ParseError::TooLarge);
    }
    SCRATCH.with(|scratch| {
        #[expect(
            clippy::panic,
            reason = "a parse re-entering its tape would overwrite live data"
        )]
        let mut scratch = scratch
            .try_borrow_mut()
            .unwrap_or_else(|_| panic!("kiln-json: parse re-entered"));
        let Scratch {
            text,
            nodes,
            stack,
            token,
        } = &mut *scratch;
        scalar::scan(source, nodes, text, stack, token)?;
        Ok(walk(&Tape { nodes, text }))
    })
}

pub fn parse_units<R>(
    source: &StringUnits,
    walk: impl FnOnce(&Tape<'_>) -> R,
) -> Result<R, ParseError> {
    parse_source(source, walk)
}
pub fn parse_utf8<R>(source: &[u8], walk: impl FnOnce(&Tape<'_>) -> R) -> Result<R, ParseError> {
    if source.is_ascii() {
        return parse_source(&source, walk);
    }
    let units = StringUnits::from_utf8(source).map_err(|_| ParseError::Syntax { offset: 0 })?;
    parse_units(&units, walk)
}
pub fn parse<R>(source: &[u8], walk: impl FnOnce(&Tape<'_>) -> R) -> Result<R, ParseError> {
    parse_utf8(source, walk)
}

pub fn parse_owned(source: &[u8]) -> Result<Document, ParseError> {
    parse_utf8(source, |tape| Document {
        nodes: tape.nodes.to_vec(),
        text: tape.text.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tape(source: &str) -> Document {
        parse_owned(source.as_bytes()).unwrap_or_default()
    }
    fn units(source: &str) -> Vec<Vec<u16>> {
        let doc = tape(source);
        doc.nodes()
            .iter()
            .filter_map(|node| doc.string_units(node).map(|s| s.to_units().into_vec()))
            .collect()
    }
    fn numbers(source: &str) -> Vec<f64> {
        let doc = tape(source);
        doc.nodes().iter().filter_map(Node::as_number).collect()
    }
    fn refused(source: &str) -> bool {
        matches!(
            parse_owned(source.as_bytes()),
            Err(ParseError::Syntax { .. })
        )
    }

    #[test]
    fn keys_and_values_keep_units() {
        assert_eq!(
            units(r#"{"\ud800":"\ude00","x":"\ud83d\ude00"}"#),
            vec![
                vec![0xd800],
                vec![0xde00],
                vec![b'x' as u16],
                vec![0xd83d, 0xde00]
            ]
        );
    }
    #[test]
    fn ascii_tape_stays_latin1() {
        assert!(matches!(
            tape(r#"{"a":"b\u0000"}"#).text(),
            TextSlice::Latin1(_)
        ));
    }
    #[test]
    fn numbers_keep_ecmascript_rules() {
        assert!(numbers("[-0,0,1e400,-1e400]")[0].is_sign_negative());
        assert_eq!(numbers("[1e400,-1e400]")[0], f64::INFINITY);
    }
    #[test]
    fn grammar_refusals() {
        for bad in [
            "", "{", "[", "{\"a\"}", "[1,]", "01", "1.", ".5", "1e", "truex", "1 2",
        ] {
            assert!(refused(bad), "{bad:?}");
        }
    }
    #[test]
    fn deep_stack_is_iterative() {
        let deep = "[".repeat(4000) + &"]".repeat(4000);
        assert!(parse_owned(deep.as_bytes()).is_ok());
    }

    #[test]
    fn raw_utf16_and_escaped_surrogates_are_the_same_units() {
        let source = StringUnits::from_units(&[
            0x7b, 0x22, 0xd800, 0x22, 0x3a, 0x22, 0xd800, 0x22, 0x2c, 0x22, 0x70, 0x22, 0x3a, 0x22,
            0x5c, 0x75, 0x64, 0x38, 0x30, 0x30, 0x22, 0x7d,
        ]);
        let parsed = parse_units(&source, |tape| {
            tape.nodes()
                .iter()
                .filter_map(|node| {
                    tape.string_units(node)
                        .map(|value| value.to_units().into_vec())
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            parsed.unwrap_or_default(),
            vec![vec![0xd800], vec![0xd800], vec![0x70], vec![0xd800]]
        );
    }

    #[test]
    fn escaped_pairs_and_nul_keep_offsets_in_mixed_keys() {
        let document = tape(r#"{"a\u0000":"\ud83d\ude00","\udc00":"x"}"#);
        let values = document
            .nodes()
            .iter()
            .filter_map(|node| {
                document
                    .string_units(node)
                    .map(|value| value.to_units().into_vec())
            })
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                vec![b'a' as u16, 0],
                vec![0xd83d, 0xde00],
                vec![0xdc00],
                vec![b'x' as u16]
            ]
        );
    }

    #[test]
    fn incomplete_deep_document_is_refused() {
        assert!(refused(&"[".repeat(300)));
    }

    fn string_units(source: &str) -> Vec<Vec<u16>> {
        units(source)
    }
    fn scalar_units(text: &str) -> Vec<u16> {
        StringUnits::from(text).to_units().into_vec()
    }

    #[test]
    fn keys_keep_source_order_and_duplicates_stay_in_place() {
        let document = tape(r#"{"b":1,"a":2,"b":3,"__proto__":4}"#);
        assert_eq!(
            (document.nodes()[0].tag, document.nodes()[0].len),
            (TAG_OBJECT, 4)
        );
        let keys: Vec<_> = document
            .nodes()
            .iter()
            .skip(1)
            .step_by(2)
            .filter_map(|node| {
                document
                    .string_units(node)
                    .map(|value| value.to_units().into_vec())
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                scalar_units("b"),
                scalar_units("a"),
                scalar_units("b"),
                scalar_units("__proto__")
            ]
        );
    }

    #[test]
    fn containers_carry_child_counts() {
        let document = tape(r#"[[],{},[1,[2,3]],{"k":[]}]"#);
        assert_eq!(
            (document.nodes()[0].tag, document.nodes()[0].len),
            (TAG_ARRAY, 4)
        );
        assert_eq!(
            (document.nodes()[1].tag, document.nodes()[1].len),
            (TAG_ARRAY, 0)
        );
        assert_eq!(
            (document.nodes()[2].tag, document.nodes()[2].len),
            (TAG_OBJECT, 0)
        );
        assert_eq!(
            (document.nodes()[3].tag, document.nodes()[3].len),
            (TAG_ARRAY, 2)
        );
    }

    #[test]
    fn negative_zero_is_kept_and_only_for_that_spelling() {
        let values = numbers("[-0, 0, 0.0, -0.0, 0e1, -0e1, -1, 1]");
        assert!(values[0] == 0.0 && values[0].is_sign_negative());
        assert!(values[1] == 0.0 && values[1].is_sign_positive());
        assert!(values[2].is_sign_positive());
        assert!(values[3].is_sign_negative());
        assert!(values[4].is_sign_positive());
        assert!(values[5].is_sign_negative());
        assert_eq!(&values[6..], &[-1.0, 1.0]);
    }

    #[test]
    fn integers_past_u64_round_as_doubles() {
        assert_eq!(
            numbers("18446744073709551616"),
            vec![18446744073709551616.0]
        );
        assert_eq!(
            numbers("-9223372036854775809"),
            vec![-9223372036854775809.0]
        );
        assert_eq!(numbers("9007199254740993"), vec![9007199254740992.0]);
        assert_eq!(
            numbers("123456789012345678901234567890"),
            vec![1.2345678901234568e29]
        );
    }

    #[test]
    fn overflow_is_infinity_not_a_refusal() {
        assert_eq!(numbers("1e400"), vec![f64::INFINITY]);
        assert_eq!(numbers("-1e400"), vec![f64::NEG_INFINITY]);
        assert_eq!(
            numbers(r#"[1e400, "1e400", 2, -2e999, {"1e400": 1e-400}]"#),
            vec![f64::INFINITY, 2.0, f64::NEG_INFINITY, 0.0]
        );
        assert_eq!(
            string_units(r#"[1e400, "1e400", {"1e400": 1}]"#),
            vec![scalar_units("1e400"), scalar_units("1e400")]
        );
        assert!(refused("01e400"));
        assert!(refused("1e400x"));
        assert!(refused("[1e400,]"));
    }

    #[test]
    fn surrogate_pairs_and_lone_halves_are_preserved() {
        assert_eq!(units(r#""\ud83d\ude00""#), vec![vec![0xd83d, 0xde00]]);
        assert_eq!(
            units(r#""a\ud83d\ude00b""#),
            vec![vec![b'a' as u16, 0xd83d, 0xde00, b'b' as u16]]
        );
        assert_eq!(units(r#""\ud83d""#), vec![vec![0xd83d]]);
        assert_eq!(units(r#""\ude00""#), vec![vec![0xde00]]);
        assert_eq!(units(r#""\ud83dx""#), vec![vec![0xd83d, b'x' as u16]]);
        assert_eq!(units(r#""\ud83d\ud83d""#), vec![vec![0xd83d, 0xd83d]]);
        assert_eq!(units(r#""\ud83dA""#), vec![vec![0xd83d, b'A' as u16]]);
        assert_eq!(
            units(r#""\ud83d\ude00\uDE00""#),
            vec![vec![0xd83d, 0xde00, 0xde00]]
        );
        assert_eq!(
            units(r#"{"\ud83d\ude00":"\u00e9"}"#),
            vec![vec![0xd83d, 0xde00], vec![0xe9]]
        );
        assert_eq!(units(r#""\\ud800""#), vec![scalar_units("\\ud800")]);
        assert_eq!(units(r#""\\\ud800""#), vec![vec![b'\\' as u16, 0xd800]]);
        assert_eq!(units(r#""\u0000A""#), vec![vec![0, b'A' as u16]]);
    }

    #[test]
    fn escapes_and_raw_text() {
        assert_eq!(
            units(r#""\"\\\/\b\f\n\r\t""#),
            vec![vec![0x22, 0x5c, 0x2f, 8, 12, 10, 13, 9]]
        );
        assert_eq!(
            units("\"h\u{e9}llo w\u{f6}rld \u{1F600}\""),
            vec![scalar_units("héllo wörld 😀")]
        );
        assert!(refused(r#""\x""#));
        assert!(refused(r#""\u12""#));
        assert!(refused(r#""\ug000""#));
        assert!(refused("\"a\u{1}b\""));
        assert!(refused("\"a\nb\""));
        assert!(refused(r#""unterminated"#));
    }

    #[test]
    fn grammar_refusals_match_the_language() {
        for bad in [
            "",
            " ",
            "{",
            "[",
            "]",
            "}",
            "{\"a\"}",
            "{\"a\":}",
            "{a:1}",
            "{'a':1}",
            "[1,]",
            "[,1]",
            "{\"a\":1,}",
            "01",
            "1.",
            ".5",
            "+1",
            "1e",
            "1e+",
            "-",
            "--1",
            "0x10",
            "NaN",
            "Infinity",
            "-Infinity",
            "undefined",
            "nul",
            "tru",
            "truex",
            "[truex]",
            "1 2",
            "{} {}",
            "\"a\" \"b\"",
            "[1] x",
            "\u{feff}1",
        ] {
            assert!(refused(bad), "{bad:?} should be refused");
        }
        for good in [
            "1", " 1 ", "\n[]\r\n", "true", "false", "null", "\"\"", "{}", "[[[[]]]]", "-0.5e-3",
        ] {
            assert!(
                parse_owned(good.as_bytes()).is_ok(),
                "{good:?} should parse"
            );
        }
    }

    #[test]
    fn numbers_round_correctly() {
        assert_eq!(
            numbers("[0.1, 1e21, 1.7976931348623157e308, 5e-324, 2.5e-324, 4.9e-324]"),
            vec![0.1, 1e21, f64::MAX, 5e-324, 5e-324, 5e-324]
        );
        assert_eq!(
            numbers("[1E2, 1e+2, 1e-2, 123456789.123456789]"),
            vec![100.0, 100.0, 0.01, 123_456_789.123_456_79]
        );
        assert_eq!(numbers("1e-400"), vec![0.0]);
    }

    #[test]
    fn nesting_is_bounded_by_length_not_a_constant() {
        let deep = "[".repeat(20_000) + &"]".repeat(20_000);
        let parsed = parse_owned(deep.as_bytes());
        assert!(parsed.is_ok(), "the deep document parses: {parsed:?}");
        let document = parsed.unwrap_or_default();
        assert_eq!(document.nodes().len(), 20_000);
        assert!(document.nodes().iter().all(|node| node.tag == TAG_ARRAY));
        let objects = "{\"a\":".repeat(5_000) + "1" + &"}".repeat(5_000);
        assert!(parse_owned(objects.as_bytes()).is_ok());
        assert!(refused(&"[".repeat(300)));
    }

    #[test]
    fn scalar_roots_and_whitespace() {
        assert_eq!(numbers(" \t\r\n42 \t\r\n"), vec![42.0]);
        assert_eq!(units("\"x\""), vec![vec![b'x' as u16]]);
        assert_eq!(tape("null").nodes()[0].tag, TAG_NULL);
        assert_eq!(tape("true").nodes()[0].tag, TAG_TRUE);
        assert_eq!(tape("false").nodes()[0].tag, TAG_FALSE);
    }

    #[test]
    fn a_document_is_reusable_after_errors_and_across_parses() {
        assert!(refused("{"));
        assert_eq!(numbers("[1]"), vec![1.0]);
        assert!(refused("1e400x"));
        assert_eq!(numbers("[1e400]"), vec![f64::INFINITY]);
        let first = parse_owned(b"[\"a\"]").unwrap_or_default();
        let second = parse_owned(b"[\"b\"]").unwrap_or_default();
        assert_eq!(
            first
                .string_units(&first.nodes()[1])
                .map(|s| s.to_units().into_vec()),
            Some(vec![b'a' as u16])
        );
        assert_eq!(
            second
                .string_units(&second.nodes()[1])
                .map(|s| s.to_units().into_vec()),
            Some(vec![b'b' as u16])
        );
    }
}
