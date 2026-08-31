//! The small, versioned ABI between a Kiln program and its host.
//!
//! Compiler and runtime crates may evolve independently of the process that provides host
//! services. Keep this crate dependency-free and make values crossing the boundary owned.

/// Version of the host operation and value contract.
pub const HOST_ABI_VERSION: u32 = 2;

/// Operations a compiled program can request from its host.
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostOp {
    /// Write one line to the host's standard output.
    WriteStdout = 1,
    /// Wall-clock milliseconds since the Unix epoch for a host-clock realm.
    ClockNow = 2,
    /// One host-supplied entropy word for a host-entropy realm.
    EntropyU64 = 3,
    /// The host timezone identifier, such as `America/Denver`.
    TimeZone = 4,
    /// The host locale tag, such as `en-US`.
    Locale = 5,
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

/// Opaque identity of one asynchronous host request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    /// Creates an identity from its stable numeric representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric representation used by private embedding seams.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Opaque identity of a host resource owned by one realm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(u64);

impl ResourceId {
    /// Creates an identity from its stable numeric representation.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the numeric representation used by private embedding seams.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One asynchronous operation requested from a realm host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncHostOp {
    /// Read an entire file as owned bytes.
    ReadFile { path: String },
    /// Replace or create a file with the supplied bytes.
    WriteFile { path: String, data: Vec<u8> },
    /// Read metadata without opening a persistent file resource.
    Stat { path: String },
    /// Close a persistent host resource.
    CloseResource { resource: ResourceId },
    /// Change whether a persistent resource keeps its realm alive.
    SetResourceReferenced {
        resource: ResourceId,
        referenced: bool,
    },
    /// Inspect lifecycle state without exposing the host's resource-table representation.
    ResourceState { resource: ResourceId },
    /// Bind a TCP listener. Port zero asks the host to select an ephemeral port.
    TcpListen { host: String, port: u16 },
    /// Wait for one connection and return its stream resource.
    TcpAccept { listener: ResourceId },
    /// Connect a TCP stream.
    TcpConnect { host: String, port: u16 },
    /// Read up to `max_bytes` from a stream. Empty bytes represent EOF.
    TcpRead {
        stream: ResourceId,
        max_bytes: usize,
    },
    /// Write the complete byte buffer to a stream.
    TcpWrite { stream: ResourceId, data: Vec<u8> },
    /// Shut down the write half of a stream.
    TcpShutdown { stream: ResourceId },
}

/// Stable resource category that does not expose executor or operating-system types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    TcpListener,
    TcpStream,
}

/// Portable information returned when the host creates a persistent resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceInfo {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub address: Option<String>,
    pub port: Option<u16>,
}

/// Observable lifecycle state of a realm-owned resource.
///
/// Node adapters use this snapshot to implement `ref`, `unref`, `hasRef`, half-close, and
/// diagnostics without learning which executor or operating-system handle backs the resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceState {
    pub info: ResourceInfo,
    pub referenced: bool,
    /// True once the read half has observed EOF. Listeners do not have a read half.
    pub read_closed: bool,
    /// True once the write half has been shut down. Listeners do not have a write half.
    pub write_closed: bool,
}

/// Result of accepting a TCP connection, including both local and peer endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedTcpStream {
    pub resource: ResourceInfo,
    pub peer_address: String,
    pub peer_port: u16,
}

/// Portable file metadata needed by the first Node filesystem slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    /// File length in bytes.
    pub size: u64,
    /// Whether the path names a regular file.
    pub is_file: bool,
    /// Whether the path names a directory.
    pub is_directory: bool,
}

/// A successful asynchronous host operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncHostValue {
    /// No payload beyond successful completion.
    Undefined,
    /// Owned bytes, normally a file or stream chunk.
    Bytes(Vec<u8>),
    /// Portable filesystem metadata.
    FileStat(FileStat),
    /// A persistent resource inserted into the realm's resource table.
    Resource(ResourceInfo),
    /// Current lifecycle state of a persistent resource.
    ResourceState(ResourceState),
    /// A newly accepted TCP stream.
    AcceptedTcpStream(AcceptedTcpStream),
}

/// Stable error information that a Node compatibility Adapter can turn into a JavaScript error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsyncHostError {
    /// Node-compatible short error code where one exists, such as `ENOENT`.
    pub code: String,
    /// Operation name, such as `open` or `stat`.
    pub syscall: String,
    /// Path associated with the failure, if any.
    pub path: Option<String>,
    /// Human-readable detail supplied by the host implementation.
    pub message: String,
}

impl AsyncHostError {
    /// Constructs one structured asynchronous host error.
    pub fn new(
        code: impl Into<String>,
        syscall: impl Into<String>,
        path: Option<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            syscall: syscall.into(),
            path,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for AsyncHostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AsyncHostError {}

/// Result delivered when an asynchronous host request reaches a realm checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// The request being completed.
    pub request: RequestId,
    /// Monotonic submission order used as the deterministic readiness tie-break.
    pub sequence: u64,
    /// Operation result owned by the receiving realm.
    pub result: Result<AsyncHostValue, AsyncHostError>,
}

/// Starts and controls asynchronous operations without exposing an executor implementation.
pub trait AsyncHostOps {
    /// Start a non-resource operation and return its opaque request identity immediately.
    ///
    /// Persistent-resource operations are started by the runtime owner so creation and table
    /// mutation stay on the realm task; this trait remains the provider seam used by filesystem
    /// operations and deterministic scripted hosts.
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId;

    /// Cancel a pending operation, returning whether it was still owned by this host.
    fn cancel(&mut self, request: RequestId) -> bool;

    /// Return whether referenced work currently keeps the realm alive.
    fn has_referenced_work(&self) -> bool;

    /// Block until the next completion is ready, or return `None` when no requests remain.
    fn next_completion(&mut self) -> Option<Completion>;
}

/// Starts and cancels asynchronous services while code is executing on its realm task.
///
/// This is the narrow active-realm half of [`AsyncHostOps`]. It deliberately has no blocking
/// completion method: callers already inside a runtime must return control to the realm pump before
/// a provider can make progress. Persistent-resource operations use this same service boundary;
/// they are not a Node-specific executor API.
pub trait ActiveAsyncHostOps {
    /// Submit one operation and return its realm-scoped request identity immediately.
    fn start(&mut self, operation: AsyncHostOp, referenced: bool) -> RequestId;

    /// Cancel a pending operation, returning whether it was still owned by this host.
    fn cancel(&mut self, request: RequestId) -> bool;
}

/// A number as `String(value)` renders it.
///
/// The allocation-free semantic core writes into caller-owned storage; this boundary turns those
/// bytes into the owned string expected by interpreter hosts. Keeping the digit algorithm out of
/// this crate also keeps interpreter and native artifacts on one implementation.
pub fn number_to_js_string(value: f64) -> String {
    let mut output = [0_u8; kiln_number_core::MAX_FORMATTED_LEN];
    let Ok(length) = kiln_number_core::format_number(value, &mut output) else {
        // `MAX_FORMATTED_LEN` is the semantic core's own advertised bound. Reaching this branch
        // means that crate violated its API invariant; avoid converting that into a host panic.
        return "NaN".to_owned();
    };
    let Some(bytes) = output.get(..length) else {
        return "NaN".to_owned();
    };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Converts a string using the ECMAScript `StringNumericLiteral` grammar.
///
/// The semantic core consumes a bounded byte span so the same operation can cross the private C
/// runtime seam without allocation or NUL-termination assumptions.
pub fn string_to_number(value: &str) -> f64 {
    kiln_number_core::string_to_number(value.as_bytes())
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
    fn async_identities_round_trip_without_exposing_layout() {
        assert_eq!(RequestId::new(7).get(), 7);
        assert_eq!(ResourceId::new(11).get(), 11);
    }

    #[test]
    fn async_errors_keep_node_shaping_data_separate() {
        let error = AsyncHostError::new(
            "ENOENT",
            "open",
            Some("missing.txt".to_owned()),
            "no such file or directory",
        );
        assert_eq!(error.code, "ENOENT");
        assert_eq!(error.syscall, "open");
        assert_eq!(error.path.as_deref(), Some("missing.txt"));
        assert_eq!(error.to_string(), "ENOENT: no such file or directory");
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
        // Rust's scientific formatter chooses a different last digit here. ECMAScript requires
        // the shortest decimal which round-trips to the same binary64 value.
        assert_eq!(
            number_to_js_string(157_651_540_651_918.12),
            "157651540651918.12"
        );
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
