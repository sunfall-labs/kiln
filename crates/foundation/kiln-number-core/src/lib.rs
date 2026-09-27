//! Allocation-free ECMAScript number conversion, and the default numeric order of typed-array
//! elements ([`typed_order`]).
//!
//! This crate is the semantic-core proof for ADR 0018. It deliberately owns no C ABI, allocator,
//! runtime packaging, or host values: callers provide a byte span to parse or a byte buffer to
//! format. The native adapter and archive policy remain separate decisions.
#![no_std]
#![deny(unsafe_code)]
#![deny(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

pub mod typed_order;

/// The largest ECMAScript spelling emitted by `dragonbox_ecma` for an `f64`.
pub const MAX_FORMATTED_LEN: usize = 25;

/// A caller-owned formatting buffer was too small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatError {
    /// Required bytes, excluding any terminator. This API does not write one.
    pub required: usize,
}

/// Write ECMAScript's shortest `Number::toString` spelling into `output`.
///
/// Nothing is written when `output` is too small. The returned length never exceeds
/// [`MAX_FORMATTED_LEN`], and the bytes are always ASCII. A future C adapter can add its own NUL
/// terminator without making termination part of the language operation.
pub fn format_number(value: f64, output: &mut [u8]) -> Result<usize, FormatError> {
    let mut buffer = dragonbox_ecma::Buffer::new();
    let bytes = buffer.format(value).as_bytes();
    let required = bytes.len();
    let Some(destination) = output.get_mut(..required) else {
        return Err(FormatError { required });
    };
    destination.copy_from_slice(bytes);
    Ok(required)
}

/// Convert an ECMAScript `StringNumericLiteral` supplied as an arbitrary bounded byte span.
///
/// Invalid UTF-8, embedded NUL, non-ECMAScript whitespace, malformed literals, and partial
/// parses produce `NaN`. Empty input after ECMAScript whitespace trimming produces positive zero.
/// Binary, octal, and hexadecimal integer forms are rounded to binary64 with ties to even without
/// allocating, including inputs much larger than a machine integer.
pub fn string_to_number(input: &[u8]) -> f64 {
    let Ok(value) = core::str::from_utf8(input) else {
        return f64::NAN;
    };
    string_numeric_literal(value)
}

fn string_numeric_literal(value: &str) -> f64 {
    let value = value.trim_matches(is_ecmascript_whitespace);
    if value.is_empty() {
        return 0.0;
    }

    if let Some(value) = parse_radix_integer(value) {
        return value;
    }

    match value {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }

    if !is_decimal_literal(value) {
        return f64::NAN;
    }

    // The grammar check excludes Rust-only spellings such as `inf` and `NaN`. `core` performs
    // the required correctly-rounded decimal-to-binary64 conversion without allocation.
    value.parse::<f64>().unwrap_or(f64::NAN)
}

fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

fn is_decimal_literal(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = usize::from(
        bytes
            .first()
            .is_some_and(|byte| matches!(byte, b'+' | b'-')),
    );
    if index == bytes.len() {
        return false;
    }

    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index = index.saturating_add(1);
    }
    let has_integer_digits = index != integer_start;

    if bytes.get(index) == Some(&b'.') {
        index = index.saturating_add(1);
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index = index.saturating_add(1);
        }
        if !has_integer_digits && index == fraction_start {
            return false;
        }
    } else if !has_integer_digits {
        return false;
    }

    if bytes
        .get(index)
        .is_some_and(|byte| matches!(byte, b'e' | b'E'))
    {
        index = index.saturating_add(1);
        if bytes
            .get(index)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            index = index.saturating_add(1);
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index = index.saturating_add(1);
        }
        if index == exponent_start {
            return false;
        }
    }

    index == bytes.len()
}

/// `None` means the input has no unsigned radix prefix; `Some(NaN)` means it has one but is
/// malformed. This distinction lets the caller route ordinary decimal strings to `core`.
fn parse_radix_integer(value: &str) -> Option<f64> {
    let bytes = value.as_bytes();
    let (radix, bits_per_digit) = match (bytes.first(), bytes.get(1)) {
        (Some(b'0'), Some(b'x' | b'X')) => (16_u32, 4_usize),
        (Some(b'0'), Some(b'o' | b'O')) => (8, 3),
        (Some(b'0'), Some(b'b' | b'B')) => (2, 1),
        _ => return None,
    };
    let Some(digits) = bytes.get(2..) else {
        return Some(f64::NAN);
    };
    if digits.is_empty() {
        return Some(f64::NAN);
    }

    let mut first_nonzero = None;
    for (index, &byte) in digits.iter().enumerate() {
        let digit = digit_value(byte);
        if digit >= radix {
            return Some(f64::NAN);
        }
        if first_nonzero.is_none() && digit != 0 {
            first_nonzero = Some(index);
        }
    }
    let Some(first_nonzero) = first_nonzero else {
        return Some(0.0);
    };
    let Some(significant_digits) = digits.get(first_nonzero..) else {
        return Some(f64::NAN);
    };
    let Some(first_digit) = significant_digits.first().copied() else {
        return Some(f64::NAN);
    };

    let leading_bits = u32::BITS.saturating_sub(digit_value(first_digit).leading_zeros()) as usize;
    let bit_length = significant_digits
        .len()
        .saturating_sub(1)
        .saturating_mul(bits_per_digit)
        .saturating_add(leading_bits);
    let exponent = bit_length.saturating_sub(1);
    if exponent > 1023 {
        return Some(f64::INFINITY);
    }

    let mut significand = 0_u64;
    let mut bit_index = 0_usize;
    let mut guard = false;
    let mut sticky = false;
    for (digit_index, &byte) in significant_digits.iter().enumerate() {
        let digit = digit_value(byte);
        let digit_bits = if digit_index == 0 {
            leading_bits
        } else {
            bits_per_digit
        };
        for shift in (0..digit_bits).rev() {
            let bit = (digit >> shift) & 1;
            if bit_index < 53 {
                significand = (significand << 1) | u64::from(bit);
            } else if bit_index == 53 {
                guard = bit != 0;
            } else if bit != 0 {
                sticky = true;
            }
            bit_index = bit_index.saturating_add(1);
        }
    }

    if bit_length <= 53 {
        return Some(significand as f64);
    }

    if guard && (sticky || significand & 1 != 0) {
        significand = significand.saturating_add(1);
        if significand == 1 << 53 {
            if exponent == 1023 {
                return Some(f64::INFINITY);
            }
            return Some(f64::from_bits((exponent as u64).saturating_add(1024) << 52));
        }
    }

    Some(f64::from_bits(
        ((exponent as u64).saturating_add(1023) << 52) | significand.saturating_sub(1 << 52),
    ))
}

fn digit_value(digit: u8) -> u32 {
    match digit {
        b'0'..=b'9' => u32::from(digit.saturating_sub(b'0')),
        b'a'..=b'f' => u32::from(digit.saturating_sub(b'a').saturating_add(10)),
        b'A'..=b'F' => u32::from(digit.saturating_sub(b'A').saturating_add(10)),
        _ => u32::MAX,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;
    use std::string::String;
    use std::vec;

    fn formatted(value: f64) -> String {
        let mut output = [0_u8; MAX_FORMATTED_LEN];
        let Ok(length) = format_number(value, &mut output) else {
            return String::new();
        };
        let Some(bytes) = output.get(..length) else {
            return String::new();
        };
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn formats_ecmascript_thresholds_specials_and_negative_zero() {
        for (value, expected) in [
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (0.1 + 0.2, "0.30000000000000004"),
            (5e-324, "5e-324"),
            (-0.0, "0"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
        ] {
            assert_eq!(formatted(value), expected);
        }
    }

    #[test]
    fn a_short_format_buffer_is_unchanged_and_reports_exact_capacity() {
        let mut short = [b'X'; 2];
        assert_eq!(
            format_number(1.234, &mut short),
            Err(FormatError { required: 5 })
        );
        assert_eq!(short, [b'X'; 2]);
        let mut exact = [0_u8; 5];
        assert_eq!(format_number(1.234, &mut exact), Ok(5));
        assert_eq!(&exact, b"1.234");
    }

    #[test]
    fn parses_the_string_numeric_literal_boundaries() {
        for (text, expected) in [
            ("", 0.0),
            (" \t\u{000B}\u{000C}\n\r", 0.0),
            ("+1", 1.0),
            ("1.", 1.0),
            (".5", 0.5),
            ("01.25e+2", 125.0),
            ("Infinity", f64::INFINITY),
            ("+Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("1e309", f64::INFINITY),
            ("5e-324", 5e-324),
            ("1e-324", 0.0),
            ("9007199254740993", 9007199254740992.0),
            ("0x10", 16.0),
            ("0XfF", 255.0),
            ("0b11", 3.0),
            ("0o10", 8.0),
            ("0x20000000000001", 9007199254740992.0),
            ("0x20000000000003", 9007199254740996.0),
        ] {
            assert_eq!(
                string_to_number(text.as_bytes()).to_bits(),
                expected.to_bits()
            );
        }
        assert!(string_to_number(b"-0").is_sign_negative());
        assert!(string_to_number(b"-1e-324").is_sign_negative());
    }

    #[test]
    fn accepts_only_ecmascript_whitespace_and_valid_utf8() {
        let whitespace = "\u{0009}\u{000A}\u{000B}\u{000C}\u{000D}\u{0020}\u{00A0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";
        let surrounded = format!("{whitespace}1{whitespace}");
        assert_eq!(string_to_number(surrounded.as_bytes()), 1.0);
        assert!(string_to_number("\u{180E}1".as_bytes()).is_nan());
        for invalid in [&[0xC0, 0xAF][..], &[0xED, 0xA0, 0x80], b"1\0"] {
            assert!(string_to_number(invalid).is_nan());
        }
    }

    #[test]
    fn rejects_partial_decimal_and_signed_radix_forms() {
        for text in [
            "NaN",
            "nan",
            "inf",
            "INFINITY",
            "1e",
            "1e+",
            "1_000",
            "1.2x",
            "Infinityx",
            "0x",
            "0b",
            "0o",
            "+0x10",
            "-0x10",
            "0x1p2",
            "0b2",
            "0o8",
        ] {
            assert!(
                string_to_number(text.as_bytes()).is_nan(),
                "accepted {text}"
            );
        }
    }

    #[test]
    fn radix_conversion_is_unbounded_and_rounds_ties_to_even() {
        let mut enormous = vec![b'1'; 20_000];
        enormous.splice(0..0, *b"0b");
        assert_eq!(string_to_number(&enormous), f64::INFINITY);
        assert_eq!(string_to_number(b"0x20000000000002"), 9007199254740994.0);
        assert_eq!(string_to_number(b"0x3fffffffffffff"), 18014398509481984.0);
    }

    #[test]
    fn formatted_binary64_samples_round_trip_without_exceeding_the_bound() {
        let mut state = 0x6b69_6c6e_2d72_7975_u64;
        for _ in 0..20_000 {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut bits = state;
            bits = (bits ^ (bits >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            bits = (bits ^ (bits >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            bits ^= bits >> 31;
            let value = f64::from_bits(bits);
            let text = formatted(value);
            assert!(text.len() <= MAX_FORMATTED_LEN);
            if value.is_finite() {
                assert_eq!(
                    string_to_number(text.as_bytes()).to_bits(),
                    value.to_bits(),
                    "{bits:#018x}: {text}"
                );
            }
        }
    }

    #[test]
    fn formats_the_shortest_round_trip_when_rust_scientific_rounding_differs() {
        assert_eq!(formatted(157_651_540_651_918.12), "157651540651918.12");
    }
}
