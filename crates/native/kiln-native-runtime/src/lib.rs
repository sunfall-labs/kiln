//! The C embedding contract and small runtime used by Kiln's native command-line launcher.
//!
//! The generated program translation units deliberately do not contain host policy.  This crate
//! emits the stable, reusable callback object that implements the command-line `WriteStdout`
//! operation, plus a tiny executable launcher for one generated module.  Keeping those sources
//! separate lets the runtime object be cached independently from application code. Embedded hosts
//! consume [`embedding_header`] and supply their own callback policy without linking the CLI code.

use std::sync::OnceLock;

mod launch;

pub use launch::{
    ChildStream, inherit_built_executable, output_built_executable, spawn_built_executable,
    stream_built_executable,
};

/// Version of the generated native runtime source and its C ABI.
pub const NATIVE_RUNTIME_VERSION: u32 = 14;

/// Environment variable that enables private scalar-result sidecar emission by the CLI launcher.
pub const NATIVE_RESULT_PATH_ENV: &str = "KILN_NATIVE_RESULT_PATH_V1";

/// Magic prefix for the private scalar-result sidecar format.
///
/// Version 2 writes every string as little-endian UTF-16 code units.  Keep the version in the
/// magic so an old reader cannot silently interpret an even-length UTF-8 payload as UTF-16.
pub const NATIVE_RESULT_MAGIC: [u8; 8] = *b"KILNRES2";

const NATIVE_RESULT_HEADER_LEN: usize = NATIVE_RESULT_MAGIC.len() + 1 + 8;

/// A malformed private scalar-result sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeResultError {
    Truncated { needed: usize, available: usize },
    InvalidMagic,
    UnknownTag(u8),
    InvalidLength { tag: u8, expected: u64, actual: u64 },
    InvalidBoolean(u8),
    LengthOverflow(u64),
    InvalidStringPayload,
    TrailingBytes(usize),
}

impl std::fmt::Display for NativeResultError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { needed, available } => {
                write!(
                    formatter,
                    "truncated native result (need {needed} bytes, have {available})"
                )
            }
            Self::InvalidMagic => formatter.write_str("invalid native result magic"),
            Self::UnknownTag(tag) => write!(formatter, "unknown native result tag {tag}"),
            Self::InvalidLength {
                tag,
                expected,
                actual,
            } => write!(
                formatter,
                "invalid native result length for tag {tag} (expected {expected}, got {actual})"
            ),
            Self::InvalidBoolean(value) => {
                write!(formatter, "invalid native result boolean {value}")
            }
            Self::LengthOverflow(length) => {
                write!(formatter, "native result length {length} overflows usize")
            }
            Self::InvalidStringPayload => {
                formatter.write_str("native result string has invalid UTF-16 payload")
            }
            Self::TrailingBytes(count) => {
                write!(formatter, "native result has {count} trailing bytes")
            }
        }
    }
}

impl std::error::Error for NativeResultError {}

/// Decode one complete `KILNRES2` sidecar record into the shared host scalar type.
pub fn decode_native_result(bytes: &[u8]) -> Result<kiln_host_abi::HostValue, NativeResultError> {
    if bytes.len() < NATIVE_RESULT_HEADER_LEN {
        return Err(NativeResultError::Truncated {
            needed: NATIVE_RESULT_HEADER_LEN,
            available: bytes.len(),
        });
    }
    if bytes[..NATIVE_RESULT_MAGIC.len()] != NATIVE_RESULT_MAGIC {
        return Err(NativeResultError::InvalidMagic);
    }

    let tag_offset = NATIVE_RESULT_MAGIC.len();
    let tag = bytes[tag_offset];
    let length_offset = tag_offset + 1;
    let mut length_bytes = [0_u8; 8];
    length_bytes.copy_from_slice(&bytes[length_offset..NATIVE_RESULT_HEADER_LEN]);
    let length = u64::from_le_bytes(length_bytes);

    let expected_length = match tag {
        0 | 1 => 0,
        2 => 1,
        3 => 8,
        4 => {
            let length =
                usize::try_from(length).map_err(|_| NativeResultError::LengthOverflow(length))?;
            let payload_end = NATIVE_RESULT_HEADER_LEN
                .checked_add(length)
                .ok_or(NativeResultError::LengthOverflow(length as u64))?;
            if bytes.len() < payload_end {
                return Err(NativeResultError::Truncated {
                    needed: payload_end,
                    available: bytes.len(),
                });
            }
            if bytes.len() != payload_end {
                return Err(NativeResultError::TrailingBytes(bytes.len() - payload_end));
            }
            let payload = &bytes[NATIVE_RESULT_HEADER_LEN..payload_end];
            if payload.len() % 2 != 0 {
                return Err(NativeResultError::InvalidStringPayload);
            }
            let units = payload
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            return Ok(kiln_host_abi::HostValue::String(
                kiln_string::StringUnits::from_units(&units),
            ));
        }
        other => return Err(NativeResultError::UnknownTag(other)),
    };
    if length != expected_length {
        return Err(NativeResultError::InvalidLength {
            tag,
            expected: expected_length,
            actual: length,
        });
    }

    let payload_end = NATIVE_RESULT_HEADER_LEN + expected_length as usize;
    if bytes.len() < payload_end {
        return Err(NativeResultError::Truncated {
            needed: payload_end,
            available: bytes.len(),
        });
    }
    if bytes.len() != payload_end {
        return Err(NativeResultError::TrailingBytes(bytes.len() - payload_end));
    }

    Ok(match tag {
        0 => kiln_host_abi::HostValue::Undefined,
        1 => kiln_host_abi::HostValue::Null,
        2 => match bytes[NATIVE_RESULT_HEADER_LEN] {
            0 => kiln_host_abi::HostValue::Boolean(false),
            1 => kiln_host_abi::HostValue::Boolean(true),
            value => return Err(NativeResultError::InvalidBoolean(value)),
        },
        3 => {
            let mut bits = [0_u8; 8];
            bits.copy_from_slice(&bytes[NATIVE_RESULT_HEADER_LEN..payload_end]);
            kiln_host_abi::HostValue::Number(u64::from_le_bytes(bits))
        }
        _ => unreachable!("all native result tags handled above"),
    })
}

/// Encode one sidecar record from the shared host scalar type.
pub fn encode_native_result(value: &kiln_host_abi::HostValue) -> Vec<u8> {
    let (tag, payload): (u8, Vec<u8>) = match value {
        kiln_host_abi::HostValue::Undefined => (0, Vec::new()),
        kiln_host_abi::HostValue::Null => (1, Vec::new()),
        kiln_host_abi::HostValue::Boolean(value) => (2, vec![u8::from(*value)]),
        kiln_host_abi::HostValue::Number(bits) => (3, bits.to_le_bytes().to_vec()),
        kiln_host_abi::HostValue::String(value) => {
            let mut payload = Vec::with_capacity(value.unit_len() * 2);
            for unit in value.units() {
                payload.extend_from_slice(&unit.to_le_bytes());
            }
            (4, payload)
        }
    };
    let mut encoded = Vec::with_capacity(NATIVE_RESULT_HEADER_LEN + payload.len());
    encoded.extend_from_slice(&NATIVE_RESULT_MAGIC);
    encoded.push(tag);
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    encoded.extend_from_slice(&payload);
    encoded
}

/// C entry-point ABI version declared by [`embedding_header`].
pub const EMBEDDING_C_ABI_VERSION: u32 = 1;

/// Version of the generated module API declared by [`embedding_module_header`].
pub const EMBEDDING_MODULE_API_VERSION: u32 = 2;

/// Host callback ABI version declared by [`embedding_header`].
pub const EMBEDDING_HOST_ABI_VERSION: u32 = 7;

/// The operation implemented by [`runtime_source`].
pub const WRITE_STDOUT_OPERATION: u16 = 1;

/// The single C declaration of the boundary value ABI.
///
/// This macro lets generated C source consume the same literal inside `concat!` without a
/// build-script filesystem dependency or runtime allocation.
#[macro_export]
macro_rules! kiln_value_abi_declaration {
    () => {
        r#"typedef struct kiln_value {
    uint16_t tag;
    uint16_t flags;
    uint32_t len;
    union { uint64_t bits; const char *ptr; };
} kiln_value;
#define KILN_VALUE_FLAG_OWNED_STRING UINT16_C(1)

#if defined(__cplusplus)
static_assert(offsetof(kiln_value, tag) == 0, "kiln_value.tag ABI offset");
static_assert(offsetof(kiln_value, flags) == 2, "kiln_value.flags ABI offset");
static_assert(offsetof(kiln_value, len) == 4, "kiln_value.len ABI offset");
static_assert(offsetof(kiln_value, bits) == 8, "kiln_value.bits ABI offset");
static_assert(offsetof(kiln_value, ptr) == 8, "kiln_value.ptr shares the payload word");
static_assert(sizeof(kiln_value) == 16, "kiln_value fits the AAPCS64 register threshold");
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(offsetof(kiln_value, tag) == 0, "kiln_value.tag ABI offset");
_Static_assert(offsetof(kiln_value, flags) == 2, "kiln_value.flags ABI offset");
_Static_assert(offsetof(kiln_value, len) == 4, "kiln_value.len ABI offset");
_Static_assert(offsetof(kiln_value, bits) == 8, "kiln_value.bits ABI offset");
_Static_assert(offsetof(kiln_value, ptr) == 8, "kiln_value.ptr shares the payload word");
_Static_assert(sizeof(kiln_value) == 16, "kiln_value fits the AAPCS64 register threshold");
#endif

"#
    };
}

/// The CLI host's per-realm state (ADR 0070, realms issue 04 step 5): the last host refusal and
/// the string header its text is read through. A launcher owns one per realm and passes it as the
/// context of `kiln_cli_host_call`; a static would be one refusal that every realm in the process
/// overwrites and every launcher then reports. `output` is where the realm's standard streams go:
/// NULL for the process's own stdout and stderr, or one file for both when the realm's host gave
/// it one -- a realm hosted beside others in one process (realms issue 05). Requires the value and
/// string declarations and `<stdio.h>`.
#[macro_export]
macro_rules! kiln_cli_host_state_declaration {
    () => {
        r#"typedef struct kiln_cli_host_state {
    kiln_value last_error;
    kiln_string error_text;
    FILE *output;
    /* Where the realm's standard error goes when it is not `output`: a worker's
     * (`node:worker_threads`) goes to its parent as messages, one stream apart from the other. */
    FILE *error_output;
} kiln_cli_host_state;
"#
    };
}

/// The canonical immutable JavaScript string header shared by generated C and runtime code.
#[macro_export]
macro_rules! kiln_string_abi_declaration {
    () => {
        r#"#define KILN_STRING_LATIN1 UINT32_C(0)
#define KILN_STRING_UTF16 UINT32_C(1)
#define KILN_STRING_SLICED UINT32_C(2)
/* Property-key identity, never a character in the string. A Symbol's private key and a user
 * string with identical UTF-16 units must occupy different canonical-key domains. */
#define KILN_STRING_SYMBOL_KEY UINT32_C(4)
#define KILN_STRING_PRIVATE_KEY UINT32_C(8)
typedef struct kiln_string {
    uint32_t unit_len;
    uint32_t flags;
    const void *data;
} kiln_string;
typedef struct kiln_sliced_string {
    kiln_string base;
    const kiln_string *parent;
} kiln_sliced_string;
#if defined(__cplusplus)
static_assert(offsetof(kiln_string, unit_len) == 0, "kiln_string.unit_len ABI offset");
static_assert(offsetof(kiln_string, flags) == 4, "kiln_string.flags ABI offset");
static_assert(offsetof(kiln_string, data) == 8, "kiln_string.data ABI offset");
static_assert(sizeof(kiln_string) == 16, "kiln_string ABI size");
static_assert(offsetof(kiln_sliced_string, base) == 0, "kiln_sliced_string.base ABI offset");
static_assert(offsetof(kiln_sliced_string, parent) == 16, "kiln_sliced_string.parent ABI offset");
static_assert(sizeof(kiln_sliced_string) == 24, "kiln_sliced_string ABI size");
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(offsetof(kiln_string, unit_len) == 0, "kiln_string.unit_len ABI offset");
_Static_assert(offsetof(kiln_string, flags) == 4, "kiln_string.flags ABI offset");
_Static_assert(offsetof(kiln_string, data) == 8, "kiln_string.data ABI offset");
_Static_assert(sizeof(kiln_string) == 16, "kiln_string ABI size");
_Static_assert(offsetof(kiln_sliced_string, base) == 0, "kiln_sliced_string.base ABI offset");
_Static_assert(offsetof(kiln_sliced_string, parent) == 16, "kiln_sliced_string.parent ABI offset");
_Static_assert(sizeof(kiln_sliced_string) == 24, "kiln_sliced_string ABI size");
#endif
static inline int kiln_string_is_utf16(const kiln_string *string) {
    return string != NULL && (string->flags & KILN_STRING_UTF16) != 0;
}
static inline int kiln_string_is_sliced(const kiln_string *string) {
    return string != NULL && (string->flags & KILN_STRING_SLICED) != 0;
}
static inline const uint8_t *kiln_string_latin1(const kiln_string *string) {
    return (const uint8_t *)(string == NULL ? NULL : string->data);
}
static inline const uint16_t *kiln_string_utf16(const kiln_string *string) {
    return (const uint16_t *)(string == NULL ? NULL : string->data);
}
static inline uint16_t kiln_string_unit_at(const kiln_string *string, size_t index) {
    if (string == NULL || index >= string->unit_len || string->data == NULL) return 0;
    return kiln_string_is_utf16(string) ? kiln_string_utf16(string)[index]
                                        : kiln_string_latin1(string)[index];
}
static inline int kiln_string_equals_ascii(const kiln_string *string, const char *ascii,
                                           size_t length) {
    size_t index;
    if (string == NULL || (ascii == NULL && length != 0) || string->unit_len != length) return 0;
    for (index = 0; index < length; ++index)
        if (kiln_string_unit_at(string, index) != (uint8_t)ascii[index]) return 0;
    return 1;
}
/* Compare a JavaScript UTF-16 string with an already UTF-8 filesystem path without allocating.
 * Valid surrogate pairs are encoded as their four-byte scalar; unpaired surrogates cannot equal
 * a filesystem path represented by UTF-8 bytes. */
static inline int kiln_string_equals_utf8(const kiln_string *string, const char *utf8,
                                           size_t length) {
    size_t unit = 0;
    size_t byte = 0;
    if (string == NULL || (utf8 == NULL && length != 0)) return 0;
    while (unit < string->unit_len) {
        uint32_t codepoint = kiln_string_unit_at(string, unit++);
        uint8_t encoded[4];
        size_t width;
        if (codepoint >= UINT32_C(0xD800) && codepoint <= UINT32_C(0xDBFF)) {
            if (unit >= string->unit_len) return 0;
            uint32_t low = kiln_string_unit_at(string, unit++);
            if (low < UINT32_C(0xDC00) || low > UINT32_C(0xDFFF)) return 0;
            codepoint = UINT32_C(0x10000) + ((codepoint - UINT32_C(0xD800)) << 10) +
                        (low - UINT32_C(0xDC00));
        } else if (codepoint >= UINT32_C(0xDC00) && codepoint <= UINT32_C(0xDFFF)) {
            return 0;
        }
        if (codepoint < UINT32_C(0x80)) {
            encoded[0] = (uint8_t)codepoint;
            width = 1;
        } else if (codepoint < UINT32_C(0x800)) {
            encoded[0] = (uint8_t)(UINT32_C(0xC0) | (codepoint >> 6));
            encoded[1] = (uint8_t)(UINT32_C(0x80) | (codepoint & UINT32_C(0x3F)));
            width = 2;
        } else if (codepoint < UINT32_C(0x10000)) {
            encoded[0] = (uint8_t)(UINT32_C(0xE0) | (codepoint >> 12));
            encoded[1] = (uint8_t)(UINT32_C(0x80) | ((codepoint >> 6) & UINT32_C(0x3F)));
            encoded[2] = (uint8_t)(UINT32_C(0x80) | (codepoint & UINT32_C(0x3F)));
            width = 3;
        } else {
            encoded[0] = (uint8_t)(UINT32_C(0xF0) | (codepoint >> 18));
            encoded[1] = (uint8_t)(UINT32_C(0x80) | ((codepoint >> 12) & UINT32_C(0x3F)));
            encoded[2] = (uint8_t)(UINT32_C(0x80) | ((codepoint >> 6) & UINT32_C(0x3F)));
            encoded[3] = (uint8_t)(UINT32_C(0x80) | (codepoint & UINT32_C(0x3F)));
            width = 4;
        }
        if (width > length - byte) return 0;
        for (size_t i = 0; i < width; ++i)
            if ((uint8_t)utf8[byte++] != encoded[i]) return 0;
    }
    return byte == length;
}
static inline int kiln_string_starts_ascii(const kiln_string *string, const char *ascii,
                                           size_t length) {
    size_t index;
    if (string == NULL || ascii == NULL || string->unit_len < length) return 0;
    for (index = 0; index < length; ++index)
        if (kiln_string_unit_at(string, index) != (uint8_t)ascii[index]) return 0;
    return 1;
}
static inline int kiln_string_equal(const kiln_string *left, const kiln_string *right) {
    if (left == NULL || right == NULL || left->unit_len != right->unit_len) return 0;
    if ((left->flags & (KILN_STRING_LATIN1 | KILN_STRING_UTF16)) == (right->flags & (KILN_STRING_LATIN1 | KILN_STRING_UTF16)) && left->unit_len != 0)
        return memcmp(left->data, right->data,
                      (size_t)left->unit_len * (kiln_string_is_utf16(left) ? sizeof(uint16_t) : sizeof(uint8_t))) == 0;
    for (size_t index = 0; index < left->unit_len; ++index)
        if (kiln_string_unit_at(left, index) != kiln_string_unit_at(right, index)) return 0;
    return 1;
}
static inline int kiln_string_compare_units(const kiln_string *left, const kiln_string *right) {
    size_t index;
    size_t common;
    if (left == NULL || right == NULL) return left == right ? 0 : (left == NULL ? -1 : 1);
    if (!kiln_string_is_utf16(left) && !kiln_string_is_utf16(right)) {
        common = left->unit_len < right->unit_len ? left->unit_len : right->unit_len;
        if (common > 0) {
            int cmp = memcmp(left->data, right->data, common);
            if (cmp != 0) return cmp < 0 ? -1 : 1;
        }
        return left->unit_len == right->unit_len ? 0 : (left->unit_len < right->unit_len ? -1 : 1);
    }
    common = left->unit_len < right->unit_len ? left->unit_len : right->unit_len;
    for (index = 0; index < common; ++index) {
        uint16_t a = kiln_string_unit_at(left, index);
        uint16_t b = kiln_string_unit_at(right, index);
        if (a != b) return a < b ? -1 : 1;
    }
    return left->unit_len == right->unit_len ? 0 : (left->unit_len < right->unit_len ? -1 : 1);
}
static inline size_t kiln_string_unit_width(const kiln_string *string) {
    return kiln_string_is_utf16(string) ? sizeof(uint16_t) : sizeof(uint8_t);
}
static inline int kiln_string_storage_size(uint32_t unit_len, uint32_t flags, size_t *out) {
    size_t width;
    uint32_t enc = flags & (KILN_STRING_LATIN1 | KILN_STRING_UTF16);
    if ((flags & ~(KILN_STRING_LATIN1 | KILN_STRING_UTF16 | KILN_STRING_SLICED | KILN_STRING_SYMBOL_KEY | KILN_STRING_PRIVATE_KEY)) != 0) return 0;
    width = enc == KILN_STRING_UTF16 ? sizeof(uint16_t) : sizeof(uint8_t);
    size_t units = (size_t)unit_len;
    if (units > (SIZE_MAX - sizeof(kiln_string)) / width) return 0;
    *out = sizeof(kiln_string) + units * width;
    return 1;
}
"#
    };
}

/// Return the C header shared by embedded modules and the native CLI runtime.
///
/// The header is self-contained, include-guarded, and contains only the stable value/callback
/// contract. It is suitable for writing to a header file or embedding directly in a generated C
/// translation unit.
pub fn embedding_header() -> &'static str {
    EMBEDDING_HEADER
}

/// Return the canonical embedding ABI plus a declaration for one generated module.
///
/// The module declaration has its own symbol-specific include guard so callers can safely
/// compose headers for several modules. C++ consumers receive C linkage for the generated
/// entry point, matching the C object emitted by `kiln-backend-c`.
pub fn embedding_module_header(symbol: &str) -> Result<String, RuntimeSourceError> {
    validate_symbol(symbol)?;
    let api_symbol = embedding_module_api_symbol(symbol)?;
    let guard = format!("KILN_NATIVE_MODULE_{symbol}_H");
    let mut header = String::with_capacity(
        embedding_header().len()
            + MODULE_HEADER_PREFIX.len()
            + 3 * guard.len()
            + MODULE_HEADER_DEFINE.len()
            + MODULE_HEADER_DECL_PREFIX.len()
            + symbol.len()
            + api_symbol.len()
            + MODULE_HEADER_DECL_SUFFIX.len()
            + " */\n".len(),
    );
    header.push_str(embedding_header());
    header.push_str(MODULE_HEADER_PREFIX);
    header.push_str(&guard);
    header.push_str(MODULE_HEADER_DEFINE);
    header.push_str(&guard);
    header.push_str(MODULE_HEADER_DECL_PREFIX);
    header.push_str(symbol);
    header.push_str(MODULE_HEADER_DECL_API_SEPARATOR);
    header.push_str(&api_symbol);
    header.push_str(MODULE_HEADER_DECL_SUFFIX);
    header.push_str(&guard);
    header.push_str(" */\n");
    Ok(header)
}

/// Return the v2 owned-result API symbol for a generated module base symbol.
pub fn embedding_module_api_symbol(base: &str) -> Result<String, RuntimeSourceError> {
    validate_symbol(base)?;
    Ok(format!("{base}_api_v2"))
}

/// Why a native runtime source request was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSourceError {
    /// The requested entry point is not a valid portable C identifier.
    InvalidSymbol { symbol: String },
}

impl std::fmt::Display for RuntimeSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSymbol { symbol } => {
                write!(formatter, "invalid native module symbol `{symbol}`")
            }
        }
    }
}

impl std::error::Error for RuntimeSourceError {}

/// Short compatibility spelling for callers that do not distinguish source kinds.
pub type RuntimeError = RuntimeSourceError;

/// Return the deterministic, program-independent C11 runtime translation unit.
pub fn runtime_source() -> &'static str {
    RUNTIME_SOURCE
        .get_or_init(|| {
            let mut source = String::with_capacity(
                RUNTIME_PREFIX.len() + embedding_header().len() + RUNTIME_BODY.len(),
            );
            source.push_str(RUNTIME_PREFIX);
            source.push_str(embedding_header());
            source.push_str(RUNTIME_BODY);
            source
        })
        .as_str()
}

/// Emit a deterministic C11 launcher that calls `symbol` as the process entry point.
///
/// A module returns zero on success and a nonzero status for a host/control-flow failure.  The
/// launcher maps that status to the conventional process exit values 0 and 1.
pub fn launcher_source(symbol: &str) -> Result<String, RuntimeSourceError> {
    validate_symbol(symbol)?;
    let mut source = String::with_capacity(
        LAUNCHER_PREFIX.len()
            + embedding_header().len()
            + LAUNCHER_DECL_PREFIX.len()
            + 2 * symbol.len()
            + LAUNCHER_DECL_SUFFIX.len()
            + LAUNCHER_BODY.len(),
    );
    source.push_str(LAUNCHER_PREFIX);
    source.push_str(embedding_header());
    source.push_str(LAUNCHER_DECL_PREFIX);
    source.push_str(symbol);
    source.push_str(LAUNCHER_DECL_SUFFIX);
    source.push_str(symbol);
    source.push_str(LAUNCHER_BODY);
    Ok(source)
}

/// Alias with an explicit verb for callers that keep several generated source kinds together.
pub fn emit_launcher(symbol: &str) -> Result<String, RuntimeError> {
    launcher_source(symbol)
}

fn validate_symbol(symbol: &str) -> Result<(), RuntimeSourceError> {
    let mut bytes = symbol.bytes();
    let Some(first) = bytes.next() else {
        return Err(RuntimeSourceError::InvalidSymbol {
            symbol: symbol.to_owned(),
        });
    };
    if !(first == b'_' || first.is_ascii_alphabetic())
        || bytes.any(|byte| !(byte == b'_' || byte.is_ascii_alphanumeric()))
    {
        return Err(RuntimeSourceError::InvalidSymbol {
            symbol: symbol.to_owned(),
        });
    }
    Ok(())
}

/// Keep this declaration byte-for-byte compatible with kiln-backend-c's generated C99 units.
/// The runtime is C11, but only uses facilities available in C99 for portability.
const EMBEDDING_HEADER: &str = concat!(
    r#"/* Kiln native embedding ABI. */
#ifndef KILN_NATIVE_EMBEDDING_H
#define KILN_NATIVE_EMBEDDING_H

#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#define KILN_NATIVE_RUNTIME_VERSION 14
#define KILN_C_ABI_VERSION 1
#define KILN_HOST_ABI_VERSION 7
#define KILN_MODULE_API_VERSION 2
#define KILN_C99_HOST_MODULE_LOADER UINT16_C(0x7FF1)

#define KILN_C99_OK 0
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)
#define KILN_C99_OUT_OF_MEMORY (-4)
#define KILN_C99_CALL_DEPTH_ERROR (-5)
#define KILN_C99_UNINITIALIZED_ERROR (-6)
/* A throw the program did not catch. The entry point converts the thrown value to text before
 * returning this, because a reference cannot cross the boundary and the launcher's whole job with
 * it is to print it. */
#define KILN_C99_THROWN (-7)

#define KILN_HOST_OP_WRITE_STDOUT 1
#define KILN_HOST_OP_CLOCK_NOW 2
#define KILN_HOST_OP_ENTROPY_U64 3
#define KILN_HOST_OP_WRITE_STDERR 6

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4
};

/* This layout is the complete boundary value.  `bits` and `ptr` share one word because a value
 * is a payload or a pointer and never both: `bits` stores booleans and IEEE-754 numbers, `ptr`
 * points at a canonical kiln_string header and `len` mirrors its UTF-16 code-unit count.
 *
 * Sixteen bytes is a threshold rather than a size. AAPCS64 passes a composite of sixteen bytes or
 * fewer in registers and anything larger indirectly, by reference to a copy -- so at the previous
 * thirty-two bytes every argument and every return travelled through memory. Measured at 1.37x on
 * a call-bound benchmark, and at nothing on benchmarks whose values already stayed in registers,
 * which is the shape that says the threshold is what moved rather than the byte count. */"#,
    kiln_value_abi_declaration!(),
    kiln_string_abi_declaration!(),
    r#"/* Argument ptr/len values are borrowed for the callback invocation.  A string stored in *result
 * must remain alive until the module entry point returns.  A nonzero callback return is
 * propagated as KILN_C99_HOST_ERROR. */
typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
    void *module_context;
    void *module_owner;
} kiln_host;

/* An invocation-scoped owned result.  The v2 run function initializes *result to NULL before
 * doing any work.  On successful allocation it returns a handle even when its nonzero status
 * reports a JavaScript throw, so the thrown scalar can still be read.  read writes a borrowed
 * scalar to *value; a string remains valid until dispose.  dispose takes a pointer-to-pointer,
 * clears it, and is idempotent when passed NULL.  Handles belong to one invocation: callers must
 * not copy or share ownership, and every handle must be disposed before unloading its module DSO. */
typedef struct kiln_result_v2 kiln_result_v2;
typedef struct kiln_module_api_v2 {
    uint32_t abi_version;
    uint32_t struct_size;
    int (*run)(kiln_host *host, kiln_result_v2 **result);
    int (*read)(const kiln_result_v2 *result, kiln_value *value);
    void (*dispose)(kiln_result_v2 **result);
} kiln_module_api_v2;

#if defined(__cplusplus)
static_assert(offsetof(kiln_module_api_v2, abi_version) == 0,
              "kiln_module_api_v2.abi_version ABI offset");
static_assert(offsetof(kiln_module_api_v2, struct_size) == sizeof(uint32_t),
              "kiln_module_api_v2.struct_size ABI offset");
static_assert(offsetof(kiln_module_api_v2, run) == 2 * sizeof(uint32_t),
              "kiln_module_api_v2.run ABI offset");
static_assert(offsetof(kiln_module_api_v2, read)
                  == offsetof(kiln_module_api_v2, run)
                      + sizeof(((kiln_module_api_v2 *)0)->run),
              "kiln_module_api_v2.read ABI offset");
static_assert(offsetof(kiln_module_api_v2, dispose)
                  == offsetof(kiln_module_api_v2, read)
                      + sizeof(((kiln_module_api_v2 *)0)->read),
              "kiln_module_api_v2.dispose ABI offset");
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(offsetof(kiln_module_api_v2, abi_version) == 0,
               "kiln_module_api_v2.abi_version ABI offset");
_Static_assert(offsetof(kiln_module_api_v2, struct_size) == sizeof(uint32_t),
               "kiln_module_api_v2.struct_size ABI offset");
_Static_assert(offsetof(kiln_module_api_v2, run) == 2 * sizeof(uint32_t),
               "kiln_module_api_v2.run ABI offset");
_Static_assert(offsetof(kiln_module_api_v2, read)
                   == offsetof(kiln_module_api_v2, run)
                       + sizeof(((kiln_module_api_v2 *)0)->run),
               "kiln_module_api_v2.read ABI offset");
_Static_assert(offsetof(kiln_module_api_v2, dispose)
                   == offsetof(kiln_module_api_v2, read)
                       + sizeof(((kiln_module_api_v2 *)0)->read),
               "kiln_module_api_v2.dispose ABI offset");
#endif


"#,
    kiln_cli_host_state_declaration!(),
    r#"
#endif /* KILN_NATIVE_EMBEDDING_H */"#,
);

const MODULE_HEADER_PREFIX: &str = r#"
/* Kiln native module embedding declaration. */
#ifndef "#;

const MODULE_HEADER_DEFINE: &str = r#"
#define "#;

const MODULE_HEADER_DECL_PREFIX: &str = r#"

#if defined(__cplusplus)
extern "C" {
#endif
extern int "#;

const MODULE_HEADER_DECL_API_SEPARATOR: &str = r#"(kiln_host *host, kiln_value *result);
extern const kiln_module_api_v2 *"#;

const MODULE_HEADER_DECL_SUFFIX: &str = r#"(void);
#if defined(__cplusplus)
} /* extern "C" */
#endif

#endif /* "#;

static RUNTIME_SOURCE: OnceLock<String> = OnceLock::new();

const RUNTIME_PREFIX: &str = r#"/* Kiln native CLI runtime: deterministic HostOps callback. */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#if defined(_WIN32)
#include <windows.h>
#include <bcrypt.h>
#endif

"#;

const RUNTIME_BODY: &str = r#"
/* Private number-provider ABI. The native linker supplies these from the validated runtime
 * provider artifact; they are deliberately absent from the public embedding header. */
extern int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
extern double kiln_string_to_number(const char *bytes, size_t length);

/* The sink is a parameter because `console.error` and `console.warn` go to standard error, and
 * which stream a line went to is observable to anyone redirecting one and not the other. */
static int kiln_cli_write_bytes(FILE *sink, const char *bytes, size_t length) {
    if (length != 0 && bytes == NULL) {
        return KILN_C99_HOST_ERROR;
    }
    if (length != 0 && fwrite(bytes, 1, length, sink) != length) {
        return KILN_C99_HOST_ERROR;
    }
    return KILN_C99_OK;
}

static int kiln_cli_write_result_u64(FILE *file, uint64_t value) {
    unsigned char bytes[8];
    size_t index;
    for (index = 0; index < sizeof(bytes); ++index) {
        bytes[index] = (unsigned char)((value >> (index * 8)) & 0xffu);
    }
    return fwrite(bytes, 1, sizeof(bytes), file) == sizeof(bytes)
        ? KILN_C99_OK
        : KILN_C99_HOST_ERROR;
}

static int kiln_cli_validate_result_string(const kiln_string *string, int *utf16) {
    if (string == NULL || utf16 == NULL) return KILN_C99_INVALID_ARGUMENT;
    /* Only the canonical encodings are valid at this boundary. */
    if ((string->flags & ~(KILN_STRING_LATIN1 | KILN_STRING_UTF16 | KILN_STRING_SLICED)) != 0) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (string->unit_len != 0 && string->data == NULL) return KILN_C99_INVALID_ARGUMENT;
    *utf16 = kiln_string_is_utf16(string);
    return KILN_C99_OK;
}

static int kiln_cli_write_result_string(FILE *file, const kiln_string *string) {
    unsigned char bytes[4096];
    size_t at = 0;
    int utf16;
    if (kiln_cli_validate_result_string(string, &utf16) != KILN_C99_OK) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    while (at < string->unit_len) {
        size_t count = string->unit_len - at;
        size_t index;
        if (count > sizeof(bytes) / 2) count = sizeof(bytes) / 2;
        for (index = 0; index < count; ++index) {
            uint16_t unit = utf16
                                ? ((const uint16_t *)string->data)[at + index]
                                : ((const uint8_t *)string->data)[at + index];
            bytes[index * 2] = (unsigned char)(unit & 0xffu);
            bytes[index * 2 + 1] = (unsigned char)((unit >> 8) & 0xffu);
        }
        if (fwrite(bytes, 1, count * 2, file) != count * 2) return KILN_C99_HOST_ERROR;
        at += count;
    }
    return KILN_C99_OK;
}

/* Error text crosses the process boundary as UTF-8 on stderr, while the runtime value keeps
 * UTF-16 code units. Keep this codec local to the diagnostic boundary; language operations keep
 * their canonical header and never use it as a key identity. */
static int kiln_cli_write_string(FILE *sink, const kiln_string *string) {
    size_t index;
    if (kiln_cli_validate_result_string(string, &(int){0}) != KILN_C99_OK) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    for (index = 0; index < string->unit_len; ++index) {
        uint32_t unit = kiln_string_unit_at(string, index);
        uint32_t codepoint = unit;
        if (kiln_string_is_utf16(string) && unit >= 0xD800 && unit <= 0xDBFF &&
            index + 1 < string->unit_len) {
            uint32_t low = kiln_string_unit_at(string, index + 1);
            if (low >= 0xDC00 && low <= 0xDFFF) {
                codepoint = 0x10000u + ((unit - 0xD800u) << 10) + (low - 0xDC00u);
                ++index;
            }
        }
        if (codepoint >= 0xD800 && codepoint <= 0xDFFF) codepoint = 0xFFFD;
        if (codepoint < 0x80) {
            if (fputc((int)codepoint, sink) == EOF) return KILN_C99_HOST_ERROR;
        } else if (codepoint < 0x800) {
            if (fputc((int)(0xC0 | (codepoint >> 6)), sink) == EOF ||
                fputc((int)(0x80 | (codepoint & 0x3F)), sink) == EOF)
                return KILN_C99_HOST_ERROR;
        } else if (codepoint < 0x10000) {
            if (fputc((int)(0xE0 | (codepoint >> 12)), sink) == EOF ||
                fputc((int)(0x80 | ((codepoint >> 6) & 0x3F)), sink) == EOF ||
                fputc((int)(0x80 | (codepoint & 0x3F)), sink) == EOF)
                return KILN_C99_HOST_ERROR;
        } else if (fputc((int)(0xF0 | (codepoint >> 18)), sink) == EOF ||
                   fputc((int)(0x80 | ((codepoint >> 12) & 0x3F)), sink) == EOF ||
                   fputc((int)(0x80 | ((codepoint >> 6) & 0x3F)), sink) == EOF ||
                   fputc((int)(0x80 | (codepoint & 0x3F)), sink) == EOF) {
            return KILN_C99_HOST_ERROR;
        }
    }
    return KILN_C99_OK;
}

/* A string value over static text, read through `header`, which the caller keeps alive as long
 * as the value: the refusing realm's host state. */
static kiln_value kiln_cli_ascii_value(kiln_string *header, const char *text, size_t length) {
    kiln_value value = { .tag = KILN_VALUE_STRING, .flags = 0, .len = 0, .bits = 0 };
    header->unit_len = length > UINT32_MAX ? 0 : (uint32_t)length;
    header->flags = KILN_STRING_LATIN1;
    header->data = text;
    value.ptr = (const char *)header;
    value.len = header->unit_len;
    return value;
}

/* The host state of a caller that passed none: this thread's, for an embedder calling
 * `kiln_cli_host_call` directly. Every launcher passes its realm's own. */
static _Thread_local kiln_cli_host_state kiln_cli_thread_host_state;

static kiln_cli_host_state *kiln_cli_state_of(void *context) {
    return context != NULL ? (kiln_cli_host_state *)context : &kiln_cli_thread_host_state;
}

/* Record a host refusal in the realm's state and answer with it. */
static kiln_value kiln_cli_refused(kiln_cli_host_state *state, const char *text, size_t length) {
    state->last_error = kiln_cli_ascii_value(&state->error_text, text, length);
    return state->last_error;
}

/* Private KILNRES2 scalar-result writer. The explicit-path form is used by focused C tests;
 * the environment wrapper below is the only launcher-facing entry point. */
int kiln_cli_write_result_file(const char *path, kiln_value value) {
    static const unsigned char magic[] = {'K', 'I', 'L', 'N', 'R', 'E', 'S', '2'};
    FILE *file;
    uint8_t tag;
    uint64_t length;
    int status = KILN_C99_OK;
    /* The result file is a public scalar boundary.  Preserve bytes and length, but never
     * expose the internal GC-ownership provenance outside the runtime ABI. */
    value.flags = 0;
    if (path == NULL || path[0] == '\0') {
        return KILN_C99_INVALID_ARGUMENT;
    }
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_NULL:
        tag = (uint8_t)value.tag;
        length = 0;
        break;
    case KILN_VALUE_BOOLEAN:
        if (value.bits > 1) {
            return KILN_C99_INVALID_ARGUMENT;
        }
        tag = (uint8_t)value.tag;
        length = 1;
        break;
    case KILN_VALUE_NUMBER:
        tag = (uint8_t)value.tag;
        length = 8;
        break;
    case KILN_VALUE_STRING:
        if (value.ptr == NULL) {
            return KILN_C99_INVALID_ARGUMENT;
        }
        {
            const kiln_string *string = (const kiln_string *)value.ptr;
            int utf16;
            if (kiln_cli_validate_result_string(string, &utf16) != KILN_C99_OK) {
                return KILN_C99_INVALID_ARGUMENT;
            }
            (void)utf16;
            length = (uint64_t)string->unit_len * 2;
        }
        tag = (uint8_t)value.tag;
        break;
    default:
        return KILN_C99_INVALID_ARGUMENT;
    }
    file = fopen(path, "wb");
    if (file == NULL) {
        return KILN_C99_HOST_ERROR;
    }
    if (fwrite(magic, 1, sizeof(magic), file) != sizeof(magic)
        || fwrite(&tag, 1, sizeof(tag), file) != sizeof(tag)
        || kiln_cli_write_result_u64(file, length) != KILN_C99_OK) {
        status = KILN_C99_HOST_ERROR;
    } else if (value.tag == KILN_VALUE_BOOLEAN) {
        unsigned char boolean_value = (unsigned char)value.bits;
        if (fwrite(&boolean_value, 1, sizeof(boolean_value), file) != sizeof(boolean_value)) {
            status = KILN_C99_HOST_ERROR;
        }
    } else if (value.tag == KILN_VALUE_NUMBER) {
        status = kiln_cli_write_result_u64(file, value.bits);
    } else if (value.tag == KILN_VALUE_STRING) {
        status = kiln_cli_write_result_string(file, (const kiln_string *)value.ptr);
    }
    if (fclose(file) != 0) {
        status = KILN_C99_HOST_ERROR;
    }
    return status;
}

/* Whether the result file can carry this value at all.
 *
 * The boundary carries five scalar tags and nothing else, and a module's completion value is
 * under no obligation to be one of them: a CommonJS entry completes with its `module.exports`,
 * which is an object in every module that does not assign a scalar to it. So every successful
 * `--node` run handed an object to a writer that understands only scalars, the writer said
 * `KILN_C99_INVALID_ARGUMENT`, and the launcher turned that into `exit 1` -- a program that had
 * printed exactly the right bytes and had nothing at all to say on stderr.
 *
 * Declining to write it is right; charging it to the *program's* exit status was not. An
 * unrepresentable result is an absent result, which is precisely what the `Option<HostValue>` on
 * the reading side of this file already models. */
static int kiln_cli_result_representable(kiln_value value) {
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_NULL:
    case KILN_VALUE_BOOLEAN:
    case KILN_VALUE_NUMBER:
    case KILN_VALUE_STRING:
        return 1;
    default:
        return 0;
    }
}

int kiln_cli_write_result(kiln_value value) {
    const char *path = getenv("KILN_NATIVE_RESULT_PATH_V1");
    if (path == NULL || path[0] == '\0') {
        return KILN_C99_OK;
    }
    /* No file at all, rather than an empty or partial one: the reader reads an absent file as
     * "this run finished and produced no scalar result", and a half-written one as a decode
     * failure, which is the report the program does not deserve. */
    if (!kiln_cli_result_representable(value)) {
        return KILN_C99_OK;
    }
    value.flags = 0;
    return kiln_cli_write_result_file(path, value);
}

/* Why the program stopped, on stderr.
 *
 * Without this a native run that threw exited 1 in silence, which is the failure mode the whole
 * corpus is built to avoid: a refusal you can see beats a wrong answer you cannot, and an exit
 * code with no words is neither.
 *
 * Every line here is prefixed `kiln: `, because this stream now carries the program's own
 * `console.error` output as well. The prefix is the only thing separating the engine's words
 * about the program from the program's words, and it belongs where the words are written: the
 * caller used to add it by prefixing the whole stream, which made a program's own error output
 * read as a compiler diagnostic. */
kiln_value kiln_cli_describe_error(kiln_value value) {
    if (value.tag == KILN_VALUE_STRING && value.ptr != NULL) {
        return value;
    }
    if (value.tag == 16 /* KILN_VALUE_PROMISE */ && value.bits != 0) {
        typedef struct kiln_runtime_promise_view {
            int state;
            kiln_value val;
        } kiln_runtime_promise_view;
        kiln_runtime_promise_view *p = (kiln_runtime_promise_view *)(uintptr_t)value.bits;
        if (p != NULL && p->state == 2 /* KILN_C99_REJECTED */) {
            return kiln_cli_describe_error(p->val);
        }
    }
    if (value.tag == 9 /* KILN_VALUE_OBJECT */ && value.bits != 0) {
        typedef struct kiln_runtime_prop_view {
            const kiln_string *key;
            /* The six one-bit descriptor flags in kiln_property share one unsigned word.
             * Keep the complete stride: an Error may have several own properties, and the
             * launcher needs to find message or stack without calling back into the heap. */
            unsigned int attributes;
            /* The runtime's value/getter+setter union has a two-value stride. */
            kiln_value val;
            kiln_value accessor_second_half;
        } kiln_runtime_prop_view;
        typedef struct kiln_runtime_obj_view {
            kiln_runtime_prop_view *properties;
            size_t count;
        } kiln_runtime_obj_view;
        kiln_runtime_obj_view *obj = (kiln_runtime_obj_view *)(uintptr_t)value.bits;
        if (obj != NULL && obj->properties != NULL) {
            for (size_t i = 0; i < obj->count; ++i) {
                if (obj->properties[i].key != NULL &&
                    obj->properties[i].key->unit_len == 5 &&
                    kiln_string_unit_at(obj->properties[i].key, 0) == 's' &&
                    kiln_string_unit_at(obj->properties[i].key, 1) == 't' &&
                    kiln_string_unit_at(obj->properties[i].key, 2) == 'a' &&
                    kiln_string_unit_at(obj->properties[i].key, 3) == 'c' &&
                    kiln_string_unit_at(obj->properties[i].key, 4) == 'k') {
                    if (obj->properties[i].val.tag == KILN_VALUE_STRING &&
                        obj->properties[i].val.ptr != NULL) {
                        return obj->properties[i].val;
                    }
                }
            }
            for (size_t i = 0; i < obj->count; ++i) {
                if (obj->properties[i].key != NULL &&
                    obj->properties[i].key->unit_len == 7 &&
                    kiln_string_unit_at(obj->properties[i].key, 0) == 'm' &&
                    kiln_string_unit_at(obj->properties[i].key, 1) == 'e' &&
                    kiln_string_unit_at(obj->properties[i].key, 2) == 's' &&
                    kiln_string_unit_at(obj->properties[i].key, 3) == 's' &&
                    kiln_string_unit_at(obj->properties[i].key, 4) == 'a' &&
                    kiln_string_unit_at(obj->properties[i].key, 5) == 'g' &&
                    kiln_string_unit_at(obj->properties[i].key, 6) == 'e') {
                    if (obj->properties[i].val.tag == KILN_VALUE_STRING &&
                        obj->properties[i].val.ptr != NULL) {
                        return obj->properties[i].val;
                    }
                }
            }
        }
    }
    return value;
}


/* Where a realm's `standard` stream goes: its host's output when it was given one. */
static FILE *kiln_cli_sink(const kiln_cli_host_state *state, FILE *standard) {
    if (state != NULL && standard == stderr && state->error_output != NULL) return state->error_output;
    return state != NULL && state->output != NULL ? state->output : standard;
}

/* A line on the realm's stderr from the launcher, which has no stdio of its own. `host_state` is
 * the realm's, or NULL outside one. */
void kiln_cli_warn(const char *message, const kiln_cli_host_state *host_state) {
    FILE *sink = kiln_cli_sink(host_state, stderr);
    fputs(message, sink);
    fputc('\n', sink);
    fflush(sink);
}

/* `host_state` is the failing realm's (`kiln_cli_host_state`), whose last refusal names a host
 * error the value does not, and whose output the report goes to; NULL reads this thread's. */
void kiln_cli_report_failure_with(int status, kiln_value value, const char *phase,
                                  const kiln_cli_host_state *host_state) {
    kiln_value last_host_error =
        (host_state != NULL ? host_state : &kiln_cli_thread_host_state)->last_error;
    FILE *sink = kiln_cli_sink(host_state, stderr);
    value = kiln_cli_describe_error(value);
    value.flags = 0;
    fputs("kiln: ", sink);
    if (status == KILN_C99_THROWN) {
        fputs("uncaught ", sink);
        if (value.tag == KILN_VALUE_STRING && value.len != 0 && value.ptr != NULL) {
            kiln_cli_write_string(sink, (const kiln_string *)value.ptr);
        } else {
            fputs("exception", sink);
        }
        fputc('\n', sink);
        return;
    }
    /* Say what the status *means*, not just what it is. `-2` is by far the most common one a
     * program hits and it is the one that says least: it is the backend reaching something it
     * does not implement yet, which is a backlog entry, but a bare number reads as a crash and
     * sends whoever sees it looking for a memory fault instead of a missing builtin. Naming it is
     * what makes the difference between a refusal you can see and an exit code with no words. */
    switch (status) {
    case KILN_C99_CONTROL_ERROR:
        fputs("native backend refusal", sink);
        if (value.tag == KILN_VALUE_STRING && value.len != 0 && value.ptr != NULL) {
            fputs(": ", sink);
            kiln_cli_write_string(sink, (const kiln_string *)value.ptr);
        } else if (phase != NULL) {
            /* The module body carries its own diagnostic; everything else has none to carry, so
             * the phase is the only thing that can be said -- and saying it beats a bare line. */
            fputs(" in ", sink);
            fputs(phase, sink);
        }
        fputc('\n', sink);
        return;
    case KILN_C99_OUT_OF_MEMORY:
        fputs("out of memory\n", sink);
        return;
    case KILN_C99_CALL_DEPTH_ERROR:
        fputs("call depth exceeded\n", sink);
        return;
    case KILN_C99_UNINITIALIZED_ERROR:
        fputs("a binding was read before it was initialised\n", sink);
        return;
    case KILN_C99_HOST_ERROR:
        fputs("the host refused an operation", sink);
        if (value.tag == KILN_VALUE_STRING && value.len != 0 && value.ptr != NULL) {
            fputs(": ", sink);
            kiln_cli_write_string(sink, (const kiln_string *)value.ptr);
        } else if (last_host_error.tag == KILN_VALUE_STRING && last_host_error.len != 0 && last_host_error.ptr != NULL) {
            fputs(": ", sink);
            kiln_cli_write_string(sink, (const kiln_string *)last_host_error.ptr);
        } else if (phase != NULL) {
            fputs(" in ", sink);
            fputs(phase, sink);
        }
        fputc('\n', sink);
        return;
    case KILN_C99_INVALID_ARGUMENT:
        fputs("an argument was not valid\n", sink);
        return;
    default:
        fprintf(sink, "the program stopped with status %d\n", status);
        return;
    }
}

void kiln_cli_report_failure_in(int status, kiln_value value, const char *phase) {
    kiln_cli_report_failure_with(status, value, phase, NULL);
}

static int kiln_cli_write_value_to(FILE *sink, kiln_value value) {
    static const char undefined_text[] = "undefined";
    static const char null_text[] = "null";
    static const char true_text[] = "true";
    static const char false_text[] = "false";
    /* Console/host output consumes a borrowed span; ownership is an internal optimization
     * and must not escape through this public writer. */
    value.flags = 0;
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
        return kiln_cli_write_bytes(sink, undefined_text, sizeof(undefined_text) - 1);
    case KILN_VALUE_NULL:
        return kiln_cli_write_bytes(sink, null_text, sizeof(null_text) - 1);
    case KILN_VALUE_BOOLEAN:
        return value.bits != 0
            ? kiln_cli_write_bytes(sink, true_text, sizeof(true_text) - 1)
            : kiln_cli_write_bytes(sink, false_text, sizeof(false_text) - 1);
    case KILN_VALUE_STRING:
        return kiln_cli_write_string(sink, (const kiln_string *)value.ptr);
    case KILN_VALUE_NUMBER: {
        double number;
        char buffer[64];
        int length;
        memcpy(&number, &value.bits, sizeof(number));
        if (isnan(number)) {
            return kiln_cli_write_bytes(sink, "NaN", 3);
        }
        if (isinf(number)) {
            return number < 0.0
                ? kiln_cli_write_bytes(sink, "-Infinity", 9)
                : kiln_cli_write_bytes(sink, "Infinity", 8);
        }
        /* Console display preserves negative zero; String(-0) is the separate `0` conversion. */
        if (number == 0.0) {
            return signbit(number)
                ? kiln_cli_write_bytes(sink, "-0", 2)
                : kiln_cli_write_bytes(sink, "0", 1);
        }
        length = kiln_number_to_js_string(number, buffer, sizeof(buffer));
        if (length < 0 || (size_t)length >= sizeof(buffer)) {
            return KILN_C99_HOST_ERROR;
        }
        return kiln_cli_write_bytes(sink, buffer, (size_t)length);
    }
    default:
        return KILN_C99_HOST_ERROR;
    }
}

static kiln_value kiln_cli_undefined(void) {
    kiln_value value;
    value.tag = KILN_VALUE_UNDEFINED;
    value.len = 0;
    value.bits = 0;
    return value;
}

static kiln_value kiln_cli_number(double number) {
    kiln_value value = kiln_cli_undefined();
    value.tag = KILN_VALUE_NUMBER;
    memcpy(&value.bits, &number, sizeof(number));
    return value;
}

static int kiln_cli_clock_now(kiln_value *result) {
    double milliseconds;
#if defined(TIME_UTC)
    struct timespec now;
    if (timespec_get(&now, TIME_UTC) != TIME_UTC) return KILN_C99_HOST_ERROR;
    milliseconds = (double)now.tv_sec * 1000.0 + (double)now.tv_nsec / 1000000.0;
#else
    time_t now = time(NULL);
    if (now == (time_t)-1) return KILN_C99_HOST_ERROR;
    milliseconds = (double)now * 1000.0;
#endif
    *result = kiln_cli_number(milliseconds);
    return KILN_C99_OK;
}

static int kiln_cli_entropy_u64(kiln_value *result) {
    uint64_t word = 0;
#if defined(_WIN32)
    if (BCryptGenRandom(NULL, (PUCHAR)&word, (ULONG)sizeof(word),
                        BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) {
        return KILN_C99_HOST_ERROR;
    }
#else
    FILE *random = fopen("/dev/urandom", "rb");
    if (random == NULL) return KILN_C99_HOST_ERROR;
    if (fread(&word, 1, sizeof(word), random) != sizeof(word) || fclose(random) != 0) {
        return KILN_C99_HOST_ERROR;
    }
#endif
    *result = kiln_cli_undefined();
    result->tag = KILN_VALUE_NUMBER;
    result->bits = word;
    return KILN_C99_OK;
}

/* Stable CLI implementations of the explicit synchronous HostOps boundary. */
int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                       const kiln_value *arguments, kiln_value *result) {
    uint16_t index;
    FILE *sink;
    kiln_cli_host_state *state = kiln_cli_state_of(context);
    if (result == NULL || (argc != 0 && arguments == NULL)) {
        {
            kiln_value refused = kiln_cli_refused(state, "host capability", 15);
            if (result != NULL) *result = refused;
        }
        return KILN_C99_HOST_ERROR;
    }
    if (operation == KILN_HOST_OP_CLOCK_NOW) {
        if (argc != 0) {
            *result = kiln_cli_refused(state, "clock time", 10);
            return KILN_C99_HOST_ERROR;
        }
        int status = kiln_cli_clock_now(result);
        if (status != KILN_C99_OK) {
            *result = kiln_cli_refused(state, "clock time", 10);
        }
        return status;
    }
    if (operation == KILN_HOST_OP_ENTROPY_U64) {
        if (argc != 0) {
            *result = kiln_cli_refused(state, "entropy", 7);
            return KILN_C99_HOST_ERROR;
        }
        int status = kiln_cli_entropy_u64(result);
        if (status != KILN_C99_OK) {
            *result = kiln_cli_refused(state, "entropy", 7);
        }
        return status;
    }
    if (operation != KILN_HOST_OP_WRITE_STDOUT && operation != KILN_HOST_OP_WRITE_STDERR) {
        *result = kiln_cli_refused(state, "host capability", 15);
        return KILN_C99_HOST_ERROR;
    }
    /* Which stream a line went to is observable -- a caller that redirects one and not the other
     * sees the difference -- so `console.error` and `console.warn` reach the same place here that
     * they reach in node. */
    sink = kiln_cli_sink(state, operation == KILN_HOST_OP_WRITE_STDERR ? stderr : stdout);
    /* One line is one unit on a stream several realms share (`KILN_WORKERS` thread mode): the
     * stream stays locked from its first byte to its flush, so another realm's line never lands
     * inside it. */
#ifndef _WIN32
    extern void flockfile(FILE *file);
    extern void funlockfile(FILE *file);
    flockfile(sink);
#endif
    int status = KILN_C99_OK;
    for (index = 0; index < argc && status == KILN_C99_OK; ++index) {
        if (index != 0 && fputc(' ', sink) == EOF) status = KILN_C99_HOST_ERROR;
        else if (kiln_cli_write_value_to(sink, arguments[index]) != KILN_C99_OK)
            status = KILN_C99_HOST_ERROR;
    }
    if (status == KILN_C99_OK && (fputc('\n', sink) == EOF || fflush(sink) != 0))
        status = KILN_C99_HOST_ERROR;
#ifndef _WIN32
    funlockfile(sink);
#endif
    if (status != KILN_C99_OK) {
        *result = kiln_cli_refused(state, "terminal I/O", 12);
        return KILN_C99_HOST_ERROR;
    }
    *result = kiln_cli_undefined();
    return KILN_C99_OK;
}
"#;

const LAUNCHER_PREFIX: &str = r#"/* Kiln native CLI launcher. */
#include <locale.h>

"#;

const LAUNCHER_DECL_PREFIX: &str = r#"
extern int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                              const kiln_value *arguments, kiln_value *result);
extern int kiln_cli_write_result(kiln_value value);
extern void kiln_cli_report_failure_with(int status, kiln_value value, const char *phase,
                                         const kiln_cli_host_state *host_state);
extern kiln_value kiln_cli_describe_error(kiln_value value);
typedef int (*kiln_realm_entry_fn)(void *context);
extern int kiln_realm_create(uint64_t *out_realm);
extern int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context);
extern int kiln_realm_set_argv(uint64_t realm, uint64_t argc, const char *const *argv);
extern int kiln_realm_drive(uint64_t realm, void *host, void *thrown);
extern int kiln_realm_destroy(uint64_t realm);
/* A string value over static text, read through `header`, which lives as long as the realm run. */
static kiln_value kiln_cli_launcher_ascii(kiln_string *header, const char *text, size_t length) {
    kiln_value value = { .tag = KILN_VALUE_STRING, .flags = 0, .len = 0, .bits = 0 };
    header->unit_len = length > UINT32_MAX ? 0 : (uint32_t)length;
    header->flags = KILN_STRING_LATIN1;
    header->data = text;
    value.ptr = (const char *)header;
    value.len = header->unit_len;
    return value;
}
extern int 
"#;

const LAUNCHER_DECL_SUFFIX: &str = r#"(kiln_host *host, kiln_value *result);

typedef struct kiln_cli_entry_context {
    kiln_host *host;
    kiln_value *result;
} kiln_cli_entry_context;

static int kiln_cli_enter_module(void *opaque) {
    kiln_cli_entry_context *context = (kiln_cli_entry_context *)opaque;
    if (context == NULL) return KILN_C99_INVALID_ARGUMENT;
    return
"#;

const LAUNCHER_BODY: &str = r#"(context->host, context->result);
}

/* argc/argv reach the realm before it is entered, so the first `process` the module builds
 * already has the command line. The strings are copied on the other side of this call. */
int main(int argc, char **argv) {
    kiln_host host;
    kiln_value result;
    kiln_cli_entry_context context;
    /* This realm's host state and the header its launcher messages are read through: the
     * realm's, not the process's (ADR 0070). */
    kiln_cli_host_state host_state = { { .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }, { 0, 0, NULL }, NULL, NULL };
    kiln_string message = { 0, 0, NULL };
    uint64_t realm = 0;
    const char *phase;
    int status;
    int destroy_status;
    if (setlocale(LC_NUMERIC, "C") == NULL) {
        return 1;
    }
    host.call = kiln_cli_host_call;
    host.context = &host_state;
    result = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
    context.host = &host;
    context.result = &result;
    /* Which call failed, so a failure outside the module body still names something.
     *
     * `result` carries the diagnostic only when the module body is what refused. A refusal from
     * the realm's own machinery -- entering it, driving its turns, tearing it down -- left it
     * untouched, so the report printed `native backend refusal` and stopped: a refusal that names
     * nothing, which is the one thing a refusal is supposed never to be. A bundled `rxjs` fails
     * exactly here and said precisely that. */
    phase = "realm creation";
    status = kiln_realm_create(&realm);
    if (status == KILN_C99_OK && argc > 0) {
        phase = "argv installation";
        status = kiln_realm_set_argv(realm, (uint64_t)argc, (const char *const *)argv);
    }
    if (status == KILN_C99_OK) {
        phase = "module body";
        status = kiln_realm_enter(realm, kiln_cli_enter_module, &context);
    }
    if (status == KILN_C99_OK) {
        phase = "the turn loop";
        status = kiln_realm_drive(realm, &host, &result);
    }
    if (status == KILN_C99_OK && result.tag == 16 /* KILN_VALUE_PROMISE */) {
        typedef struct kiln_launcher_promise_view {
            int state;
            kiln_value value;
        } kiln_launcher_promise_view;
        kiln_launcher_promise_view *promise = (kiln_launcher_promise_view *)(uintptr_t)result.bits;
        if (promise != NULL) {
            if (promise->state == 2 /* KILN_C99_REJECTED */) {
                status = KILN_C99_THROWN;
                phase = "module body";
                result = kiln_cli_describe_error(promise->value);
            } else if (promise->state == 0 /* KILN_C99_PENDING */) {
                status = KILN_C99_CONTROL_ERROR;
                phase = "the turn loop";
                result = kiln_cli_launcher_ascii(&message, "top-level await never settled", 29);
            } else if (promise->state == 1 /* KILN_C99_FULFILLED */) {
                result = promise->value;
            }
        }
    }
    if (realm != 0) {
        destroy_status = kiln_realm_destroy(realm);
        if (status == KILN_C99_OK) {
            phase = "realm teardown";
            status = destroy_status;
        }
    }
    if (status != 0) {
        kiln_cli_report_failure_with(status, result, phase, &host_state);
        return 1;
    }
    if (kiln_cli_write_result(result) != KILN_C99_OK) {
        kiln_cli_report_failure_with(KILN_C99_HOST_ERROR, kiln_cli_launcher_ascii(&message, "filesystem access", 17), "result write", &host_state);
        return 1;
    }
    return 0;
}
"#;

// The two source gaps above put the validated symbol in its declaration and in the callback that
// Tokio enters. No generated module or Tokio type crosses the private Realm ABI.
const _: () = {
    // Compile-time assertion that the source fragments remain nonempty and deterministic.
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn runtime_is_deterministic_and_does_not_name_a_module() {
        assert_eq!(runtime_source(), runtime_source());
        assert!(runtime_source().contains("int kiln_cli_host_call"));
        assert!(runtime_source().contains("kiln_number_to_js_string"));
        assert!(runtime_source().contains("kiln_string_to_number"));
        assert!(
            runtime_source().contains("kiln_number_to_js_string(number, buffer, sizeof(buffer))")
        );
        assert!(runtime_source().contains("Private number-provider ABI"));
        assert!(!runtime_source().contains("Copyright 2024 Tiago Carvalho"));
        assert!(!runtime_source().contains("%.17g"));
        assert!(!runtime_source().contains("kiln_module_deadbeef"));
    }

    #[test]
    fn runtime_identity_tracks_native_launcher_changes() {
        assert_eq!(NATIVE_RUNTIME_VERSION, 14);
        assert!(embedding_header().contains(&format!(
            "#define KILN_NATIVE_RUNTIME_VERSION {NATIVE_RUNTIME_VERSION}"
        )));
    }

    #[test]
    fn native_result_round_trips_all_scalar_tags_and_display() {
        let values = [
            kiln_host_abi::HostValue::Undefined,
            kiln_host_abi::HostValue::Null,
            kiln_host_abi::HostValue::Boolean(false),
            kiln_host_abi::HostValue::Boolean(true),
            kiln_host_abi::HostValue::Number((-0.0_f64).to_bits()),
            kiln_host_abi::HostValue::Number(f64::NAN.to_bits()),
            kiln_host_abi::HostValue::String("a\0b".to_owned().into()),
        ];
        let displays = ["undefined", "null", "false", "true", "0", "NaN", "a\0b"];
        for (value, display) in values.iter().zip(displays) {
            assert_eq!(
                decode_native_result(&encode_native_result(value)),
                Ok(value.clone())
            );
            assert_eq!(value.to_js_string(), display);
        }
    }

    #[test]
    fn native_result_rejects_malformed_records() {
        let fixture = encode_native_result(&kiln_host_abi::HostValue::Boolean(true));

        let mut bad_magic = fixture.clone();
        bad_magic[0] = b'X';
        assert_eq!(
            decode_native_result(&bad_magic),
            Err(NativeResultError::InvalidMagic)
        );

        let mut unknown_tag = fixture.clone();
        unknown_tag[NATIVE_RESULT_MAGIC.len()] = 99;
        assert_eq!(
            decode_native_result(&unknown_tag),
            Err(NativeResultError::UnknownTag(99))
        );

        let mut invalid_length = fixture.clone();
        invalid_length[NATIVE_RESULT_MAGIC.len() + 1] = 2;
        assert!(matches!(
            decode_native_result(&invalid_length),
            Err(NativeResultError::InvalidLength {
                tag: 2,
                expected: 1,
                actual: 2
            })
        ));

        let mut invalid_boolean = fixture.clone();
        invalid_boolean[NATIVE_RESULT_HEADER_LEN] = 2;
        assert_eq!(
            decode_native_result(&invalid_boolean),
            Err(NativeResultError::InvalidBoolean(2))
        );

        assert!(matches!(
            decode_native_result(&fixture[..fixture.len() - 1]),
            Err(NativeResultError::Truncated { .. })
        ));

        let mut trailing = fixture;
        trailing.push(0);
        assert_eq!(
            decode_native_result(&trailing),
            Err(NativeResultError::TrailingBytes(1))
        );

        let mut invalid_utf8 =
            encode_native_result(&kiln_host_abi::HostValue::String("ok".to_owned().into()));

        invalid_utf8.truncate(invalid_utf8.len() - 1);
        invalid_utf8[9..17].copy_from_slice(&3_u64.to_le_bytes());

        assert_eq!(
            decode_native_result(&invalid_utf8),
            Err(NativeResultError::InvalidStringPayload)
        );

        let mut huge_string = Vec::from(NATIVE_RESULT_MAGIC);
        huge_string.push(4);
        huge_string.extend_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(
            decode_native_result(&huge_string),
            Err(NativeResultError::LengthOverflow(u64::MAX))
        );
    }

    #[test]
    fn embedding_header_is_deterministic_and_contains_the_stable_contract() {
        assert_eq!(embedding_header(), embedding_header());
        assert_eq!(
            embedding_header()
                .matches("typedef struct kiln_value")
                .count(),
            1,
            "the boundary value declaration must have one owner"
        );
        assert!(embedding_header().contains("#ifndef KILN_NATIVE_EMBEDDING_H"));
        assert!(embedding_header().contains(&format!(
            "#define KILN_NATIVE_RUNTIME_VERSION {NATIVE_RUNTIME_VERSION}"
        )));
        assert!(embedding_header().contains(&format!(
            "#define KILN_C_ABI_VERSION {EMBEDDING_C_ABI_VERSION}"
        )));
        assert!(embedding_header().contains(&format!(
            "#define KILN_HOST_ABI_VERSION {EMBEDDING_HOST_ABI_VERSION}"
        )));
        assert!(embedding_header().contains(&format!(
            "#define KILN_MODULE_API_VERSION {EMBEDDING_MODULE_API_VERSION}"
        )));
        assert!(embedding_header().contains("#define KILN_C99_OUT_OF_MEMORY (-4)"));
        assert!(embedding_header().contains("#define KILN_C99_CALL_DEPTH_ERROR (-5)"));
        assert!(embedding_header().contains("#define KILN_C99_UNINITIALIZED_ERROR (-6)"));
        assert!(embedding_header().contains("#define KILN_HOST_OP_WRITE_STDOUT 1"));
        assert!(embedding_header().contains("uint16_t tag;"));
        assert!(embedding_header().contains("uint16_t flags;"));
        assert!(embedding_header().contains("#define KILN_VALUE_FLAG_OWNED_STRING UINT16_C(1)"));
        assert!(embedding_header().contains("union { uint64_t bits; const char *ptr; };"));
        assert!(embedding_header().contains("uint64_t bits;"));
        assert!(embedding_header().contains("const char *ptr;"));
        assert!(embedding_header().contains("uint32_t len;"));
        assert!(embedding_header().contains("typedef int (*kiln_host_call_fn)"));
        assert!(embedding_header().contains("typedef struct kiln_host"));
        assert!(embedding_header().contains("typedef struct kiln_result_v2 kiln_result_v2;"));
        assert!(embedding_header().contains("typedef struct kiln_module_api_v2"));
        assert!(
            embedding_header().contains("int (*run)(kiln_host *host, kiln_result_v2 **result);")
        );
        assert!(
            embedding_header()
                .contains("int (*read)(const kiln_result_v2 *result, kiln_value *value);")
        );
        assert!(embedding_header().contains("void (*dispose)(kiln_result_v2 **result);"));
        assert!(embedding_header().contains("sizeof(kiln_value) == 16"));
        assert!(embedding_header().contains("#endif /* KILN_NATIVE_EMBEDDING_H */"));
    }

    #[test]
    fn embedding_header_compiles_as_c_and_cxx() {
        let Some(cc) = find_cc() else {
            return;
        };
        let Some(cxx) = find_cxx() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let header = root.join("kiln_embedding.h");
        let c_source = root.join("probe.c");
        let cxx_source = root.join("probe.cc");
        std::fs::write(&header, embedding_header()).expect("write header");
        let probe = r#"
#include "kiln_embedding.h"
static int call(void *context, uint16_t operation, uint16_t argc,
                const kiln_value *arguments, kiln_value *result) {
    (void)context;
    (void)operation;
    (void)argc;
    (void)arguments;
    (void)result;
    return KILN_C99_OK;
}
int main(void) {
    kiln_host host;
    kiln_value value;
    host.call = call;
    host.context = 0;
    value.tag = KILN_VALUE_UNDEFINED;
    value.len = 0;
    value.bits = 0;
    return host.call(host.context, KILN_HOST_OP_WRITE_STDOUT, 0, 0, &value);
}
"#;
        std::fs::write(&c_source, probe).expect("write C probe");
        std::fs::write(&cxx_source, probe).expect("write C++ probe");

        for (compiler, standard, source, object) in [
            (cc, "-std=c11", &c_source, root.join("probe-c.o")),
            (cxx, "-std=c++11", &cxx_source, root.join("probe-cxx.o")),
        ] {
            let compile = Command::new(compiler)
                .args([standard, "-pedantic", "-Wall", "-Wextra", "-Werror"])
                .arg("-c")
                .arg(source)
                .arg("-o")
                .arg(&object)
                .output()
                .expect("invoke C compiler");
            assert!(
                compile.status.success(),
                "embedding header failed to compile with {compiler}:\n{}",
                String::from_utf8_lossy(&compile.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn embedding_module_header_is_deterministic_and_symbol_guarded() {
        let symbol = "kiln_module_deadbeef";
        let one = embedding_module_header(symbol).expect("valid symbol");
        assert_eq!(one, embedding_module_header(symbol).unwrap());
        assert!(one.starts_with(embedding_header()));
        assert!(one.contains("#ifndef KILN_NATIVE_MODULE_kiln_module_deadbeef_H"));
        assert!(one.contains("#define KILN_NATIVE_MODULE_kiln_module_deadbeef_H"));
        assert!(one.contains("extern \"C\" {"));
        assert!(
            one.contains("extern int kiln_module_deadbeef(kiln_host *host, kiln_value *result);")
        );
        assert!(
            one.contains("extern const kiln_module_api_v2 *kiln_module_deadbeef_api_v2(void);")
        );
        assert!(one.contains("#endif /* KILN_NATIVE_MODULE_kiln_module_deadbeef_H */"));
    }

    #[test]
    fn embedding_module_api_symbol_is_deterministic_and_validated() {
        assert_eq!(
            embedding_module_api_symbol("kiln_module_deadbeef").unwrap(),
            "kiln_module_deadbeef_api_v2"
        );
        for symbol in ["", "9module", "module-name", "module name", "méthode"] {
            assert!(matches!(
                embedding_module_api_symbol(symbol),
                Err(RuntimeSourceError::InvalidSymbol { .. })
            ));
        }
    }

    #[test]
    fn embedding_module_header_rejects_invalid_symbols() {
        for symbol in ["", "9module", "module-name", "module name", "méthode"] {
            assert!(matches!(
                embedding_module_header(symbol),
                Err(RuntimeSourceError::InvalidSymbol { .. })
            ));
        }
    }

    #[test]
    fn embedding_module_header_compiles_as_c_and_cxx() {
        let Some(cc) = find_cc() else {
            return;
        };
        let Some(cxx) = find_cxx() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let header = root.join("kiln_module.h");
        let c_source = root.join("probe.c");
        let cxx_source = root.join("probe.cc");
        let module_header = embedding_module_header("kiln_module_probe").expect("valid symbol");
        std::fs::write(&header, module_header).expect("write module header");
        let probe = r#"
#include "kiln_module.h"
#include "kiln_module.h"

int main(void) {
    kiln_host host = {0, 0, 0};
    kiln_value result;
    result.tag = 0;
    result.flags = 0;
    result.len = 0;
    result.bits = 0;
    const kiln_module_api_v2 *(*api)(void) = kiln_module_probe_api_v2;
    (void)api;
    return kiln_module_probe(&host, &result);
}
"#;
        std::fs::write(&c_source, probe).expect("write C probe");
        std::fs::write(&cxx_source, probe).expect("write C++ probe");

        for (compiler, standard, source, object) in [
            (cc, "-std=c11", &c_source, root.join("probe-c.o")),
            (cxx, "-std=c++11", &cxx_source, root.join("probe-cxx.o")),
        ] {
            let compile = Command::new(compiler)
                .args([standard, "-pedantic", "-Wall", "-Wextra", "-Werror"])
                .arg("-c")
                .arg(source)
                .arg("-o")
                .arg(&object)
                .output()
                .expect("invoke C compiler");
            assert!(
                compile.status.success(),
                "module embedding header failed to compile with {compiler}:\n{}",
                String::from_utf8_lossy(&compile.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn launcher_validates_symbols_and_is_deterministic() {
        let one = launcher_source("kiln_module_deadbeef").expect("valid symbol");
        assert_eq!(one, launcher_source("kiln_module_deadbeef").unwrap());
        assert!(one.contains("kiln_module_deadbeef"));
        for symbol in ["", "9module", "module-name", "module name", "méthode"] {
            assert!(matches!(
                launcher_source(symbol),
                Err(RuntimeSourceError::InvalidSymbol { .. })
            ));
        }
    }

    #[test]
    fn launcher_pins_numeric_locale_before_module_call() {
        let source = launcher_source("kiln_module_locale").expect("valid symbol");
        let locale = source
            .find("setlocale(LC_NUMERIC, \"C\")")
            .expect("launcher sets C numeric locale");
        let realm_enter = source
            .find("kiln_realm_enter(realm, kiln_cli_enter_module, &context)")
            .expect("launcher enters realm");
        assert!(locale < realm_enter);
        assert!(source.contains("#include <locale.h>"));
        assert!(source.contains("return\nkiln_module_locale"));
        assert!(source.contains("kiln_realm_create(&realm)"));
        assert!(source.contains("kiln_realm_enter(realm, kiln_cli_enter_module, &context)"));
        assert!(source.contains("kiln_realm_drive(realm, &host, &result)"));
        assert!(source.contains("kiln_realm_destroy(realm)"));
        assert!(source.contains("return 1;"));
    }

    /// The command line has to be on the realm before anything in the program can observe it:
    /// `process` is built lazily by whichever module mentions it first, and that can be the very
    /// first statement the entry module runs. Setting argv after `kiln_realm_enter` would leave
    /// that first `process.argv` empty and every later one correct, which is the worst of both.
    #[test]
    fn launcher_hands_its_command_line_to_the_realm_before_entering_it() {
        let source = launcher_source("kiln_module_argv").expect("valid symbol");
        assert!(source.contains("int main(int argc, char **argv)"));
        let set_argv = source
            .find("kiln_realm_set_argv(realm, (uint64_t)argc, (const char *const *)argv)")
            .expect("launcher hands its command line to the realm");
        let create = source
            .find("kiln_realm_create(&realm)")
            .expect("launcher creates a realm");
        let realm_enter = source
            .find("kiln_realm_enter(realm, kiln_cli_enter_module, &context)")
            .expect("launcher enters realm");
        assert!(create < set_argv);
        assert!(set_argv < realm_enter);
    }

    #[test]
    fn runtime_and_launcher_link_against_stub_module() {
        let Some(cc) = find_cc() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let runtime = root.join("runtime.c");
        let launcher = root.join("launcher.c");
        let header = root.join("kiln_host.h");
        let stub = root.join("stub.c");
        let number_provider = root.join("number-provider.c");
        let executable = root.join("kiln-test");
        let result_path = root.join("result.bin");
        std::fs::write(&runtime, runtime_source()).expect("write runtime");
        std::fs::write(
            &launcher,
            launcher_source("kiln_module_test").expect("valid symbol"),
        )
        .expect("write launcher");
        std::fs::write(&header, embedding_header()).expect("write header");
        std::fs::write(&stub, STUB_SOURCE).expect("write stub");
        std::fs::write(
            &number_provider,
            kiln_native_provider_contract::number::runtime_source(),
        )
        .expect("write test number provider");
        let compile = Command::new(cc)
            .args([
                "-std=c11",
                "-pedantic",
                "-Wall",
                "-Wextra",
                "-Werror",
                // Ryu's portable amalgamation retains target-specific static inline helpers.
                "-Wno-unused-function",
            ])
            .args([
                runtime.as_os_str(),
                launcher.as_os_str(),
                stub.as_os_str(),
                number_provider.as_os_str(),
            ])
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("invoke C compiler");
        assert!(
            compile.status.success(),
            "C runtime failed to compile/link:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let compatibility_run = Command::new(&executable)
            .output()
            .expect("run linked launcher without sidecar");
        assert!(compatibility_run.status.success());
        assert!(compatibility_run.stderr.is_empty());
        assert!(!result_path.exists());

        let run = Command::new(&executable)
            .env(NATIVE_RESULT_PATH_ENV, &result_path)
            .output()
            .expect("run linked launcher");
        assert!(run.status.success());
        assert!(run.stderr.is_empty());
        assert_eq!(
            String::from_utf8(run.stdout).expect("UTF-8 stdout"),
            "undefined null true false -0 NaN Infinity -Infinity 100000000000000000000 1e+21 1e-7 hello\n"
        );
        assert_eq!(
            decode_native_result(&std::fs::read(&result_path).expect("read sidecar")),
            Ok(kiln_host_abi::HostValue::Undefined)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn c_result_writer_emits_every_scalar_tag() {
        let Some(cc) = find_cc() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let runtime = root.join("runtime.c");
        let harness = root.join("harness.c");
        let header = root.join("kiln_host.h");
        let number_provider = root.join("number-provider.c");
        let executable = root.join("kiln-result-writer-test");
        std::fs::write(&runtime, runtime_source()).expect("write runtime");
        std::fs::write(&harness, RESULT_WRITER_SOURCE).expect("write result writer harness");
        std::fs::write(&header, embedding_header()).expect("write header");
        std::fs::write(
            &number_provider,
            kiln_native_provider_contract::number::runtime_source(),
        )
        .expect("write test number provider");
        let compile = Command::new(cc)
            .args([
                "-std=c11",
                "-pedantic",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-function",
            ])
            .args([
                runtime.as_os_str(),
                harness.as_os_str(),
                number_provider.as_os_str(),
            ])
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("invoke C compiler");
        assert!(
            compile.status.success(),
            "C result writer failed to compile/link:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );

        let paths: Vec<_> = ["undefined", "null", "boolean", "number", "string"]
            .iter()
            .map(|name| root.join(format!("{name}.bin")))
            .collect();
        let run = Command::new(&executable)
            .args(&paths)
            .output()
            .expect("run result writer harness");
        assert!(run.status.success());
        assert!(run.stdout.is_empty());
        assert!(run.stderr.is_empty());
        let expected = [
            kiln_host_abi::HostValue::Undefined,
            kiln_host_abi::HostValue::Null,
            kiln_host_abi::HostValue::Boolean(true),
            kiln_host_abi::HostValue::Number((-0.0_f64).to_bits()),
            kiln_host_abi::HostValue::String("a\0b".to_owned().into()),
        ];
        for (path, value) in paths.iter().zip(expected.iter()) {
            assert_eq!(
                decode_native_result(&std::fs::read(path).expect("read sidecar")),
                Ok(value.clone())
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn c_failure_reporter_explains_known_statuses_and_preserves_fallback() {
        let Some(cc) = find_cc() else {
            return;
        };
        let root = test_directory();
        std::fs::create_dir(&root).expect("create test directory");
        let runtime = root.join("runtime.c");
        let harness = root.join("harness.c");
        let header = root.join("kiln_host.h");
        let number_provider = root.join("number-provider.c");
        let executable = root.join("kiln-failure-reporter-test");
        std::fs::write(&runtime, runtime_source()).expect("write runtime");
        std::fs::write(&harness, FAILURE_REPORTER_SOURCE).expect("write failure reporter harness");
        std::fs::write(&header, embedding_header()).expect("write header");
        std::fs::write(
            &number_provider,
            kiln_native_provider_contract::number::runtime_source(),
        )
        .expect("write test number provider");
        let compile = Command::new(cc)
            .args([
                "-std=c11",
                "-pedantic",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-Wno-unused-function",
            ])
            .args([
                runtime.as_os_str(),
                harness.as_os_str(),
                number_provider.as_os_str(),
            ])
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("invoke C compiler");
        assert!(
            compile.status.success(),
            "C failure reporter failed to compile/link:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );

        let run = Command::new(&executable)
            .output()
            .expect("run failure reporter harness");
        assert!(run.status.success());
        assert!(run.stdout.is_empty());
        assert_eq!(
            String::from_utf8(run.stderr).expect("UTF-8 stderr"),
            // Every line carries the `kiln: ` prefix, because this stream also carries the
            // program's own `console.error` output now and the prefix is what tells the two
            // apart. It is written where the words are, not by prefixing the whole stream.
            "kiln: native backend refusal\n".to_owned()
                + "kiln: native backend refusal: module=node:http instruction=7 method=240 receiver_tag=18\n"
                + "kiln: out of memory\n"
                + "kiln: call depth exceeded\n"
                + "kiln: a binding was read before it was initialised\n"
                + "kiln: the host refused an operation\n"
                + "kiln: the host refused an operation: filesystem access\n"
                + "kiln: the host refused an operation: terminal I/O\n"
                + "kiln: the host refused an operation in the turn loop\n"
                + "kiln: an argument was not valid\n"
                + "kiln: the program stopped with status -99\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn find_cc() -> Option<&'static str> {
        find_compiler("cc")
    }

    fn find_cxx() -> Option<&'static str> {
        find_compiler("c++")
    }

    fn find_compiler(name: &'static str) -> Option<&'static str> {
        let result = Command::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        result.success().then_some(name)
    }

    fn test_directory() -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "kiln-native-runtime-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos(),
            TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        path
    }

    const STUB_SOURCE: &str = r#"
#include <string.h>
#include "kiln_host.h"

typedef int (*kiln_realm_entry_fn)(void *context);

int kiln_realm_create(uint64_t *out_realm) {
    if (out_realm == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_realm = 1;
    return KILN_C99_OK;
}

int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context) {
    if (realm != 1 || entry == NULL) return KILN_C99_INVALID_ARGUMENT;
    return entry(context);
}

/* The stub records nothing: this translation unit exists to prove the launcher links and
 * writes its result, and no module here reads a command line. */
int kiln_realm_set_argv(uint64_t realm, uint64_t argc, const char *const *argv) {
    (void)argc;
    (void)argv;
    return realm == 1 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
}

int kiln_realm_drive(uint64_t realm, void *host, void *thrown) {
    (void)host;
    (void)thrown;
    return realm == 1 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
}

int kiln_realm_destroy(uint64_t realm) {
    return realm == 1 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
}

int kiln_module_test(kiln_host *host, kiln_value *result) {
    static const char text[] = "hello";
    kiln_value arguments[12];
    double negative_zero = -0.0;
    double nan = 0.0 / 0.0;
    double infinity = 1.0 / 0.0;
    double negative_infinity = -1.0 / 0.0;
    double decimal_threshold = 1e20;
    double exponent_threshold = 1e21;
    double tiny = 1e-7;
    memset(arguments, 0, sizeof(arguments));
    arguments[0].tag = KILN_VALUE_UNDEFINED;
    arguments[1].tag = KILN_VALUE_NULL;
    arguments[2].tag = KILN_VALUE_BOOLEAN;
    arguments[2].bits = 1;
    arguments[3].tag = KILN_VALUE_BOOLEAN;
    arguments[4].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[4].bits, &negative_zero, sizeof(negative_zero));
    arguments[5].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[5].bits, &nan, sizeof(nan));
    arguments[6].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[6].bits, &infinity, sizeof(infinity));
    arguments[7].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[7].bits, &negative_infinity, sizeof(negative_infinity));
    arguments[8].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[8].bits, &decimal_threshold, sizeof(decimal_threshold));
    arguments[9].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[9].bits, &exponent_threshold, sizeof(exponent_threshold));
    arguments[10].tag = KILN_VALUE_NUMBER;
    memcpy(&arguments[10].bits, &tiny, sizeof(tiny));
    static kiln_string text_header;
    text_header.unit_len = sizeof(text) - 1;
    text_header.flags = KILN_STRING_LATIN1;
    text_header.data = text;
    arguments[11].tag = KILN_VALUE_STRING;
    arguments[11].ptr = (const char *)&text_header;
    arguments[11].len = sizeof(text) - 1;
    return host->call(host->context, 1, 12, arguments, result);
}
"#;

    const RESULT_WRITER_SOURCE: &str = r#"
#include <string.h>
#include "kiln_host.h"

extern int kiln_cli_write_result_file(const char *path, kiln_value value);

int main(int argc, char **argv) {
    static const char text[] = {'a', '\0', 'b'};
    double negative_zero = -0.0;
    kiln_value value;
    if (argc != 6) {
        return 1;
    }
    memset(&value, 0, sizeof(value));
    value.tag = KILN_VALUE_UNDEFINED;
    if (kiln_cli_write_result_file(argv[1], value) != KILN_C99_OK) {
        return 1;
    }
    value.tag = KILN_VALUE_NULL;
    if (kiln_cli_write_result_file(argv[2], value) != KILN_C99_OK) {
        return 1;
    }
    value.tag = KILN_VALUE_BOOLEAN;
    value.bits = 1;
    if (kiln_cli_write_result_file(argv[3], value) != KILN_C99_OK) {
        return 1;
    }
    value.tag = KILN_VALUE_NUMBER;
    memcpy(&value.bits, &negative_zero, sizeof(negative_zero));
    if (kiln_cli_write_result_file(argv[4], value) != KILN_C99_OK) {
        return 1;
    }
    value.tag = KILN_VALUE_STRING;
    /* This is the internal owned-string provenance bit.  The public result writer must
     * consume the ordinary ptr/len span without serializing that provenance. */
    value.flags = KILN_VALUE_FLAG_OWNED_STRING;
    static const kiln_string string = {3, KILN_STRING_LATIN1, text};
    value.ptr = (const char *)&string;
    value.len = string.unit_len;
    return kiln_cli_write_result_file(argv[5], value) == KILN_C99_OK ? 0 : 1;
}
"#;

    const FAILURE_REPORTER_SOURCE: &str = r#"
#include <string.h>
#include "kiln_host.h"

extern void kiln_cli_report_failure_in(int status, kiln_value value, const char *phase);

static kiln_value ascii_val(const char *text) {
    static kiln_string h[3];
    static size_t idx = 0;
    kiln_string *header = &h[idx++];
    header->unit_len = (uint32_t)strlen(text);
    header->flags = KILN_STRING_LATIN1;
    header->data = text;
    kiln_value val = { .tag = KILN_VALUE_STRING, .flags = KILN_VALUE_FLAG_OWNED_STRING, .len = header->unit_len, .bits = 0 };
    val.ptr = (const char *)header;
    return val;
}

int main(void) {
    kiln_value value = { .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
    kiln_cli_report_failure_in(KILN_C99_CONTROL_ERROR, value, NULL);
    value = ascii_val("module=node:http instruction=7 method=240 receiver_tag=18");
    kiln_cli_report_failure_in(KILN_C99_CONTROL_ERROR, value, NULL);
    value = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
    kiln_cli_report_failure_in(KILN_C99_OUT_OF_MEMORY, value, NULL);
    kiln_cli_report_failure_in(KILN_C99_CALL_DEPTH_ERROR, value, NULL);
    kiln_cli_report_failure_in(KILN_C99_UNINITIALIZED_ERROR, value, NULL);
    kiln_cli_report_failure_in(KILN_C99_HOST_ERROR, value, NULL);
    value = ascii_val("filesystem access");
    kiln_cli_report_failure_in(KILN_C99_HOST_ERROR, value, NULL);
    value = ascii_val("terminal I/O");
    kiln_cli_report_failure_in(KILN_C99_HOST_ERROR, value, NULL);
    value = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
    kiln_cli_report_failure_in(KILN_C99_HOST_ERROR, value, "the turn loop");
    kiln_cli_report_failure_in(KILN_C99_INVALID_ARGUMENT, value, NULL);
    kiln_cli_report_failure_in(-99, value, NULL);
    return 0;
}
"#;
}
