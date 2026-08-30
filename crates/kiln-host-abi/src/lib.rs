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
}
