//! The small, versioned ABI between a Kiln program and its host.
//!
//! Compiler and runtime crates may evolve independently of the process that provides host
//! services. Keep this crate dependency-free and make values crossing the boundary owned.

/// Version of the host operation and value contract.
pub const HOST_ABI_VERSION: u32 = 1;

/// Operations a compiled program can request from its host.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostOp {
    /// Write one line to the host's standard output.
    WriteStdout = 1,
}

/// A scalar value that is safe to own at the host boundary.
///
/// Numbers are represented by their IEEE-754 bits. This preserves NaN payloads and negative
/// zero while keeping equality total and making serialization the responsibility of a future
/// ABI transport rather than this foundational crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(u64),
    String(String),
}

impl HostValue {
    pub const fn number(value: f64) -> Self {
        Self::Number(value.to_bits())
    }

    pub const fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }

    /// JavaScript's scalar display form, suitable for `console.log`-style output.
    /// The value as a console writes it.
    ///
    /// Differs from [`Self::to_js_string`] only for negative zero, which `String(-0)` renders as
    /// `0` and a console renders as `-0` -- the console is showing the value, not converting it.
    pub fn to_console_string(&self) -> String {
        match self {
            Self::Number(bits) => {
                let value = f64::from_bits(*bits);
                if value == 0.0 && value.is_sign_negative() {
                    return "-0".to_owned();
                }
                number_to_js_string(value)
            }
            other => other.to_js_string(),
        }
    }

    pub fn to_js_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_owned(),
            Self::Null => "null".to_owned(),
            Self::Boolean(value) => value.to_string(),
            Self::Number(bits) => number_to_js_string(f64::from_bits(*bits)),
            Self::String(value) => value.clone(),
        }
    }
}

/// A number as `String(value)` renders it.
///
/// Not `f64::to_string`: JavaScript switches to exponential notation outside
/// `1e-7 < |value| < 1e21`, and Rust does not, so `String(1e21)` differs from `1e21.to_string()`
/// by fifteen characters of zeroes. The shortest round-tripping digits come from `{:e}`, which
/// hands over the digits and the exponent already separated; the rest is the specification's
/// case analysis on where the decimal point lands.
pub fn number_to_js_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "Infinity".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".to_owned();
    }
    // Both zero signs render as `0`. A console shows `-0`, which is a different question and
    // lives in `to_console_string`.
    if value == 0.0 {
        return "0".to_owned();
    }

    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` always writes an exponent");
    let exponent: i32 = exponent.parse().expect("`{:e}` writes a decimal exponent");
    let digits: String = mantissa.chars().filter(|digit| *digit != '.').collect();
    // `k` digits, with the decimal point `n` places from the left, in the specification's terms.
    let k = digits.len() as i32;
    let n = exponent + 1;

    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (whole, fraction) = digits.split_at(n as usize);
        format!("{whole}.{fraction}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(-n as usize))
    } else {
        // The exponent is written with an explicit `+` when positive, and never padded.
        let power = n - 1;
        let sign = if power < 0 { "-" } else { "+" };
        let magnitude = power.abs();
        if k == 1 {
            format!("{digits}e{sign}{magnitude}")
        } else {
            let (first, rest) = digits.split_at(1);
            format!("{first}.{rest}e{sign}{magnitude}")
        }
    };

    if value < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

/// Converts a string using the ECMAScript `StringNumericLiteral` grammar.
///
/// This intentionally does not use [`str::trim`]: ECMAScript has its own, smaller, precisely
/// specified set of leading and trailing whitespace characters.  Decimal literals are validated
/// before handing them to the standard library parser, while the non-decimal integer forms are
/// rounded to binary64 explicitly so that arbitrarily long inputs have the specified ties-to-even
/// behavior.
pub fn string_to_number(value: &str) -> f64 {
    let value = value.trim_matches(is_ecmascript_whitespace);
    if value.is_empty() {
        return 0.0;
    }

    if let Some(value) = parse_radix_integer(value) {
        return value;
    }

    if value == "Infinity" {
        return f64::INFINITY;
    }
    if value == "+Infinity" {
        return f64::INFINITY;
    }
    if value == "-Infinity" {
        return f64::NEG_INFINITY;
    }

    if !is_decimal_literal(value) {
        return f64::NAN;
    }

    // The grammar check above excludes Rust's accepted `inf`, `NaN`, and other non-ECMAScript
    // spellings, leaving this conversion as the correctly rounded decimal conversion.
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
    let mut index = 0;
    if bytes
        .first()
        .is_some_and(|character| *character == b'+' || *character == b'-')
    {
        index += 1;
    }
    if index == bytes.len() {
        return false;
    }

    let integer_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    let has_integer_digits = index != integer_start;

    if index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        let fraction_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if !has_integer_digits && index == fraction_start {
            return false;
        }
    } else if !has_integer_digits {
        return false;
    }

    if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
        index += 1;
        if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
            index += 1;
        }
        let exponent_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }

    index == bytes.len()
}

/// Returns `None` when `value` is not one of the unsigned `0x`, `0o`, or `0b` forms.  Returns
/// `Some(NaN)` for a radix prefix with invalid or missing digits, allowing the caller to continue
/// treating all other malformed strings uniformly as NaN.
fn parse_radix_integer(value: &str) -> Option<f64> {
    let bytes = value.as_bytes();
    let (radix, bits_per_digit) = match bytes.get(..2) {
        Some(b"0x") | Some(b"0X") => (16, 4),
        Some(b"0o") | Some(b"0O") => (8, 3),
        Some(b"0b") | Some(b"0B") => (2, 1),
        _ => return None,
    };

    let digits = &bytes[2..];
    if digits.is_empty() {
        return Some(f64::NAN);
    }

    let mut first_nonzero = None;
    for (index, &digit) in digits.iter().enumerate() {
        let valid = match radix {
            16 => digit.is_ascii_hexdigit(),
            8 => matches!(digit, b'0'..=b'7'),
            2 => matches!(digit, b'0' | b'1'),
            _ => unreachable!(),
        };
        if !valid {
            return Some(f64::NAN);
        }
        if first_nonzero.is_none() && digit_value(digit) != 0 {
            first_nonzero = Some(index);
        }
    }

    let Some(first_nonzero) = first_nonzero else {
        return Some(0.0);
    };
    let significant_digits = &digits[first_nonzero..];

    // The exact bit length is only needed to distinguish the finite range and to form the
    // exponent. Saturating here is safe: any saturated value is far beyond binary64's range.
    let leading_bits = (32 - digit_value(significant_digits[0]).leading_zeros()) as usize;
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
    for (digit_index, &digit) in significant_digits.iter().enumerate() {
        let digit = digit_value(digit);
        let first_digit_bits = if digit_index == 0 {
            leading_bits
        } else {
            bits_per_digit
        };
        for shift in (0..first_digit_bits).rev() {
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
        significand += 1;
        if significand == 1 << 53 {
            if exponent == 1023 {
                return Some(f64::INFINITY);
            }
            return Some(f64::from_bits((exponent as u64 + 1 + 1023) << 52));
        }
    }

    Some(f64::from_bits(
        ((exponent as u64 + 1023) << 52) | (significand - (1 << 52)),
    ))
}

fn digit_value(digit: u8) -> u32 {
    match digit {
        b'0'..=b'9' => u32::from(digit - b'0'),
        b'a'..=b'f' => u32::from(digit - b'a' + 10),
        b'A'..=b'F' => u32::from(digit - b'A' + 10),
        _ => 0,
    }
}

/// An operation failure reported by a host implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostError {
    pub message: String,
}

impl HostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HostError {}

/// Host implementation invoked by the interpreter or a future native backend.
pub trait HostOps {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_round_trip_as_bits() {
        let value = HostValue::number(-0.0);
        assert_eq!(value, HostValue::Number((-0.0_f64).to_bits()));
        assert_eq!(value.as_number(), Some(-0.0));
        assert_eq!(value.to_js_string(), "0");
    }

    #[test]
    fn scalar_display_is_js_like() {
        assert_eq!(HostValue::Undefined.to_js_string(), "undefined");
        assert_eq!(HostValue::Null.to_js_string(), "null");
        assert_eq!(HostValue::Boolean(true).to_js_string(), "true");
        assert_eq!(HostValue::number(f64::NAN).to_js_string(), "NaN");
        assert_eq!(HostValue::number(f64::INFINITY).to_js_string(), "Infinity");
        assert_eq!(HostValue::String("kiln".into()).to_js_string(), "kiln");
    }

    #[test]
    fn numbers_switch_to_exponential_where_javascript_does() {
        // The two boundaries, from both sides. `f64::to_string` crosses neither, which is the
        // whole reason this function exists.
        assert_eq!(number_to_js_string(1e20), "100000000000000000000");
        assert_eq!(number_to_js_string(1e21), "1e+21");
        assert_eq!(number_to_js_string(1e-6), "0.000001");
        assert_eq!(number_to_js_string(1e-7), "1e-7");
        assert_eq!(number_to_js_string(1.5e21), "1.5e+21");
        assert_eq!(number_to_js_string(1.2345e-7), "1.2345e-7");
        assert_eq!(number_to_js_string(-1e21), "-1e+21");
        assert_eq!(number_to_js_string(5e-324), "5e-324");
    }

    #[test]
    fn ordinary_numbers_keep_their_shortest_form() {
        assert_eq!(number_to_js_string(100.0), "100");
        assert_eq!(number_to_js_string(1.5), "1.5");
        assert_eq!(number_to_js_string(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(number_to_js_string(1.0 / 3.0), "0.3333333333333333");
        assert_eq!(number_to_js_string(2f64.powi(53)), "9007199254740992");
    }

    #[test]
    fn the_non_finite_values_have_names_rather_than_digits() {
        assert_eq!(number_to_js_string(f64::NAN), "NaN");
        assert_eq!(number_to_js_string(f64::INFINITY), "Infinity");
        assert_eq!(number_to_js_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn only_a_console_distinguishes_negative_zero() {
        // `String(-0)` is "0" and a console shows "-0": converting and displaying are different
        // questions, and the two methods are where that difference lives.
        assert_eq!(number_to_js_string(-0.0), "0");
        assert_eq!(HostValue::number(-0.0).to_js_string(), "0");
        assert_eq!(HostValue::number(-0.0).to_console_string(), "-0");
        assert_eq!(HostValue::number(0.0).to_console_string(), "0");
        // Everything else is the same through both.
        assert_eq!(HostValue::number(1.5).to_console_string(), "1.5");
    }

    #[test]
    fn string_numbers_accept_the_decimal_grammar() {
        assert_eq!(string_to_number("0"), 0.0);
        assert_eq!(string_to_number("+12"), 12.0);
        assert_eq!(string_to_number("-12."), -12.0);
        assert_eq!(string_to_number(".125"), 0.125);
        assert_eq!(string_to_number("12.5e-1"), 1.25);
        assert_eq!(string_to_number("1E+3"), 1000.0);
        assert_eq!(string_to_number("Infinity"), f64::INFINITY);
        assert_eq!(string_to_number("+Infinity"), f64::INFINITY);
        assert_eq!(string_to_number("-Infinity"), f64::NEG_INFINITY);
    }

    #[test]
    fn string_numbers_trim_only_ecmascript_whitespace() {
        let whitespace = "\u{0009}\u{000A}\u{000B}\u{000C}\u{000D}\u{0020}\u{00A0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}";
        assert_eq!(
            string_to_number(&format!("{whitespace}42{whitespace}")),
            42.0
        );
        assert!(string_to_number("\u{180E}42").is_nan());
        assert!(string_to_number("4\u{0085}2").is_nan());
    }

    #[test]
    fn string_numbers_reject_non_literals() {
        for value in [
            "+", "-", ".", ".e1", "1e", "1e+", "1..0", "1 2", "inf", "-inf", "nan", "NaN", "0x",
            "0o", "0b", "0x1g", "0o8", "0b2", "+0x1", "-0b1",
        ] {
            assert!(
                string_to_number(value).is_nan(),
                "accepted invalid literal {value:?}"
            );
        }
    }

    #[test]
    fn string_numbers_preserve_negative_zero_and_empty_is_positive_zero() {
        assert_eq!(string_to_number(""), 0.0);
        assert!(!string_to_number("").is_sign_negative());
        assert!(string_to_number("-0").is_sign_negative());
        assert!(string_to_number("-0.0e+100").is_sign_negative());
    }

    #[test]
    fn string_numbers_parse_unsigned_radix_forms() {
        assert_eq!(string_to_number("0b101010"), 42.0);
        assert_eq!(string_to_number("0o755"), 493.0);
        assert_eq!(string_to_number("0xDeadBeef"), 3_735_928_559.0);
        assert_eq!(string_to_number("0x0000000000000001"), 1.0);
        assert_eq!(
            string_to_number(&("0b".to_owned() + &"1".repeat(200))),
            2_f64.powi(200)
        );
    }

    #[test]
    fn string_numbers_radix_rounding_is_ties_to_even() {
        // At this magnitude the spacing is two.  +1 is exactly halfway and rounds down to the
        // even significand; +3 is halfway between the next pair and rounds up.
        assert_eq!(
            string_to_number("0x20000000000001"),
            9_007_199_254_740_992.0
        );
        assert_eq!(
            string_to_number("0x20000000000003"),
            9_007_199_254_740_996.0
        );
        assert_eq!(
            string_to_number("0x20000000000002"),
            9_007_199_254_740_994.0
        );

        // The same check through binary and octal digit boundaries exercises leading-bit removal.
        assert_eq!(
            string_to_number(&format!("0b1{}1", "0".repeat(52))),
            2_f64.powi(53)
        );
        assert_eq!(
            string_to_number(&format!("0o4{}1", "0".repeat(16))),
            2_f64.powi(53)
        );
    }

    #[test]
    fn string_numbers_radix_overflow_to_infinity() {
        let max_finite_bits = format!("0b{}{}", "1".repeat(53), "0".repeat(971));
        assert_eq!(
            string_to_number(&max_finite_bits),
            f64::from_bits(0x7fefffffffffffff)
        );
        let overflow_tie = format!("0b{}1{}", "1".repeat(53), "0".repeat(970));
        assert_eq!(string_to_number(&overflow_tie), f64::INFINITY);
        assert_eq!(
            string_to_number(&format!("0x1{}", "0".repeat(256))),
            f64::INFINITY
        );
        assert_eq!(
            string_to_number(&format!("0b1{}", "0".repeat(1024))),
            f64::INFINITY
        );
        assert_eq!(
            string_to_number("0xffffffffffffffff"),
            18_446_744_073_709_551_616.0
        );
    }
}
