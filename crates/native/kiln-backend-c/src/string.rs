use std::fmt::Write;

/// [`c_string`], broken into adjacent literals so no single one grows unbounded.
///
/// C99 only requires a compiler to accept a 4095-character string literal, and a unit's whole
/// source is routinely far past that -- a bundle is megabytes. Adjacent string literals are
/// concatenated by the translation phase that runs before the limit applies, so splitting costs
/// nothing at run time and keeps the emitted C inside what the standard guarantees.
pub(crate) fn c_string_chunked(value: &str) -> String {
    const CHUNK: usize = 2048;
    let mut output = String::from("\"");
    let mut since = 0usize;
    for byte in value.bytes() {
        let before = output.len();
        push_c_byte(&mut output, byte);
        since += output.len() - before;
        if since >= CHUNK {
            output.push_str("\"\n\"");
            since = 0;
        }
    }
    output.push('"');
    output
}

/// One source byte as it is spelled inside a C string literal.
fn push_c_byte(output: &mut String, byte: u8) {
    match byte {
        b'"' => output.push_str("\\\""),
        b'\\' => output.push_str("\\\\"),
        // C translates trigraphs before it parses string literals. Spelling either question
        // mark in `??=` as `\\?` preserves JavaScript source text instead of turning it into
        // `#` on compilers that still perform that translation.
        b'?' => output.push_str("\\?"),
        b'\n' => output.push_str("\\n"),
        b'\r' => output.push_str("\\r"),
        b'\t' => output.push_str("\\t"),
        0x20..=0x7e => output.push(char::from(byte)),
        // The empty literal that follows ends the hex escape, which would otherwise swallow any
        // hex digit written next to it.
        _ => write!(output, "\\x{byte:02X}\"\"").unwrap(),
    }
}

pub(crate) fn c_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    let bytes = value.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        let next_escape = remaining
            .iter()
            .position(|&b| b < 0x20 || b == b'"' || b == b'\\' || b == b'?' || b > 0x7e);
        match next_escape {
            Some(pos) => {
                if pos > 0 {
                    output.push_str(&value[offset..offset + pos]);
                }
                push_c_byte(&mut output, remaining[pos]);
                offset += pos + 1;
            }
            None => {
                output.push_str(&value[offset..]);
                break;
            }
        }
    }
    output.push('"');
    output
}

#[cfg(test)]
mod tests {
    use super::{c_string, c_string_chunked};

    #[test]
    fn question_marks_cannot_form_c_trigraphs() {
        assert_eq!(c_string("left ??= right"), r#""left \?\?= right""#);
        assert_eq!(c_string_chunked("left ??= right"), r#""left \?\?= right""#);
    }
}
