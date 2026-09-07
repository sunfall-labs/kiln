//! `JSON.parse`'s grammar, once, as a flat tape both engines walk.
//!
//! The interpreter and the emitted C each used to carry a recursive-descent reader of their own,
//! held in agreement by the differential corpus: two implementations of one exactly-specified
//! thing, which agree until one of them has a bug and then nothing can tell which (ADR 0025 gives
//! the same reason for regular expressions and `BigInt`). Now there is one reader, here, and each
//! engine keeps only the part that has to touch its own heap: allocate a string, an array or an
//! object, and insert a property.
//!
//! The boundary is a **tape**: a document becomes a flat sequence of [`Node`]s in source order --
//! a container node carries its child count, a string node points into the decoded text, a number
//! node carries the double -- so a consumer walks it once with an explicit stack and never needs to
//! know the grammar. Across the native provider ABI nothing but scalars and byte ranges cross,
//! exactly as ADR 0025 requires.
//!
//! The reader is the one-pass scalar scanner in `scalar.rs`. It began as simd-json's tape with a
//! conversion pass, and simd-json was measured against the scalar pass on documents of the
//! benchmark's shape before being dropped (2026-09-06, Apple Silicon, release, us per parse):
//!
//! | bytes | scalar | simd-json | simd/scalar |
//! | --- | --- | --- | --- |
//! | 159 | 0.127 | 0.153 | 1.21 |
//! | 1,894 | 1.31 | 1.32 | 1.01 |
//! | 31,694 | 21.3 | 24.1 | 1.13 |
//! | 519,892 | 344 | 391 | 1.14 |
//! | 2,094,981 | 1,369 | 1,581 | 1.16 |
//! | 8,494,746 | 5,553 | 6,435 | 1.16 |
//!
//! The scalar pass won at every size, by a fifth on small documents and a sixth on large ones:
//! the tape has to be written either way, and simd-json's structural index was a second pass over
//! the input before that, plus a padded copy and a conversion of its tape into this one. The
//! rules simd-json needed help with -- a lone surrogate escape as U+FFFD (ADR 0022), `-0`, a
//! number past a double's range as infinity, integers past `u64` -- are the scanner's own now,
//! and `tests/agreement.rs` pins the number rule to `f64::from_str` across a generated sweep.
//!
//! Depth is bounded by the input's length, not by a constant: the scanner's stack is explicit and
//! so is every walker's.

use std::cell::RefCell;

mod scalar;

/// One entry of the tape. `#[repr(C)]` because the emitted C reads it as `kiln_json_node`.
///
/// * `tag` is one of the `TAG_*` constants.
/// * `len` is a string's byte length, or a container's number of children (elements, or key/value
///   pairs).
/// * `value` is a string's byte offset into the text, or a number's `f64` bits.
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
    /// The double of a number node.
    pub fn as_number(&self) -> Option<f64> {
        (self.tag == TAG_NUMBER).then(|| f64::from_bits(self.value))
    }
}

/// A parsed document: the tape in source order and the decoded text every string node indexes.
///
/// Borrowed from this thread's scratch for the duration of [`parse`]'s walk, so a parse allocates
/// nothing once the scratch has grown to the size of the documents it sees.
#[derive(Clone, Copy, Debug)]
pub struct Tape<'a> {
    nodes: &'a [Node],
    text: &'a [u8],
}

impl<'a> Tape<'a> {
    /// The tape, in source order.
    pub fn nodes(&self) -> &'a [Node] {
        self.nodes
    }

    /// The decoded text every string node's offset and length index.
    pub fn text(&self) -> &'a [u8] {
        self.text
    }

    /// The decoded bytes of a string node.
    ///
    /// Valid UTF-8 by construction -- the input is a JavaScript string's UTF-8 and every escape
    /// decodes to a scalar value -- but read back through `from_utf8` rather than an unchecked
    /// conversion, because the offsets crossed an ABI and a lie there must not become undefined
    /// behaviour here.
    pub fn string(&self, node: &Node) -> Option<&'a str> {
        if node.tag != TAG_STRING {
            return None;
        }
        let start = usize::try_from(node.value).ok()?;
        let end = start.checked_add(node.len as usize)?;
        std::str::from_utf8(self.text.get(start..end)?).ok()
    }
}

/// An owned copy of a [`Tape`], for a caller that wants to keep one past the walk. Tests, mostly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    nodes: Vec<Node>,
    text: Vec<u8>,
}

impl Document {
    /// The tape, in source order.
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// The decoded text every string node's offset and length index.
    pub fn text(&self) -> &[u8] {
        &self.text
    }

    /// The decoded bytes of a string node; see [`Tape::string`].
    pub fn string(&self, node: &Node) -> Option<&str> {
        self.tape().string(node)
    }

    fn tape(&self) -> Tape<'_> {
        Tape {
            nodes: &self.nodes,
            text: &self.text,
        }
    }
}

/// Why a document did not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The text is not JSON. `offset` is where the scanner stopped, for a diagnostic; the language
    /// reports a `SyntaxError` regardless.
    Syntax { offset: usize },
    /// Longer than the tape can index.
    TooLarge,
}

/// Everything a parse needs, kept per thread and grown once: the copy of the input the scanner
/// decodes into, the node array, and the scanner's frame stack. A parse writes into these in
/// place and lends them to the walk; nothing is allocated per call after the first document of a
/// given size.
#[derive(Default)]
struct Scratch {
    text: Vec<u8>,
    nodes: Vec<Node>,
    stack: Vec<scalar::Frame>,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

/// Parse `source` -- the bytes of a JavaScript string, so valid UTF-8 -- and hand the tape to
/// `walk`.
///
/// One parse is in flight per thread: the tape borrows the thread's scratch, and a walk that
/// parsed again would be overwriting the tape it is reading. That is asserted, not assumed -- a
/// re-entrant parse panics -- and neither engine's walk parses, so the panic names a bug rather
/// than a case. The scratch outlives the walk, which is what lets a C caller keep reading the
/// tape after the call returns, until the next parse on its thread.
pub fn parse<R>(source: &[u8], walk: impl FnOnce(&Tape<'_>) -> R) -> Result<R, ParseError> {
    if u32::try_from(source.len()).is_err() {
        return Err(ParseError::TooLarge);
    }
    SCRATCH.with(|scratch| {
        // The one-parse-in-flight invariant is asserted, not assumed: this is the crate's own
        // scratch, and a re-entrant parse would overwrite the tape its caller is still reading.
        #[expect(clippy::panic, reason = "an asserted invariant, not an error path")]
        let mut scratch = scratch
            .try_borrow_mut()
            .unwrap_or_else(|_| panic!("kiln-json: a parse re-entered from inside a parse's walk"));
        let Scratch { text, nodes, stack } = &mut *scratch;
        scalar::scan(source, nodes, text, stack)?;
        Ok(walk(&Tape { nodes, text }))
    })
}

/// [`parse`], keeping the result: a copy of the tape and text.
pub fn parse_owned(source: &[u8]) -> Result<Document, ParseError> {
    parse(source, |tape| Document {
        nodes: tape.nodes().to_vec(),
        text: tape.text().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tape(source: &str) -> Vec<(u32, u32, Option<String>, Option<f64>)> {
        let parsed = parse_owned(source.as_bytes());
        assert!(parsed.is_ok(), "{source} parses: {parsed:?}");
        let document = parsed.unwrap_or_default();
        document
            .nodes()
            .iter()
            .map(|node| {
                (
                    node.tag,
                    node.len,
                    document.string(node).map(str::to_owned),
                    node.as_number(),
                )
            })
            .collect()
    }

    fn strings(source: &str) -> Vec<String> {
        tape(source).into_iter().filter_map(|row| row.2).collect()
    }

    fn numbers(source: &str) -> Vec<f64> {
        tape(source).into_iter().filter_map(|row| row.3).collect()
    }

    fn refused(source: &str) -> bool {
        matches!(
            parse_owned(source.as_bytes()),
            Err(ParseError::Syntax { .. })
        )
    }

    #[test]
    fn keys_keep_source_order_and_duplicates_stay_in_place() {
        let rows = tape(r#"{"b":1,"a":2,"b":3,"__proto__":4}"#);
        assert_eq!(rows[0].0, TAG_OBJECT);
        assert_eq!(rows[0].1, 4);
        let keys: Vec<_> = rows
            .iter()
            .skip(1)
            .step_by(2)
            .map(|row| row.2.clone())
            .collect();
        assert_eq!(
            keys,
            vec![
                Some("b".into()),
                Some("a".into()),
                Some("b".into()),
                Some("__proto__".into())
            ]
        );
    }

    #[test]
    fn containers_carry_child_counts() {
        let rows = tape(r#"[[],{},[1,[2,3]],{"k":[]}]"#);
        assert_eq!((rows[0].0, rows[0].1), (TAG_ARRAY, 4));
        assert_eq!((rows[1].0, rows[1].1), (TAG_ARRAY, 0));
        assert_eq!((rows[2].0, rows[2].1), (TAG_OBJECT, 0));
        assert_eq!((rows[3].0, rows[3].1), (TAG_ARRAY, 2));
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
            strings(r#"[1e400, "1e400", {"1e400": 1}]"#),
            vec!["1e400", "1e400"]
        );
        // A spelling JSON forbids stays refused even when a double would overflow on it.
        assert!(refused("01e400"));
        assert!(refused("1e400x"));
        assert!(refused("[1e400,]"));
    }

    #[test]
    fn surrogate_pairs_combine_and_lone_halves_are_replaced() {
        assert_eq!(strings(r#""\ud83d\ude00""#), vec!["\u{1F600}"]);
        assert_eq!(strings(r#""a\ud83d\ude00b""#), vec!["a\u{1F600}b"]);
        assert_eq!(strings(r#""\ud83d""#), vec!["\u{FFFD}"]);
        assert_eq!(strings(r#""\ude00""#), vec!["\u{FFFD}"]);
        assert_eq!(strings(r#""\ud83dx""#), vec!["\u{FFFD}x"]);
        assert_eq!(strings(r#""\ud83d\ud83d""#), vec!["\u{FFFD}\u{FFFD}"]);
        assert_eq!(strings(r#""\ud83dA""#), vec!["\u{FFFD}A"]);
        assert_eq!(
            strings(r#""\ud83d\ude00\uDE00""#),
            vec!["\u{1F600}\u{FFFD}"]
        );
        assert_eq!(
            strings(r#"{"\ud83d\ude00":"\u00e9"}"#),
            vec!["\u{1F600}", "\u{e9}"]
        );
        // An escaped backslash followed by the letters of a surrogate is not an escape.
        assert_eq!(strings(r#""\\ud800""#), vec!["\\ud800"]);
        assert_eq!(strings(r#""\\\ud800""#), vec!["\\\u{FFFD}"]);
        assert_eq!(strings(r#""\u0000A""#), vec!["\0A"]);
    }

    #[test]
    fn escapes_and_raw_text() {
        assert_eq!(
            strings(r#""\"\\\/\b\f\n\r\t""#),
            vec!["\"\\/\u{8}\u{c}\n\r\t"]
        );
        assert_eq!(
            strings("\"h\u{e9}llo w\u{f6}rld \u{1F600}\""),
            vec!["h\u{e9}llo w\u{f6}rld \u{1F600}"]
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
        assert_eq!(strings("\"x\""), vec!["x"]);
        assert_eq!(tape("null")[0].0, TAG_NULL);
        assert_eq!(tape("true")[0].0, TAG_TRUE);
        assert_eq!(tape("false")[0].0, TAG_FALSE);
    }

    #[test]
    fn a_document_is_reusable_after_errors_and_across_parses() {
        assert!(refused("{"));
        assert_eq!(numbers("[1]"), vec![1.0]);
        assert!(refused("1e400x"));
        assert_eq!(numbers("[1e400]"), vec![f64::INFINITY]);
        let parsed = parse_owned(b"[\"a\"]");
        assert!(parsed.is_ok(), "{parsed:?}");
        let first = parsed.unwrap_or_default();
        let parsed = parse_owned(b"[\"b\"]");
        assert!(parsed.is_ok(), "{parsed:?}");
        let second = parsed.unwrap_or_default();
        assert_eq!(first.string(&first.nodes()[1]), Some("a"));
        assert_eq!(second.string(&second.nodes()[1]), Some("b"));
    }
}
