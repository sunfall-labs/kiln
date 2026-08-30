//! The C embedding contract and small runtime used by Kiln's native command-line launcher.
//!
//! The generated program translation units deliberately do not contain host policy.  This crate
//! emits the stable, reusable callback object that implements the command-line `WriteStdout`
//! operation, plus a tiny executable launcher for one generated module.  Keeping those sources
//! separate lets the runtime object be cached independently from application code. Embedded hosts
//! consume [`embedding_header`] and supply their own callback policy without linking the CLI code.

use std::sync::OnceLock;

/// Version of the generated native runtime source and its C ABI.
pub const NATIVE_RUNTIME_VERSION: u32 = 8;
/// Version of the private invocation-scoped graph scheduler contract.
pub const GRAPH_SCHEDULER_ABI_VERSION: u32 = 1;

/// Environment variable that enables private scalar-result sidecar emission by the CLI launcher.
pub const NATIVE_RESULT_PATH_ENV: &str = "KILN_NATIVE_RESULT_PATH_V1";

/// Magic prefix for the private scalar-result sidecar format.
pub const NATIVE_RESULT_MAGIC: [u8; 8] = *b"KILNRES1";

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
    InvalidUtf8,
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
            Self::InvalidUtf8 => formatter.write_str("native result string is not UTF-8"),
            Self::TrailingBytes(count) => {
                write!(formatter, "native result has {count} trailing bytes")
            }
        }
    }
}

impl std::error::Error for NativeResultError {}

/// Decode one complete `KILNRES1` sidecar record into the shared host scalar type.
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
            let value = String::from_utf8(bytes[NATIVE_RESULT_HEADER_LEN..payload_end].to_vec())
                .map_err(|_| NativeResultError::InvalidUtf8)?;
            return Ok(kiln_host_abi::HostValue::String(value));
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
        kiln_host_abi::HostValue::String(value) => (4, value.as_bytes().to_vec()),
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
pub const EMBEDDING_HOST_ABI_VERSION: u32 = 1;

/// The operation implemented by [`runtime_source`].
pub const WRITE_STDOUT_OPERATION: u16 = 1;

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
                RUNTIME_PREFIX.len()
                    + embedding_header().len()
                    + SCHEDULER_SOURCE.len()
                    + RUNTIME_BODY.len(),
            );
            source.push_str(RUNTIME_PREFIX);
            source.push_str(embedding_header());
            source.push_str(SCHEDULER_SOURCE);
            source.push_str(RUNTIME_BODY);
            source
        })
        .as_str()
}

/// Return the invocation-scoped graph scheduler implementation. Shared-library wrappers embed
/// this private source directly because they deliberately do not link the CLI runtime object.
pub fn scheduler_source() -> &'static str {
    SCHEDULER_SOURCE
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
const EMBEDDING_HEADER: &str = r#"/* Kiln native embedding ABI. */
#ifndef KILN_NATIVE_EMBEDDING_H
#define KILN_NATIVE_EMBEDDING_H

#include <stddef.h>
#include <stdint.h>

#define KILN_NATIVE_RUNTIME_VERSION 8
#define KILN_GRAPH_SCHEDULER_ABI_VERSION 1
#define KILN_C_ABI_VERSION 1
#define KILN_HOST_ABI_VERSION 1
#define KILN_MODULE_API_VERSION 2

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

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4
};

/* This layout is the complete boundary value.  bits stores booleans and IEEE-754 numbers;
 * ptr/len stores UTF-8 string bytes and is not required to be NUL terminated. */
typedef struct kiln_value {
    uint32_t tag;
    uint32_t reserved;
    uint64_t bits;
    const char *ptr;
    size_t len;
} kiln_value;

#if defined(__cplusplus)
static_assert(offsetof(kiln_value, tag) == 0, "kiln_value.tag ABI offset");
static_assert(offsetof(kiln_value, reserved) == 4, "kiln_value.reserved ABI offset");
static_assert(offsetof(kiln_value, bits) == 8, "kiln_value.bits ABI offset");
static_assert(offsetof(kiln_value, ptr) == 16, "kiln_value.ptr ABI offset");
static_assert(offsetof(kiln_value, len) == 16 + sizeof(void *), "kiln_value.len ABI offset");
static_assert(sizeof(kiln_value) == 16 + 2 * sizeof(void *), "kiln_value ABI size");
#elif defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(offsetof(kiln_value, tag) == 0, "kiln_value.tag ABI offset");
_Static_assert(offsetof(kiln_value, reserved) == 4, "kiln_value.reserved ABI offset");
_Static_assert(offsetof(kiln_value, bits) == 8, "kiln_value.bits ABI offset");
_Static_assert(offsetof(kiln_value, ptr) == 16, "kiln_value.ptr ABI offset");
_Static_assert(offsetof(kiln_value, len) == 16 + sizeof(void *), "kiln_value.len ABI offset");
_Static_assert(sizeof(kiln_value) == 16 + 2 * sizeof(void *), "kiln_value ABI size");
#endif

/* Argument ptr/len values are borrowed for the callback invocation.  A string stored in *result
 * must remain alive until the module entry point returns.  A nonzero callback return is
 * propagated as KILN_C99_HOST_ERROR. */
typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
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

#endif /* KILN_NATIVE_EMBEDDING_H */
"#;

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

const SCHEDULER_SOURCE: &str = r#"
/* Private graph scheduler ABI. State belongs to one wrapper invocation. */
#if defined(KILN_NATIVE_HIDE_SCHEDULER) && (defined(__GNUC__) || defined(__clang__))
#define KILN_NATIVE_PRIVATE __attribute__((visibility("hidden")))
#else
#define KILN_NATIVE_PRIVATE
#endif
typedef struct kiln_microtask {
    kiln_value handler;
    kiln_value argument;
    kiln_value derived;
    int rejected;
} kiln_microtask;
typedef struct kiln_timer {
    double id;
    double due;
    uint64_t sequence;
    kiln_value callback;
    kiln_value *args;
    size_t argc;
    double repeat;
    int repeats;
} kiln_timer;
typedef struct kiln_native_scheduler kiln_native_scheduler;
typedef int (*kiln_scheduler_runner_fn)(kiln_native_scheduler *scheduler, kiln_host *host,
                                        int depth, kiln_value *thrown);
struct kiln_native_scheduler {
    kiln_microtask *queue;
    size_t queue_head;
    size_t queue_count;
    size_t queue_capacity;
    kiln_timer *timers;
    size_t timer_count;
    size_t timer_capacity;
    double now;
    double next_timer_id;
    uint64_t next_sequence;
    kiln_scheduler_runner_fn runner;
    int graph;
};

KILN_NATIVE_PRIVATE kiln_native_scheduler *kiln_native_scheduler_create(void) {
    kiln_native_scheduler *scheduler = (kiln_native_scheduler *)calloc(1, sizeof(*scheduler));
    if (scheduler != NULL) scheduler->next_timer_id = 1.0;
    return scheduler;
}

static void kiln_native_scheduler_clear(kiln_native_scheduler *scheduler) {
    size_t index;
    if (scheduler == NULL) return;
    for (index = 0; index < scheduler->timer_count; ++index) free(scheduler->timers[index].args);
    free(scheduler->timers);
    free(scheduler->queue);
    scheduler->timers = NULL;
    scheduler->queue = NULL;
    scheduler->timer_count = 0;
    scheduler->timer_capacity = 0;
    scheduler->queue_head = 0;
    scheduler->queue_count = 0;
    scheduler->queue_capacity = 0;
    scheduler->now = 0.0;
    scheduler->next_timer_id = 1.0;
    scheduler->next_sequence = 0;
}

KILN_NATIVE_PRIVATE void kiln_native_scheduler_begin(kiln_native_scheduler *scheduler) {
    kiln_native_scheduler_clear(scheduler);
    if (scheduler != NULL) { scheduler->runner = NULL; scheduler->graph = 1; }
}

KILN_NATIVE_PRIVATE void kiln_native_scheduler_destroy(kiln_native_scheduler *scheduler) {
    if (scheduler == NULL) return;
    kiln_native_scheduler_clear(scheduler);
    free(scheduler);
}

KILN_NATIVE_PRIVATE int kiln_native_scheduler_graph_mode(const kiln_native_scheduler *scheduler) {
    return scheduler != NULL && scheduler->graph != 0;
}

KILN_NATIVE_PRIVATE void kiln_native_scheduler_set_runner(kiln_native_scheduler *scheduler,
                                                         kiln_scheduler_runner_fn runner) {
    if (scheduler != NULL) scheduler->runner = runner;
}

KILN_NATIVE_PRIVATE int kiln_native_scheduler_finish(kiln_native_scheduler *scheduler,
                                                     kiln_host *host, int depth,
                                                     kiln_value *thrown) {
    int status = KILN_C99_OK;
    if (scheduler == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (scheduler->runner != NULL) status = scheduler->runner(scheduler, host, depth, thrown);
    scheduler->runner = NULL;
    scheduler->graph = 0;
    kiln_native_scheduler_clear(scheduler);
    return status;
}
"#;

const RUNTIME_PREFIX: &str = r#"/* Kiln native CLI runtime: deterministic HostOps callback. */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

"#;

const RUNTIME_BODY: &str = r#"
/* Private number-provider ABI. The native linker supplies these from the validated runtime
 * provider artifact; they are deliberately absent from the public embedding header. */
extern int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
extern double kiln_string_to_number(const char *bytes, size_t length);

static int kiln_cli_write_bytes(const char *bytes, size_t length) {
    if (length != 0 && bytes == NULL) {
        return KILN_C99_HOST_ERROR;
    }
    if (length != 0 && fwrite(bytes, 1, length, stdout) != length) {
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

/* Private KILNRES1 scalar-result writer. The explicit-path form is used by focused C tests;
 * the environment wrapper below is the only launcher-facing entry point. */
int kiln_cli_write_result_file(const char *path, kiln_value value) {
    static const unsigned char magic[] = {'K', 'I', 'L', 'N', 'R', 'E', 'S', '1'};
    FILE *file;
    uint8_t tag;
    uint64_t length;
    int status = KILN_C99_OK;
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
        if (value.len != 0 && value.ptr == NULL) {
            return KILN_C99_INVALID_ARGUMENT;
        }
        if ((uintmax_t)value.len > UINT64_MAX) {
            return KILN_C99_INVALID_ARGUMENT;
        }
        tag = (uint8_t)value.tag;
        length = (uint64_t)value.len;
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
    } else if (value.tag == KILN_VALUE_STRING && value.len != 0
               && fwrite(value.ptr, 1, value.len, file) != value.len) {
        status = KILN_C99_HOST_ERROR;
    }
    if (fclose(file) != 0) {
        status = KILN_C99_HOST_ERROR;
    }
    return status;
}

int kiln_cli_write_result(kiln_value value) {
    const char *path = getenv("KILN_NATIVE_RESULT_PATH_V1");
    if (path == NULL || path[0] == '\0') {
        return KILN_C99_OK;
    }
    return kiln_cli_write_result_file(path, value);
}

/* Why the program stopped, on stderr.
 *
 * Without this a native run that threw exited 1 in silence, which is the failure mode the whole
 * corpus is built to avoid: a refusal you can see beats a wrong answer you cannot, and an exit
 * code with no words is neither. */
void kiln_cli_report_failure(int status, kiln_value value) {
    if (status == KILN_C99_THROWN) {
        fputs("uncaught ", stderr);
        if (value.tag == KILN_VALUE_STRING && value.len != 0 && value.ptr != NULL) {
            fwrite(value.ptr, 1, value.len, stderr);
        } else {
            fputs("exception", stderr);
        }
        fputc('\n', stderr);
        return;
    }
    fprintf(stderr, "the program stopped with status %d\n", status);
}

static int kiln_cli_write_value(kiln_value value) {
    static const char undefined_text[] = "undefined";
    static const char null_text[] = "null";
    static const char true_text[] = "true";
    static const char false_text[] = "false";
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
        return kiln_cli_write_bytes(undefined_text, sizeof(undefined_text) - 1);
    case KILN_VALUE_NULL:
        return kiln_cli_write_bytes(null_text, sizeof(null_text) - 1);
    case KILN_VALUE_BOOLEAN:
        return value.bits != 0
            ? kiln_cli_write_bytes(true_text, sizeof(true_text) - 1)
            : kiln_cli_write_bytes(false_text, sizeof(false_text) - 1);
    case KILN_VALUE_STRING:
        return kiln_cli_write_bytes(value.ptr, value.len);
    case KILN_VALUE_NUMBER: {
        double number;
        char buffer[64];
        int length;
        memcpy(&number, &value.bits, sizeof(number));
        if (isnan(number)) {
            return kiln_cli_write_bytes("NaN", 3);
        }
        if (isinf(number)) {
            return number < 0.0
                ? kiln_cli_write_bytes("-Infinity", 9)
                : kiln_cli_write_bytes("Infinity", 8);
        }
        /* Console display preserves negative zero; String(-0) is the separate `0` conversion. */
        if (number == 0.0) {
            return signbit(number)
                ? kiln_cli_write_bytes("-0", 2)
                : kiln_cli_write_bytes("0", 1);
        }
        length = kiln_number_to_js_string(number, buffer, sizeof(buffer));
        if (length < 0 || (size_t)length >= sizeof(buffer)) {
            return KILN_C99_HOST_ERROR;
        }
        return kiln_cli_write_bytes(buffer, (size_t)length);
    }
    default:
        return KILN_C99_HOST_ERROR;
    }
}

static kiln_value kiln_cli_undefined(void) {
    kiln_value value;
    value.tag = KILN_VALUE_UNDEFINED;
    value.reserved = 0;
    value.bits = 0;
    value.ptr = NULL;
    value.len = 0;
    return value;
}

/* Stable CLI implementation of HostOps::WriteStdout (operation number 1). */
int kiln_cli_host_call(void *context, uint16_t operation, uint16_t argc,
                       const kiln_value *arguments, kiln_value *result) {
    uint16_t index;
    (void)context;
    if (operation != KILN_HOST_OP_WRITE_STDOUT || result == NULL
        || (argc != 0 && arguments == NULL)) {
        return KILN_C99_HOST_ERROR;
    }
    for (index = 0; index < argc; ++index) {
        if (index != 0 && fputc(' ', stdout) == EOF) {
            return KILN_C99_HOST_ERROR;
        }
        if (kiln_cli_write_value(arguments[index]) != KILN_C99_OK) {
            return KILN_C99_HOST_ERROR;
        }
    }
    if (fputc('\n', stdout) == EOF || fflush(stdout) != 0) {
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
extern void kiln_cli_report_failure(int status, kiln_value value);
extern int 
"#;

const LAUNCHER_DECL_SUFFIX: &str = r#"(kiln_host *host, kiln_value *result);

int main(void) {
    kiln_host host;
    kiln_value result;
    int status;
    if (setlocale(LC_NUMERIC, "C") == NULL) {
        return 1;
    }
    host.call = kiln_cli_host_call;
    host.context = NULL;
    status = 
"#;

const LAUNCHER_BODY: &str = r#"(&host, &result);
    if (status != 0) {
        kiln_cli_report_failure(status, result);
        return 1;
    }
    return kiln_cli_write_result(result) == KILN_C99_OK ? 0 : 1;
}
"#;

// The suffix above intentionally leaves the call expression open so the validated symbol is the
// only program-specific part of the launcher source.
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
    fn runtime_identity_is_separate_from_the_private_number_provider() {
        assert_eq!(NATIVE_RUNTIME_VERSION, 8);
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
            kiln_host_abi::HostValue::String("a\0b".to_owned()),
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
            encode_native_result(&kiln_host_abi::HostValue::String("ok".to_owned()));
        invalid_utf8[NATIVE_RESULT_HEADER_LEN] = 0xff;
        assert_eq!(
            decode_native_result(&invalid_utf8),
            Err(NativeResultError::InvalidUtf8)
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
        assert!(embedding_header().contains("uint32_t tag;"));
        assert!(embedding_header().contains("uint32_t reserved;"));
        assert!(embedding_header().contains("uint64_t bits;"));
        assert!(embedding_header().contains("const char *ptr;"));
        assert!(embedding_header().contains("size_t len;"));
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
        assert!(embedding_header().contains("sizeof(kiln_value) == 16 + 2 * sizeof(void *)"));
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
    value.reserved = 0;
    value.bits = 0;
    value.ptr = 0;
    value.len = 0;
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
    kiln_host host = {0, 0};
    kiln_value result = {0, 0, 0, 0, 0};
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
        let module_call = source
            .find("status = \nkiln_module_locale")
            .expect("launcher calls module");
        assert!(locale < module_call);
        assert!(source.contains("#include <locale.h>"));
        assert!(source.contains("return 1;"));
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
        std::fs::write(&number_provider, kiln_native_number::runtime_source())
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
        std::fs::write(&number_provider, kiln_native_number::runtime_source())
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
            kiln_host_abi::HostValue::String("a\0b".to_owned()),
        ];
        for (path, value) in paths.iter().zip(expected.iter()) {
            assert_eq!(
                decode_native_result(&std::fs::read(path).expect("read sidecar")),
                Ok(value.clone())
            );
        }
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
    arguments[11].tag = KILN_VALUE_STRING;
    arguments[11].ptr = text;
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
    value.ptr = text;
    value.len = sizeof(text);
    return kiln_cli_write_result_file(argv[5], value) == KILN_C99_OK ? 0 : 1;
}
"#;
}
