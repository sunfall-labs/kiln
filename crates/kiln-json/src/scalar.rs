//! The reader: one pass over a document, writing the tape and the decoded text directly.
//!
//! A byte-at-a-time walk over a copy of the input, with no structural index and no second pass:
//! escaped strings are decoded in place at their own offset, unescaped ones are referenced where
//! they lie, numbers are validated and converted in the same loop, and a container's node is
//! patched with its child count when it closes. This is also where the language's rules live as
//! functions with names -- [`number_from_parts`], [`decode_in_place`] -- so that the grammar is
//! decided once; `tests/agreement.rs` pins the number rule to `f64::from_str` across a sweep.

use crate::{
    Node, ParseError, TAG_ARRAY, TAG_FALSE, TAG_NULL, TAG_NUMBER, TAG_OBJECT, TAG_STRING, TAG_TRUE,
};

/// An open container while the scanner is inside it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    /// The container's node, whose `len` is written when it closes.
    node: usize,
    /// Elements, or key/value pairs, seen so far.
    children: u32,
    is_object: bool,
}

/// The exact powers of ten a double holds without rounding, `10^0` through `10^22`.
const EXACT_POW10: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// The double a JSON number denotes, from the parts the scanner collected in its one pass: the
/// sign, the first fifteen significant digits as an integer, how many significant digits there
/// were in all, the power of ten those digits sit at, and the token itself for the slow path.
///
/// The language's rule is the nearest double, ties to even, `-0` kept, and a magnitude a double
/// cannot hold answering infinity or zero. Fast path first: a mantissa of at most fifteen
/// significant digits is exact in a double, and so is every power of ten up to `10^22`, so one
/// multiply or divide of two exact values is the correctly rounded answer (Clinger, 1990).
/// Anything else goes to `f64::from_str`, which is Eisel-Lemire with a correct fallback, so both
/// routes agree on every input; the fast path is a shortcut, not a different rule.
pub(crate) fn number_from_parts(
    negative: bool,
    mantissa: u64,
    significant: u32,
    exponent: Option<i32>,
    token: &[u8],
) -> f64 {
    let fast = match exponent {
        Some(_) if mantissa == 0 => Some(0.0),
        Some(exponent) if significant <= 15 && (-22..=22).contains(&exponent) => {
            let base = mantissa as f64;
            Some(if exponent < 0 {
                base / EXACT_POW10[(-exponent) as usize]
            } else {
                base * EXACT_POW10[exponent as usize]
            })
        }
        _ => None,
    };
    let magnitude = match fast {
        Some(value) => value,
        None => std::str::from_utf8(token)
            .ok()
            .and_then(|text| text.parse::<f64>().ok())
            .unwrap_or(f64::NAN),
    };
    if negative { -magnitude } else { magnitude }
}

/// Scans the number at `buf[start]` by JSON's grammar -- an optional minus, an integer with no
/// leading zero, an optional fraction, an optional exponent -- collecting what
/// [`number_from_parts`] needs on the way, and returns the index past it with its value.
fn scan_number(buf: &[u8], start: usize) -> Result<(usize, f64), ParseError> {
    let mut at = start;
    let negative = buf.get(at) == Some(&b'-');
    if negative {
        at += 1;
    }
    let mut mantissa: u64 = 0;
    let mut significant: u32 = 0;
    let mut scale: i32 = 0;
    match buf.get(at) {
        Some(b'0') => at += 1,
        Some(b'1'..=b'9') => {
            while let Some(&byte @ b'0'..=b'9') = buf.get(at) {
                significant += 1;
                if significant <= 15 {
                    mantissa = mantissa * 10 + u64::from(byte - b'0');
                }
                at += 1;
            }
        }
        _ => return Err(ParseError::Syntax { offset: at }),
    }
    if buf.get(at) == Some(&b'.') {
        at += 1;
        let fraction_start = at;
        while let Some(&byte @ b'0'..=b'9') = buf.get(at) {
            if significant > 0 || byte != b'0' {
                significant += 1;
                if significant <= 15 {
                    mantissa = mantissa * 10 + u64::from(byte - b'0');
                }
            }
            scale -= 1;
            at += 1;
        }
        if at == fraction_start {
            return Err(ParseError::Syntax { offset: at });
        }
    }
    let mut exponent: Option<i32> = Some(scale);
    if let Some(b'e' | b'E') = buf.get(at) {
        at += 1;
        let exponent_negative = match buf.get(at) {
            Some(b'-') => {
                at += 1;
                true
            }
            Some(b'+') => {
                at += 1;
                false
            }
            _ => false,
        };
        let exponent_start = at;
        let mut explicit: Option<i32> = Some(0);
        while let Some(&byte @ b'0'..=b'9') = buf.get(at) {
            explicit = explicit
                .and_then(|value| value.checked_mul(10)?.checked_add(i32::from(byte - b'0')))
                .filter(|value| *value <= 400);
            at += 1;
        }
        if at == exponent_start {
            return Err(ParseError::Syntax { offset: at });
        }
        exponent = explicit.and_then(|explicit| {
            scale.checked_add(if exponent_negative {
                -explicit
            } else {
                explicit
            })
        });
    }
    // The digits past the fifteenth were dropped from `mantissa`; `significant` says so, and the
    // slow path reads the token itself -- its magnitude, the sign being applied once at the end.
    let magnitude = &buf[start + usize::from(negative)..at];
    Ok((
        at,
        number_from_parts(negative, mantissa, significant, exponent, magnitude),
    ))
}

/// Four hex digits, or `None`.
fn hex4(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }
    let mut unit = 0u32;
    for &byte in &bytes[..4] {
        let digit = (byte as char).to_digit(16)?;
        unit = (unit << 4) | digit;
    }
    Some(unit)
}

/// Writes the encoding of one scalar value at `buf[at..]` and returns the index past it. The
/// caller guarantees room: an escape is at least two bytes and encodes to at most four, and a
/// `\u` escape is six bytes for at most three, or twelve for a pair's four.
fn write_utf8(buf: &mut [u8], at: usize, scalar: u32) -> usize {
    let mut encoded = [0u8; 4];
    let bytes = char::from_u32(scalar)
        .unwrap_or('\u{FFFD}')
        .encode_utf8(&mut encoded)
        .as_bytes();
    buf[at..at + bytes.len()].copy_from_slice(bytes);
    at + bytes.len()
}

/// Decodes, in place, the string whose opening quote is at `buf[open]`, and returns the index
/// past its closing quote together with the decoded length. The decoded bytes start at `open + 1`
/// and never outrun the reader, because no escape decodes to more bytes than it spelled.
///
/// The language's rules for a string: the two-character escapes and `\u` with four hex digits;
/// a `\u` high surrogate followed immediately by a `\u` low surrogate is one character, and a
/// surrogate escape on its own is U+FFFD, because a kiln string cannot hold a lone surrogate
/// (ADR 0022); a raw byte below 0x20 is an error, as is any other escape. Bytes are otherwise
/// left as they are -- the input is a JavaScript string's UTF-8, valid by construction.
pub(crate) fn decode_in_place(buf: &mut [u8], open: usize) -> Result<(usize, usize), ParseError> {
    let start = open + 1;
    // The common case: no escape and no control byte before the closing quote, so the string is
    // already its own decoding. One byte loop with a three-way test: `memchr2` plus a control
    // check was measured slower on the strings a document like this is made of (5-20 bytes),
    // where a call and a second pass cost more than the loop.
    let mut at = start;
    while let Some(&byte) = buf.get(at) {
        match byte {
            b'"' => return Ok((at + 1, at - start)),
            b'\\' | 0x00..=0x1F => break,
            _ => at += 1,
        }
    }
    let mut write = at;
    loop {
        let Some(&byte) = buf.get(at) else {
            return Err(ParseError::Syntax { offset: at });
        };
        match byte {
            b'"' => return Ok((at + 1, write - start)),
            b'\\' => {
                let Some(&kind) = buf.get(at + 1) else {
                    return Err(ParseError::Syntax { offset: at });
                };
                at += 2;
                let decoded = match kind {
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'/' => b'/',
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'u' => {
                        let Some(unit) = hex4(&buf[at..]) else {
                            return Err(ParseError::Syntax { offset: at });
                        };
                        at += 4;
                        let scalar = if (0xD800..=0xDBFF).contains(&unit) {
                            // A high half: a low half spelled right after it completes the pair.
                            let low = if buf.get(at) == Some(&b'\\')
                                && buf.get(at + 1) == Some(&b'u')
                            {
                                hex4(&buf[at + 2..]).filter(|low| (0xDC00..=0xDFFF).contains(low))
                            } else {
                                None
                            };
                            match low {
                                Some(low) => {
                                    at += 6;
                                    0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
                                }
                                None => 0xFFFD,
                            }
                        } else if (0xDC00..=0xDFFF).contains(&unit) {
                            0xFFFD
                        } else {
                            unit
                        };
                        write = write_utf8(buf, write, scalar);
                        continue;
                    }
                    _ => return Err(ParseError::Syntax { offset: at - 1 }),
                };
                buf[write] = decoded;
                write += 1;
            }
            0x00..=0x1F => return Err(ParseError::Syntax { offset: at }),
            _ => {
                buf[write] = byte;
                write += 1;
                at += 1;
            }
        }
    }
}

/// The index past the JSON whitespace at `at`: space, tab, line feed, carriage return, and
/// nothing else -- a BOM or a no-break space is an error where a value was expected.
#[inline]
fn skip_whitespace(buf: &[u8], mut at: usize) -> usize {
    while let Some(b' ' | b'\t' | b'\n' | b'\r') = buf.get(at) {
        at += 1;
    }
    at
}

/// Decodes the string at `buf[at]` into a node and returns the index past it.
#[inline]
fn string_node(buf: &mut [u8], at: usize, nodes: &mut Vec<Node>) -> Result<usize, ParseError> {
    let (end, len) = decode_in_place(buf, at)?;
    nodes.push(Node {
        tag: TAG_STRING,
        len: u32::try_from(len).map_err(|_| ParseError::TooLarge)?,
        value: (at + 1) as u64,
    });
    Ok(end)
}

/// One pass over `src`, writing the tape to `nodes`; `arena` receives a copy of the source with
/// every escaped string decoded in place, which is what the tape's string offsets index.
///
/// The walk is iterative over `stack`, so a document nests as deep as it is long. A container's
/// node is pushed when it opens with a `len` of zero and patched when it closes.
pub(crate) fn scan(
    src: &[u8],
    nodes: &mut Vec<Node>,
    arena: &mut Vec<u8>,
    stack: &mut Vec<Frame>,
) -> Result<(), ParseError> {
    nodes.clear();
    stack.clear();
    arena.clear();
    arena.extend_from_slice(src);
    let buf = arena.as_mut_slice();
    let mut at = skip_whitespace(buf, 0);
    'value: loop {
        let Some(&byte) = buf.get(at) else {
            return Err(ParseError::Syntax { offset: at });
        };
        match byte {
            b'"' => at = string_node(buf, at, nodes)?,
            b'{' | b'[' => {
                let is_object = byte == b'{';
                let node = nodes.len();
                nodes.push(Node {
                    tag: if is_object { TAG_OBJECT } else { TAG_ARRAY },
                    len: 0,
                    value: 0,
                });
                at = skip_whitespace(buf, at + 1);
                let close = if is_object { b'}' } else { b']' };
                if buf.get(at) == Some(&close) {
                    at += 1;
                } else {
                    stack.push(Frame {
                        node,
                        children: 0,
                        is_object,
                    });
                    if is_object {
                        at = member_key(buf, at, nodes)?;
                    }
                    continue 'value;
                }
            }
            b't' => at = literal(buf, at, b"true", TAG_TRUE, nodes)?,
            b'f' => at = literal(buf, at, b"false", TAG_FALSE, nodes)?,
            b'n' => at = literal(buf, at, b"null", TAG_NULL, nodes)?,
            b'-' | b'0'..=b'9' => {
                let (end, value) = scan_number(buf, at)?;
                nodes.push(Node {
                    tag: TAG_NUMBER,
                    len: 0,
                    value: value.to_bits(),
                });
                at = end;
            }
            _ => return Err(ParseError::Syntax { offset: at }),
        }
        // A value is complete. Count it, and close every container that ends here.
        loop {
            let Some(frame) = stack.last_mut() else {
                at = skip_whitespace(buf, at);
                return if at == buf.len() {
                    Ok(())
                } else {
                    Err(ParseError::Syntax { offset: at })
                };
            };
            frame.children = frame.children.checked_add(1).ok_or(ParseError::TooLarge)?;
            at = skip_whitespace(buf, at);
            let Some(&next) = buf.get(at) else {
                return Err(ParseError::Syntax { offset: at });
            };
            if next == b',' {
                at = skip_whitespace(buf, at + 1);
                if frame.is_object {
                    at = member_key(buf, at, nodes)?;
                }
                continue 'value;
            }
            let close = if frame.is_object { b'}' } else { b']' };
            if next != close {
                return Err(ParseError::Syntax { offset: at });
            }
            nodes[frame.node].len = frame.children;
            stack.pop();
            at += 1;
        }
    }
}

/// A member's key and the colon after it, from `at` (whitespace already skipped); returns the
/// index of the value.
#[inline]
fn member_key(buf: &mut [u8], at: usize, nodes: &mut Vec<Node>) -> Result<usize, ParseError> {
    if buf.get(at) != Some(&b'"') {
        return Err(ParseError::Syntax { offset: at });
    }
    let after_key = string_node(buf, at, nodes)?;
    let colon = skip_whitespace(buf, after_key);
    if buf.get(colon) != Some(&b':') {
        return Err(ParseError::Syntax { offset: colon });
    }
    Ok(skip_whitespace(buf, colon + 1))
}

/// `true`, `false` or `null` at `at`, spelled exactly.
#[inline]
fn literal(
    buf: &[u8],
    at: usize,
    word: &[u8],
    tag: u32,
    nodes: &mut Vec<Node>,
) -> Result<usize, ParseError> {
    if buf.get(at..at + word.len()) != Some(word) {
        return Err(ParseError::Syntax { offset: at });
    }
    nodes.push(Node {
        tag,
        len: 0,
        value: 0,
    });
    Ok(at + word.len())
}
