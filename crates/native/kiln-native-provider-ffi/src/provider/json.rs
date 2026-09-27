use super::INVALID_ARGUMENT;
use super::{KilnString, StringView};
use kiln_json::{Node, ParseError, TextSlice, UnitSource};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The text is not JSON. The caller raises the SyntaxError the language specifies. Shares the
/// invalid-argument code deliberately: the two are the same answer to the emitted C, which
/// never passes a bad argument, and a refusal here must never read as a fault that stops the
/// program.
const SYNTAX_ERROR: i32 = -1;
/// The reader could not run at all: the text is longer than the tape indexes, or it panicked.
const READER_FAILED: i32 = -2;

fn parse_tape<S: UnitSource>(source: S) -> Result<(*const Node, usize, KilnString), ParseError> {
    kiln_json::parse_source(&source, |tape| {
        let nodes = tape.nodes();
        let view = match tape.text() {
            TextSlice::Latin1(bytes) => KilnString {
                unit_len: bytes.len() as u32,
                flags: 0,
                data: bytes.as_ptr().cast(),
            },
            TextSlice::Utf16(units) => KilnString {
                unit_len: units.len() as u32,
                flags: 1,
                data: units.as_ptr().cast(),
            },
        };
        (nodes.as_ptr(), nodes.len(), view)
    })
}

/// Parse the canonical Latin1 or UTF-16 [`KilnString`] source view.
///
/// Returns 0 with `out_count` tape nodes at `out_nodes`, in source order, and the decoded text
/// view in `out_text`; string node offsets and lengths count JavaScript UTF-16 units. Returns
/// -1 for text that is not JSON or an invalid string header, or -2 when the reader could not
/// run. The three output slots point into this thread's scratch and remain valid until its
/// next parse; the caller reads them and does not free them.
///
/// # Safety
///
/// `source` must be null only for the invalid-argument case; otherwise its header must name
/// readable Latin1 bytes or aligned UTF-16 units for `unit_len`, with flags 0 or 1. The three
/// output pointers must be writable slots. The returned pointers are borrowed from the
/// provider's thread-local scratch and must not be retained after the next parse on that
/// thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kiln_json_parse(
    source: *const KilnString,
    out_nodes: *mut *const Node,
    out_count: *mut usize,
    out_text: *mut KilnString,
) -> i32 {
    if out_nodes.is_null() || out_count.is_null() || out_text.is_null() {
        return INVALID_ARGUMENT;
    }
    let Some(source) = (unsafe { super::validate_string(source) }) else {
        return INVALID_ARGUMENT;
    };
    // A panic must not cross the C boundary: the archive's unwind policy makes one fatal to
    // the process, and this is `JSON.parse`, which a program wraps in `try`.
    let parsed = catch_unwind(AssertUnwindSafe(|| match source {
        StringView::Latin1(bytes) => parse_tape(bytes),
        StringView::Utf16(units) => parse_tape(units),
    }));
    match parsed {
        Ok(Ok((nodes, count, text))) => {
            // SAFETY: the three output slots are writable by the caller contract. The pointers are
            // into the crate's thread-local scratch, which outlives this call.
            unsafe {
                out_nodes.write(nodes);
                out_count.write(count);
                out_text.write(text);
            }
            0
        }
        Ok(Err(ParseError::Syntax { .. })) => SYNTAX_ERROR,
        Ok(Err(ParseError::TooLarge)) | Err(_) => READER_FAILED,
    }
}

macro_rules! assert_provider_export {
        ($(($group:ident, $name:ident, $signature:ident);)*) => {
            $(assert_provider_export!($group, $name, $signature);)*
        };
        (json, kiln_json_parse, json_parse) => {
            const _: unsafe extern "C" fn(*const KilnString, *mut *const Node, *mut usize, *mut KilnString) -> i32 = kiln_json_parse;
        };
        ($group:ident, $name:ident, $signature:ident) => {};
    }

kiln_native_provider_contract::provider_abi_registry!(assert_provider_export);

#[cfg(test)]
mod tests {
    use super::*;
    use kiln_json::{TAG_ARRAY, TAG_NUMBER, TAG_OBJECT, TAG_STRING};

    struct Parsed {
        status: i32,
        tags: Vec<u32>,
        key: Vec<u8>,
    }

    fn parse(text: &str) -> Parsed {
        let mut nodes: *const Node = core::ptr::null();
        let mut count = 0usize;
        let source = KilnString {
            unit_len: text.len() as u32,
            flags: 0,
            data: text.as_ptr().cast(),
        };
        let mut arena = KilnString {
            unit_len: 0,
            flags: 0,
            data: core::ptr::null(),
        };
        let status = unsafe { kiln_json_parse(&source, &mut nodes, &mut count, &mut arena) };
        if status != 0 {
            return Parsed {
                status,
                tags: Vec::new(),
                key: Vec::new(),
            };
        }
        let tape = unsafe { core::slice::from_raw_parts(nodes, count) };
        let key = tape
            .get(1)
            .filter(|node| node.tag == TAG_STRING)
            .map(|node| {
                let index = node.value as usize;
                let ptr = arena.data.cast::<u16>();
                if arena.flags == 0 {
                    unsafe {
                        core::slice::from_raw_parts(
                            arena.data.cast::<u8>().add(index),
                            node.len as usize,
                        )
                        .iter()
                        .map(|&unit| u16::from(unit))
                        .collect::<Vec<_>>()
                    }
                } else {
                    unsafe {
                        core::slice::from_raw_parts(ptr.add(index), node.len as usize).to_vec()
                    }
                }
            })
            .unwrap_or_default()
            .into_iter()
            .map(|unit| unit as u8)
            .collect();
        Parsed {
            status,
            tags: tape.iter().map(|node| node.tag).collect(),
            key,
        }
    }

    #[test]
    fn a_document_crosses_as_a_tape_into_this_threads_scratch() {
        let parsed = parse(r#"{"a":[1,"x"]}"#);
        assert_eq!(parsed.status, 0);
        assert_eq!(
            parsed.tags,
            vec![TAG_OBJECT, TAG_STRING, TAG_ARRAY, TAG_NUMBER, TAG_STRING]
        );
        assert_eq!(parsed.key, b"a");
        // The next parse overwrites the scratch in place; nothing was leaked or freed.
        let again = parse("[2,3]");
        assert_eq!(again.tags, vec![TAG_ARRAY, TAG_NUMBER, TAG_NUMBER]);
    }

    #[test]
    fn non_ascii_text_answers_units_with_unit_offsets() {
        // "é" is two UTF-8 bytes for one unit and "😀" four bytes for two units. A
        // UTF-16 source exercises the wide path; offsets that survive are unit
        // offsets, so byte offsets would land mid-character here.
        let want: Vec<u16> = "é😀".encode_utf16().collect();
        let mut source: Vec<u16> = r#"{"k":""#.encode_utf16().collect();
        source.extend_from_slice(&want);
        source.extend(r#""}"#.encode_utf16());
        let head = KilnString {
            unit_len: source.len() as u32,
            flags: 1,
            data: source.as_ptr().cast(),
        };
        let mut nodes: *const Node = core::ptr::null();
        let mut count = 0usize;
        let mut arena = KilnString {
            unit_len: 0,
            flags: 0,
            data: core::ptr::null(),
        };
        let status = unsafe { kiln_json_parse(&head, &mut nodes, &mut count, &mut arena) };
        assert_eq!(status, 0);
        // SAFETY: a zero status answers a readable tape and headed text.
        let tape = unsafe { core::slice::from_raw_parts(nodes, count) };
        assert_eq!(
            tape.iter().map(|node| node.tag).collect::<Vec<_>>(),
            vec![TAG_OBJECT, TAG_STRING, TAG_STRING]
        );
        let node = tape
            .get(2)
            .filter(|node| node.tag == TAG_STRING)
            .expect("wide value node");
        let start = node.value as usize;
        let got = if arena.flags == 0 {
            // SAFETY: the arena borrows the reader's scratch, live for this check.
            unsafe {
                core::slice::from_raw_parts(arena.data.cast::<u8>().add(start), node.len as usize)
                    .iter()
                    .map(|unit| u16::from(*unit))
                    .collect::<Vec<_>>()
            }
        } else {
            // SAFETY: same borrow; the header promises aligned units.
            unsafe {
                core::slice::from_raw_parts(arena.data.cast::<u16>().add(start), node.len as usize)
                    .to_vec()
            }
        };
        assert_eq!(got, want);
    }

    #[test]
    fn refusals_and_bad_arguments_are_told_apart() {
        assert_eq!(parse("{").status, SYNTAX_ERROR);
        assert_eq!(parse("").status, SYNTAX_ERROR);
        let mut nodes: *const Node = core::ptr::null();
        let status = unsafe {
            kiln_json_parse(
                core::ptr::null(),
                &mut nodes,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };
        assert_eq!(status, INVALID_ARGUMENT);
    }
}
