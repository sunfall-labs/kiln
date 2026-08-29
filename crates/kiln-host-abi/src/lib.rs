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

fn number_to_js_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "Infinity".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".to_owned();
    }
    // JavaScript renders both zero signs as `0`.
    if value == 0.0 {
        return "0".to_owned();
    }
    value.to_string()
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
}
