//! Deterministic C99 emission for Kiln's scalar bytecode.
//!
//! This crate is deliberately a backend, not a C runtime.  The generated translation unit owns
//! the value representation and calls a host callback for every [`kiln_ir::Instruction::HostCall`]
//! using the numeric [`kiln_host_abi::HostOp`] value.  In particular, it never writes stdout (or
//! otherwise bakes CLI policy into an object file).

use std::{collections::HashSet, fmt::Write as _};

use kiln_host_abi::{HOST_ABI_VERSION, HostOp};
use kiln_ir::{BYTECODE_VERSION, BinaryOp, Constant, Instruction, ModuleProgram, Program, UnaryOp};

/// Version of the generated C ABI.  This is independent from the bytecode and HostOps versions.
pub const C_ABI_VERSION: u32 = 1;

/// A generated C99 translation unit and its deterministic exported entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CModule {
    /// The exported function name in [`Self::source`].
    pub symbol: String,
    /// Complete, self-contained C99 source for one translation unit.
    pub source: String,
    /// Stable primary FNV-1a identity of the bytecode input and, for [`emit_module`], linkage.
    pub program_hash: u64,
}

/// Backwards-compatible descriptive alias for callers that call an emitted unit a module.
pub type GeneratedModule = CModule;

/// Why emission was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmitError {
    /// The bytecode asks this backend to do something outside its implemented surface.
    Unsupported { feature: String },
    /// The bytecode is malformed and cannot be represented safely as C.
    InvalidProgram { message: String },
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { feature } => {
                write!(formatter, "unsupported native emission: {feature}")
            }
            Self::InvalidProgram { message } => {
                write!(formatter, "invalid bytecode program: {message}")
            }
        }
    }
}

impl std::error::Error for EmitError {}

/// Emit one scalar bytecode program as a deterministic C99 translation unit.
///
/// The symbol is derived only from the canonical bytecode contents; no source path, timestamp,
/// process id, or caller-provided name appears in the generated output.  This makes equal
/// programs produce byte-for-byte equal objects while still giving each distinct program an
/// externally linkable entry point.  The entry point includes two independently seeded 64-bit
/// FNV-1a digests; [`CModule::program_hash`] remains the primary digest for API compatibility.
pub fn emit(program: &Program) -> Result<CModule, EmitError> {
    validate(program)?;
    let (program_hash, symbol_hash) = hash_program_pair(program);
    let symbol = format!("kiln_module_{program_hash:016x}{symbol_hash:016x}");
    let source = render(program, program_hash, &symbol);
    Ok(CModule {
        symbol,
        source,
        program_hash,
    })
}

/// Explicitly named spelling for users integrating several backend implementations.
pub fn emit_c(program: &Program) -> Result<CModule, EmitError> {
    emit(program)
}

/// Emit one linkage-aware module as a deterministic C99 translation unit.
///
/// The generated entry point extends the scalar ABI while retaining the original `host` and
/// `result` arguments at the front of the signature:
///
/// ```text
/// int symbol(kiln_host *host, kiln_value *result,
///            const kiln_value *imports, size_t import_count,
///            kiln_value *exports, size_t export_count)
/// ```
///
/// Imports are ordered like [`ModuleProgram::imports`] and exports are ordered like
/// [`ModuleProgram::exports`].  Import slots are initialized before the first instruction and
/// export slots are written only after successful execution.
pub fn emit_module(module: &ModuleProgram) -> Result<CModule, EmitError> {
    let max_stack = validate_module(module)?;
    let (program_hash, symbol_hash) = hash_module_pair(module);
    let symbol = format!("kiln_module_{program_hash:016x}{symbol_hash:016x}");
    let source = render_module(module, max_stack, program_hash, &symbol);
    Ok(CModule {
        symbol,
        source,
        program_hash,
    })
}

/// Emit a linkage-aware module with a symbol derived from its normalized logical path.
///
/// Graph launchers need to keep their own object cache entry when a module's body changes.  The
/// ordinary [`emit_module`] symbol intentionally includes the complete bytecode, which is useful
/// for standalone content-addressed units but would make the launcher change for every body edit.
/// A graph caller therefore supplies a project-relative path in canonical slash form.  The path
/// is validated here so the resulting symbol is independent of an absolute checkout location and
/// always has a valid C identifier.
pub fn emit_module_with_logical_path(
    module: &ModuleProgram,
    logical_path: &str,
) -> Result<CModule, EmitError> {
    validate_logical_path(logical_path)?;
    let max_stack = validate_module(module)?;
    let (program_hash, _) = hash_module_pair(module);
    let (path_hash, path_hash_seeded) = hash_logical_path_pair(logical_path);
    let symbol = format!("kiln_module_path_{path_hash:016x}{path_hash_seeded:016x}");
    let source = render_module(module, max_stack, program_hash, &symbol);
    Ok(CModule {
        symbol,
        source,
        program_hash,
    })
}

fn validate_logical_path(logical_path: &str) -> Result<(), EmitError> {
    let invalid = |message: &str| {
        Err(EmitError::InvalidProgram {
            message: format!("invalid logical module path: {message}"),
        })
    };
    if logical_path.is_empty() {
        return invalid("path is empty");
    }
    if logical_path.as_bytes().contains(&0) {
        return invalid("path contains an embedded NUL");
    }
    // Logical paths are deliberately stricter than host filesystem paths.  In particular, a
    // leading slash, backslash, or drive prefix could smuggle an absolute checkout path into the
    // supposedly project-relative identity, and empty/dot components are not normalized input.
    if logical_path.starts_with('/')
        || logical_path.starts_with('\\')
        || logical_path.as_bytes().get(1) == Some(&b':')
        || logical_path.contains('\\')
    {
        return invalid("path is not project-relative slash-separated input");
    }
    if logical_path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return invalid("path contains an empty, `.` or `..` component");
    }
    Ok(())
}

const HEADER: &str = r#"/* Kiln C99 backend: deterministic scalar unit. */
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* C ABI version 1; HostOps ABI version 1. */
#define KILN_C99_OK 0
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)

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

/* A nonzero callback return is propagated as KILN_C99_HOST_ERROR.  The callback owns any
 * interpretation of operation numbers and may replace *result. */
typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
} kiln_host;

static kiln_value kiln_c99_value(uint32_t tag, uint64_t bits, const char *ptr, size_t len) {
    kiln_value value;
    value.tag = tag;
    value.reserved = 0;
    value.bits = bits;
    value.ptr = ptr;
    value.len = len;
    return value;
}

static kiln_value kiln_c99_undefined(void) {
    return kiln_c99_value(KILN_VALUE_UNDEFINED, 0, NULL, 0);
}

static kiln_value kiln_c99_null(void) {
    return kiln_c99_value(KILN_VALUE_NULL, 0, NULL, 0);
}

static kiln_value kiln_c99_boolean(int value) {
    return kiln_c99_value(KILN_VALUE_BOOLEAN, value ? 1u : 0u, NULL, 0);
}

static kiln_value kiln_c99_number(double value) {
    uint64_t bits;
    memcpy(&bits, &value, sizeof(bits));
    return kiln_c99_value(KILN_VALUE_NUMBER, bits, NULL, 0);
}

static kiln_value kiln_c99_number_bits(uint64_t bits) {
    return kiln_c99_value(KILN_VALUE_NUMBER, bits, NULL, 0);
}

static double kiln_c99_as_number(kiln_value value) {
    double number;
    switch (value.tag) {
    case KILN_VALUE_NULL:
        return 0.0;
    case KILN_VALUE_BOOLEAN:
        return value.bits ? 1.0 : 0.0;
    case KILN_VALUE_NUMBER:
        memcpy(&number, &value.bits, sizeof(number));
        return number;
    case KILN_VALUE_STRING:
        if (value.ptr == NULL || value.len == 0) {
            return 0.0;
        }
        /* strtod requires NUL termination.  The temporary is bounded to keep malformed host
         * values from requesting an unbounded allocation in this small scalar backend. */
        if (value.len > (size_t)-1 - 1) {
            return NAN;
        }
        {
            char *copy = (char *)malloc(value.len + 1);
            char *end;
            double parsed;
            if (copy == NULL) {
                return NAN;
            }
            memcpy(copy, value.ptr, value.len);
            copy[value.len] = '\0';
            parsed = strtod(copy, &end);
            while (*end == ' ' || *end == '\t' || *end == '\n' || *end == '\r') {
                ++end;
            }
            if (*end != '\0') {
                parsed = NAN;
            }
            free(copy);
            return parsed;
        }
    default:
        return NAN;
    }
}

static int kiln_c99_truthy(kiln_value value) {
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_NULL:
        return 0;
    case KILN_VALUE_BOOLEAN:
        return value.bits != 0;
    case KILN_VALUE_NUMBER: {
        double number = kiln_c99_as_number(value);
        return number != 0.0 && !isnan(number);
    }
    case KILN_VALUE_STRING:
        return value.len != 0;
    default:
        return 0;
    }
}

static kiln_value kiln_c99_copy_text(const char *text, size_t len) {
    char *copy = (char *)malloc(len == 0 ? 1 : len);
    if (copy == NULL) {
        return kiln_c99_undefined();
    }
    if (len != 0) {
        memcpy(copy, text, len);
    }
    return kiln_c99_value(KILN_VALUE_STRING, 0, copy, len);
}

static kiln_value kiln_c99_text(kiln_value value) {
    static const char undefined_text[] = "undefined";
    static const char null_text[] = "null";
    static const char true_text[] = "true";
    static const char false_text[] = "false";
    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
        return kiln_c99_value(KILN_VALUE_STRING, 0, undefined_text, sizeof(undefined_text) - 1);
    case KILN_VALUE_NULL:
        return kiln_c99_value(KILN_VALUE_STRING, 0, null_text, sizeof(null_text) - 1);
    case KILN_VALUE_BOOLEAN:
        if (value.bits) {
            return kiln_c99_value(KILN_VALUE_STRING, 0, true_text, sizeof(true_text) - 1);
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, false_text, sizeof(false_text) - 1);
    case KILN_VALUE_STRING:
        return value;
    case KILN_VALUE_NUMBER: {
        char buffer[64];
        double number = kiln_c99_as_number(value);
        int len = snprintf(buffer, sizeof(buffer), "%.17g", number);
        if (len < 0 || (size_t)len >= sizeof(buffer)) {
            return kiln_c99_undefined();
        }
        return kiln_c99_copy_text(buffer, (size_t)len);
    }
    default:
        return kiln_c99_undefined();
    }
}

static kiln_value kiln_c99_concat(kiln_value left, kiln_value right) {
    kiln_value l = kiln_c99_text(left);
    kiln_value r = kiln_c99_text(right);
    char *copy;
    if (l.ptr == NULL || r.ptr == NULL || l.len > (size_t)-1 - r.len) {
        return kiln_c99_undefined();
    }
    copy = (char *)malloc((l.len + r.len) == 0 ? 1 : l.len + r.len);
    if (copy == NULL) {
        return kiln_c99_undefined();
    }
    if (l.len != 0) {
        memcpy(copy, l.ptr, l.len);
    }
    if (r.len != 0) {
        memcpy(copy + l.len, r.ptr, r.len);
    }
    return kiln_c99_value(KILN_VALUE_STRING, 0, copy, l.len + r.len);
}

static int kiln_c99_strict_equal(kiln_value left, kiln_value right) {
    if (left.tag != right.tag) {
        return 0;
    }
    if (left.tag == KILN_VALUE_NUMBER) {
        double l = kiln_c99_as_number(left);
        double r = kiln_c99_as_number(right);
        return l == r;
    }
    if (left.tag == KILN_VALUE_STRING) {
        return left.len == right.len &&
               (left.len == 0 || (left.ptr != NULL && right.ptr != NULL &&
                                  memcmp(left.ptr, right.ptr, left.len) == 0));
    }
    return left.bits == right.bits;
}

/* Decode the UTF-8 used at the C boundary into Unicode scalar values.  Generated constants are
 * valid UTF-8, and host strings are required to use the same representation.  Treating malformed
 * bytes as U+FFFD keeps comparison bounded and deterministic if a host violates that contract. */
static uint32_t kiln_c99_utf8_next(const char *text, size_t length, size_t *offset) {
    size_t index = *offset;
    uint32_t codepoint;
    unsigned char first;
    unsigned char second;
    unsigned char third;
    unsigned char fourth;
    if (index >= length || text == NULL) {
        return UINT32_MAX;
    }
    first = (unsigned char)text[index++];
    if (first < 0x80) {
        *offset = index;
        return first;
    }
    if (first >= 0xC2 && first <= 0xDF && index < length) {
        second = (unsigned char)text[index];
        if ((second & 0xC0) == 0x80) {
            *offset = index + 1;
            return ((uint32_t)(first & 0x1F) << 6) | (uint32_t)(second & 0x3F);
        }
    }
    if (first >= 0xE0 && first <= 0xEF && index + 1 < length) {
        second = (unsigned char)text[index];
        third = (unsigned char)text[index + 1];
        if ((second & 0xC0) == 0x80 && (third & 0xC0) == 0x80 &&
            (first != 0xE0 || second >= 0xA0) && (first != 0xED || second < 0xA0)) {
            *offset = index + 2;
            return ((uint32_t)(first & 0x0F) << 12) |
                   ((uint32_t)(second & 0x3F) << 6) | (uint32_t)(third & 0x3F);
        }
    }
    if (first >= 0xF0 && first <= 0xF4 && index + 2 < length) {
        second = (unsigned char)text[index];
        third = (unsigned char)text[index + 1];
        fourth = (unsigned char)text[index + 2];
        if ((second & 0xC0) == 0x80 && (third & 0xC0) == 0x80 &&
            (fourth & 0xC0) == 0x80 && (first != 0xF0 || second >= 0x90) &&
            (first != 0xF4 || second <= 0x8F)) {
            *offset = index + 3;
            codepoint = ((uint32_t)(first & 0x07) << 18) |
                        ((uint32_t)(second & 0x3F) << 12) |
                        ((uint32_t)(third & 0x3F) << 6) | (uint32_t)(fourth & 0x3F);
            return codepoint;
        }
    }
    /* Consume one byte, allowing the next byte to start a fresh sequence. */
    *offset = index;
    return UINT32_C(0xFFFD);
}

typedef struct kiln_c99_utf16_iter {
    const char *text;
    size_t length;
    size_t offset;
    uint16_t pending;
    int has_pending;
} kiln_c99_utf16_iter;

static int kiln_c99_utf16_next(kiln_c99_utf16_iter *iterator, uint16_t *unit) {
    uint32_t codepoint;
    if (iterator->has_pending) {
        iterator->has_pending = 0;
        *unit = iterator->pending;
        return 1;
    }
    if (iterator->offset >= iterator->length || iterator->text == NULL) {
        return 0;
    }
    codepoint = kiln_c99_utf8_next(iterator->text, iterator->length, &iterator->offset);
    if (codepoint > UINT32_C(0xFFFF)) {
        codepoint -= UINT32_C(0x10000);
        *unit = (uint16_t)(UINT32_C(0xD800) | (codepoint >> 10));
        iterator->pending = (uint16_t)(UINT32_C(0xDC00) | (codepoint & UINT32_C(0x3FF)));
        iterator->has_pending = 1;
    } else {
        *unit = (uint16_t)codepoint;
    }
    return 1;
}

static int kiln_c99_string_compare(kiln_value left, kiln_value right) {
    kiln_c99_utf16_iter left_iterator = {left.ptr, left.len, 0, 0, 0};
    kiln_c99_utf16_iter right_iterator = {right.ptr, right.len, 0, 0, 0};
    uint16_t left_unit;
    uint16_t right_unit;
    int left_has;
    int right_has;
    for (;;) {
        left_has = kiln_c99_utf16_next(&left_iterator, &left_unit);
        right_has = kiln_c99_utf16_next(&right_iterator, &right_unit);
        if (!left_has || !right_has) {
            return left_has ? 1 : (right_has ? -1 : 0);
        }
        if (left_unit != right_unit) {
            return left_unit < right_unit ? -1 : 1;
        }
    }
}

static kiln_value kiln_c99_unary(uint16_t operation, kiln_value value) {
    switch (operation) {
    case 1: return kiln_c99_number(kiln_c99_as_number(value));
    case 2: return kiln_c99_number(-kiln_c99_as_number(value));
    case 3: return kiln_c99_boolean(!kiln_c99_truthy(value));
    default: return kiln_c99_undefined();
    }
}

static kiln_value kiln_c99_binary(uint16_t operation, kiln_value left, kiln_value right) {
    switch (operation) {
    case 1:
        if (left.tag == KILN_VALUE_STRING || right.tag == KILN_VALUE_STRING) {
            return kiln_c99_concat(left, right);
        }
        return kiln_c99_number(kiln_c99_as_number(left) + kiln_c99_as_number(right));
    case 2: return kiln_c99_number(kiln_c99_as_number(left) - kiln_c99_as_number(right));
    case 3: return kiln_c99_number(kiln_c99_as_number(left) * kiln_c99_as_number(right));
    case 4: return kiln_c99_number(kiln_c99_as_number(left) / kiln_c99_as_number(right));
    case 5: return kiln_c99_number(fmod(kiln_c99_as_number(left), kiln_c99_as_number(right)));
    case 6: return kiln_c99_number(pow(kiln_c99_as_number(left), kiln_c99_as_number(right)));
    case 7: return kiln_c99_boolean(kiln_c99_strict_equal(left, right));
    case 8: return kiln_c99_boolean(!kiln_c99_strict_equal(left, right));
    case 9:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) < 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) < kiln_c99_as_number(right));
    case 10:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) <= 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) <= kiln_c99_as_number(right));
    case 11:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) > 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) > kiln_c99_as_number(right));
    case 12:
        if (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_STRING) {
            return kiln_c99_boolean(kiln_c99_string_compare(left, right) >= 0);
        }
        return kiln_c99_boolean(kiln_c99_as_number(left) >= kiln_c99_as_number(right));
    default: return kiln_c99_undefined();
    }
}

"#;

fn validate(program: &Program) -> Result<usize, EmitError> {
    if program.version != BYTECODE_VERSION {
        return Err(EmitError::Unsupported {
            feature: format!(
                "bytecode version {} (backend supports {})",
                program.version, BYTECODE_VERSION
            ),
        });
    }
    if program.code.is_empty() {
        return Err(EmitError::InvalidProgram {
            message: "program has no instructions".into(),
        });
    }
    for (index, instruction) in program.code.iter().enumerate() {
        match instruction {
            Instruction::Const(constant) if (*constant as usize) >= program.constants.len() => {
                return Err(invalid(
                    index,
                    format!("constant {constant} is out of bounds"),
                ));
            }
            Instruction::LoadLocal(local) | Instruction::StoreLocal(local)
                if (*local as u64) >= u64::from(program.local_count) =>
            {
                return Err(invalid(index, format!("local {local} is out of bounds")));
            }
            Instruction::Jump(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
                if (*target as usize) >= program.code.len() =>
            {
                return Err(invalid(
                    index,
                    format!("jump target {target} is out of bounds"),
                ));
            }
            Instruction::HostCall { op, .. } if !matches!(op, HostOp::WriteStdout) => {
                return Err(EmitError::Unsupported {
                    feature: format!("host operation {op:?} at instruction {index}"),
                });
            }
            _ => {}
        }
    }

    // Dataflow validation keeps generated stack indexing defined on every reachable branch and
    // calculates a compact allocation for the emitted function.
    let mut heights: Vec<Option<usize>> = vec![None; program.code.len()];
    let mut work = vec![(0usize, 0usize)];
    let mut max_height = 0usize;
    while let Some((index, height)) = work.pop() {
        if let Some(previous) = heights[index] {
            if previous != height {
                return Err(invalid(
                    index,
                    format!("inconsistent stack height: reached with {previous} and {height}"),
                ));
            }
            continue;
        }
        heights[index] = Some(height);
        max_height = max_height.max(height);
        let instruction = &program.code[index];
        let (next_height, terminal) = stack_effect(instruction, height, index)?;
        max_height = max_height.max(next_height);
        if terminal {
            continue;
        }
        match instruction {
            Instruction::Jump(target) => work.push((*target as usize, next_height)),
            Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target) => {
                work.push((*target as usize, next_height));
                if index + 1 < program.code.len() {
                    work.push((index + 1, next_height));
                }
            }
            _ if index + 1 < program.code.len() => work.push((index + 1, next_height)),
            _ => {
                return Err(invalid(index, "control falls off the end of the program"));
            }
        }
    }
    Ok(max_height.max(1))
}

fn validate_module(module: &ModuleProgram) -> Result<usize, EmitError> {
    let max_stack = validate(&module.program)?;
    let local_count = u64::from(module.program.local_count);
    let mut initialized_slots = HashSet::with_capacity(module.imports.len());
    for (index, binding) in module.imports.iter().enumerate() {
        if u64::from(binding.local_slot) >= local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "import {index} local slot {} is out of bounds (local count {})",
                    binding.local_slot, module.program.local_count
                ),
            });
        }
        if !initialized_slots.insert(binding.local_slot) {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "import {index} local slot {} is initialized more than once",
                    binding.local_slot
                ),
            });
        }
    }
    for (index, binding) in module.exports.iter().enumerate() {
        if u64::from(binding.local_slot) >= local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "export {index} local slot {} is out of bounds (local count {})",
                    binding.local_slot, module.program.local_count
                ),
            });
        }
    }
    Ok(max_stack)
}

fn invalid(index: usize, message: impl Into<String>) -> EmitError {
    EmitError::InvalidProgram {
        message: format!("instruction {index}: {}", message.into()),
    }
}

fn stack_effect(
    instruction: &Instruction,
    height: usize,
    index: usize,
) -> Result<(usize, bool), EmitError> {
    let underflow = |needed: usize| {
        invalid(
            index,
            format!("stack needs {needed} value(s), but has {height}"),
        )
    };
    match instruction {
        // Calls are interpreter-only for now. The native backend emits one C function per
        // module with a flat local array and no call stack, so these have no lowering yet --
        // and refusing here is what keeps the two backends from silently disagreeing about
        // what a program means. `kiln run` works on these; `kiln run --native` says why not.
        Instruction::MakeFunction(_)
        | Instruction::Call { .. }
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::NewCell(_)
        | Instruction::LoadCell(_)
        | Instruction::StoreCell(_)
        | Instruction::LoadUpvalue(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::NewObject { .. }
        | Instruction::GetProperty(_)
        | Instruction::SetProperty(_)
        | Instruction::LoadBuiltin(_)
        | Instruction::New { .. } => Err(EmitError::Unsupported {
            feature: "functions and objects in the native backend".to_string(),
        }),
        Instruction::Const(_) | Instruction::LoadLocal(_) => Ok((height + 1, false)),
        Instruction::StoreLocal(_) | Instruction::Pop => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::Dup | Instruction::Unary(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((
                    height + usize::from(matches!(instruction, Instruction::Dup)),
                    false,
                ))
            }
        }
        Instruction::Binary(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::Jump(_) => Ok((height, false)),
        Instruction::JumpIfFalse(_)
        | Instruction::JumpIfTruthy(_)
        | Instruction::JumpIfNotNullish(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::HostCall { argc, .. } => {
            let argc = usize::from(*argc);
            if height < argc {
                Err(underflow(argc))
            } else {
                Ok((height - argc + 1, false))
            }
        }
        Instruction::Return => {
            if height > 1 {
                Err(invalid(
                    index,
                    "return has more than one value on the stack",
                ))
            } else {
                Ok((height, true))
            }
        }
    }
}

fn render(program: &Program, program_hash: u64, symbol: &str) -> String {
    let max_stack = validate(program).expect("render called after validation");
    let mut output = String::with_capacity(HEADER.len() + program.code.len() * 64);
    output.push_str(HEADER);
    writeln!(
        output,
        "/* Program hash: {program_hash:016x}; bytecode version: {BYTECODE_VERSION}. */"
    )
    .unwrap();
    for (index, constant) in program.constants.iter().enumerate() {
        if let Constant::String(value) = constant {
            writeln!(
                output,
                "static const char {symbol}_constant_{index}[] = {};",
                c_string(value)
            )
            .unwrap();
        }
    }
    writeln!(
        output,
        "int {symbol}(kiln_host *host, kiln_value *result) {{"
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n");
    writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
    writeln!(
        output,
        "    kiln_value locals[{}];",
        usize::try_from(program.local_count).unwrap().max(1)
    )
    .unwrap();
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    for local in 0..program.local_count {
        writeln!(output, "    locals[{local}] = kiln_c99_undefined();").unwrap();
    }
    for index in 0..program.code.len() {
        writeln!(output, "kiln_label_{index}:;").unwrap();
        emit_instruction(
            &mut output,
            &program.code[index],
            &program.constants,
            index,
            symbol,
            None,
        );
    }
    output.push_str("    *result = kiln_c99_undefined();\n    return KILN_C99_CONTROL_ERROR;\n}\n");
    output
}

fn render_module(
    module: &ModuleProgram,
    max_stack: usize,
    program_hash: u64,
    symbol: &str,
) -> String {
    let program = &module.program;
    let mut output = String::with_capacity(HEADER.len() + program.code.len() * 72);
    output.push_str(HEADER);
    writeln!(
        output,
        "/* Module hash: {program_hash:016x}; bytecode version: {BYTECODE_VERSION}; imports: {}; exports: {}. */",
        module.imports.len(),
        module.exports.len()
    )
    .unwrap();
    for (index, constant) in program.constants.iter().enumerate() {
        if let Constant::String(value) = constant {
            writeln!(
                output,
                "static const char {symbol}_constant_{index}[] = {};",
                c_string(value)
            )
            .unwrap();
        }
    }
    writeln!(
        output,
        "int {symbol}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count) {{"
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n");
    writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
    writeln!(
        output,
        "    kiln_value locals[{}];",
        usize::try_from(program.local_count).unwrap().max(1)
    )
    .unwrap();
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    writeln!(
        output,
        "    if (import_count != (size_t){} || (import_count != 0 && imports == NULL)) return KILN_C99_INVALID_ARGUMENT;",
        module.imports.len()
    )
    .unwrap();
    writeln!(
        output,
        "    if (export_count != (size_t){} || (export_count != 0 && exports == NULL)) return KILN_C99_INVALID_ARGUMENT;",
        module.exports.len()
    )
    .unwrap();
    for local in 0..program.local_count {
        writeln!(output, "    locals[{local}] = kiln_c99_undefined();").unwrap();
    }
    for (index, binding) in module.imports.iter().enumerate() {
        writeln!(
            output,
            "    locals[{}] = imports[{index}];",
            binding.local_slot
        )
        .unwrap();
    }
    for index in 0..program.code.len() {
        writeln!(output, "kiln_label_{index}:;").unwrap();
        emit_instruction(
            &mut output,
            &program.code[index],
            &program.constants,
            index,
            symbol,
            Some(&module.exports),
        );
    }
    output.push_str("    *result = kiln_c99_undefined();\n    return KILN_C99_CONTROL_ERROR;\n}\n");
    output
}

fn emit_instruction(
    output: &mut String,
    instruction: &Instruction,
    constants: &[Constant],
    index: usize,
    symbol: &str,
    exports: Option<&[kiln_ir::ExportBinding]>,
) {
    let _ = index;
    match instruction {
        // Rejected by `verify` before emission begins, so reaching here means the two passes
        // disagree about the instruction set -- a compiler bug, not a program error.
        Instruction::MakeFunction(_)
        | Instruction::Call { .. }
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::NewCell(_)
        | Instruction::LoadCell(_)
        | Instruction::StoreCell(_)
        | Instruction::LoadUpvalue(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::NewObject { .. }
        | Instruction::GetProperty(_)
        | Instruction::SetProperty(_)
        | Instruction::LoadBuiltin(_)
        | Instruction::New { .. } => {
            unreachable!("verification rejects these instructions before emission")
        }
        Instruction::Const(constant) => match &constants[*constant as usize] {
            Constant::Undefined => output.push_str("    stack[sp++] = kiln_c99_undefined();\n"),
            Constant::Null => output.push_str("    stack[sp++] = kiln_c99_null();\n"),
            Constant::Boolean(value) => {
                writeln!(output, "    stack[sp++] = kiln_c99_boolean({});", i32::from(*value)).unwrap()
            }
            Constant::Number(bits) => {
                writeln!(output, "    stack[sp++] = kiln_c99_number_bits(UINT64_C({bits}));").unwrap()
            }
            Constant::String(value) => {
                writeln!(output, "    stack[sp++] = kiln_c99_value(KILN_VALUE_STRING, 0, {symbol}_constant_{constant}, {});", value.len()).unwrap()
            }
        },
        Instruction::LoadLocal(local) => writeln!(output, "    stack[sp++] = locals[{local}];").unwrap(),
        Instruction::StoreLocal(local) => writeln!(output, "    locals[{local}] = stack[--sp];").unwrap(),
        Instruction::Pop => output.push_str("    --sp;\n"),
        Instruction::Dup => output.push_str("    stack[sp] = stack[sp - 1]; ++sp;\n"),
        Instruction::Unary(operator) => {
            writeln!(output, "    stack[sp - 1] = kiln_c99_unary({}, stack[sp - 1]);", unary_code(*operator)).unwrap()
        }
        Instruction::Binary(operator) => {
            writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; stack[sp++] = kiln_c99_binary({}, left, right); }}", binary_code(*operator)).unwrap()
        }
        Instruction::Jump(target) => writeln!(output, "    goto kiln_label_{target};").unwrap(),
        Instruction::JumpIfFalse(target) => {
            writeln!(output, "    if (!kiln_c99_truthy(stack[--sp])) goto kiln_label_{target};").unwrap()
        }
        Instruction::JumpIfTruthy(target) => {
            writeln!(output, "    if (kiln_c99_truthy(stack[--sp])) goto kiln_label_{target};").unwrap()
        }
        Instruction::JumpIfNotNullish(target) => {
            writeln!(output, "    {{ kiln_value value = stack[--sp]; if (value.tag != KILN_VALUE_UNDEFINED && value.tag != KILN_VALUE_NULL) goto kiln_label_{target}; }}").unwrap()
        }
        Instruction::HostCall { op, argc } => {
            writeln!(output, "    {{ kiln_value host_result; const kiln_value *args = {argc} == 0 ? NULL : &stack[sp - {argc}]; if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C({argc}), args, &host_result) != 0) return KILN_C99_HOST_ERROR; sp -= {argc}; stack[sp++] = host_result; }}", host_op_code(*op)).unwrap()
        }
        Instruction::Return => match exports {
            None => output.push_str(
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp]; return KILN_C99_OK;\n",
            ),
            Some(exports) => {
                output.push_str(
                    "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp];\n",
                );
                for (index, binding) in exports.iter().enumerate() {
                    writeln!(
                        output,
                        "    exports[{index}] = locals[{}];",
                        binding.local_slot
                    )
                    .unwrap();
                }
                output.push_str("    return KILN_C99_OK;\n");
            }
        },
    }
}

fn c_string(value: &str) -> String {
    let mut output = String::from("\"");
    for byte in value.bytes() {
        match byte {
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(char::from(byte)),
            _ => {
                write!(output, "\\x{byte:02X}\"\"").unwrap();
            }
        }
    }
    output.push('"');
    output
}

fn unary_code(operator: UnaryOp) -> u16 {
    match operator {
        UnaryOp::Plus => 1,
        UnaryOp::Negate => 2,
        UnaryOp::Not => 3,
        // Stable: these are persisted cache keys, so a new operator takes the next free code
        // rather than renumbering the existing ones.
        UnaryOp::TypeOf => 4,
    }
}

fn binary_code(operator: BinaryOp) -> u16 {
    match operator {
        BinaryOp::Add => 1,
        BinaryOp::Subtract => 2,
        BinaryOp::Multiply => 3,
        BinaryOp::Divide => 4,
        BinaryOp::Remainder => 5,
        BinaryOp::Exponentiate => 6,
        BinaryOp::StrictEqual => 7,
        BinaryOp::StrictNotEqual => 8,
        BinaryOp::LessThan => 9,
        BinaryOp::LessThanOrEqual => 10,
        BinaryOp::GreaterThan => 11,
        BinaryOp::GreaterThanOrEqual => 12,
    }
}

fn host_op_code(operation: HostOp) -> u16 {
    operation as u16
}

fn hash_program_pair(program: &Program) -> (u64, u64) {
    (
        hash_program(program),
        hash_program_with_seed(program, 0x84222325cbf29ce4),
    )
}

fn hash_program(program: &Program) -> u64 {
    hash_program_with_seed(program, 0xcbf29ce484222325)
}

fn hash_program_with_seed(program: &Program, seed: u64) -> u64 {
    let mut hash = seed;
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }
    word(&mut hash, u64::from(C_ABI_VERSION));
    word(&mut hash, u64::from(HOST_ABI_VERSION));
    word(&mut hash, u64::from(program.version));
    word(&mut hash, program.constants.len() as u64);
    for constant in &program.constants {
        match constant {
            Constant::Undefined => byte(&mut hash, 0),
            Constant::Null => byte(&mut hash, 1),
            Constant::Boolean(value) => {
                byte(&mut hash, 2);
                byte(&mut hash, u8::from(*value));
            }
            Constant::Number(bits) => {
                byte(&mut hash, 3);
                word(&mut hash, *bits);
            }
            Constant::String(value) => {
                byte(&mut hash, 4);
                word(&mut hash, value.len() as u64);
                for value in value.bytes() {
                    byte(&mut hash, value);
                }
            }
        }
    }
    word(&mut hash, program.code.len() as u64);
    for instruction in &program.code {
        match instruction {
            // Tags are a persisted cache key: existing ones never change, new instructions
            // take the next free values. Reusing a tag would make two different programs hash
            // alike and serve one's object file for the other.
            Instruction::LoadBuiltin(builtin) => {
                byte(&mut hash, 25);
                word(&mut hash, *builtin as u64);
            }
            Instruction::New { argc } => {
                byte(&mut hash, 26);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::NewObject { count } => {
                byte(&mut hash, 22);
                word(&mut hash, u64::from(*count));
            }
            Instruction::GetProperty(value) => {
                byte(&mut hash, 23);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetProperty(value) => {
                byte(&mut hash, 24);
                word(&mut hash, u64::from(*value));
            }
            Instruction::NewCell(value) => {
                byte(&mut hash, 17);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadCell(value) => {
                byte(&mut hash, 18);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreCell(value) => {
                byte(&mut hash, 19);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadUpvalue(value) => {
                byte(&mut hash, 20);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreUpvalue(value) => {
                byte(&mut hash, 21);
                word(&mut hash, u64::from(*value));
            }
            Instruction::MakeFunction(value) => {
                byte(&mut hash, 13);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Call { argc } => {
                byte(&mut hash, 14);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::LoadGlobal(value) => {
                byte(&mut hash, 15);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreGlobal(value) => {
                byte(&mut hash, 16);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Const(value) => {
                byte(&mut hash, 0);
                word(&mut hash, u64::from(*value));
            }
            Instruction::LoadLocal(value) => {
                byte(&mut hash, 1);
                word(&mut hash, u64::from(*value));
            }
            Instruction::StoreLocal(value) => {
                byte(&mut hash, 2);
                word(&mut hash, u64::from(*value));
            }
            Instruction::Pop => byte(&mut hash, 3),
            Instruction::Dup => byte(&mut hash, 4),
            Instruction::Unary(value) => {
                byte(&mut hash, 5);
                word(&mut hash, u64::from(unary_code(*value)));
            }
            Instruction::Binary(value) => {
                byte(&mut hash, 6);
                word(&mut hash, u64::from(binary_code(*value)));
            }
            Instruction::Jump(value) => {
                byte(&mut hash, 7);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfFalse(value) => {
                byte(&mut hash, 8);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfTruthy(value) => {
                byte(&mut hash, 9);
                word(&mut hash, u64::from(*value));
            }
            Instruction::JumpIfNotNullish(value) => {
                byte(&mut hash, 10);
                word(&mut hash, u64::from(*value));
            }
            Instruction::HostCall { op, argc } => {
                byte(&mut hash, 11);
                word(&mut hash, u64::from(host_op_code(*op)));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::Return => byte(&mut hash, 12),
        }
    }
    word(&mut hash, u64::from(program.local_count));
    hash
}

fn hash_module_pair(module: &ModuleProgram) -> (u64, u64) {
    (
        hash_module_with_seed(module, 0xcbf29ce484222325),
        hash_module_with_seed(module, 0x84222325cbf29ce4),
    )
}

fn hash_logical_path_pair(logical_path: &str) -> (u64, u64) {
    (
        hash_logical_path_with_seed(logical_path, 0xcbf29ce484222325),
        hash_logical_path_with_seed(logical_path, 0x84222325cbf29ce4),
    )
}

fn hash_logical_path_with_seed(logical_path: &str, seed: u64) -> u64 {
    let mut hash = seed;
    for byte in b"kiln-logical-module-path\0" {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in (logical_path.len() as u64).to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    for byte in logical_path.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn hash_module_with_seed(module: &ModuleProgram, seed: u64) -> u64 {
    let mut hash = hash_program_with_seed(&module.program, seed);
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }
    fn text(hash: &mut u64, value: &str) {
        word(hash, value.len() as u64);
        for byte in value.bytes() {
            *hash ^= u64::from(byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }

    // Tags and lengths make the linkage stream unambiguous even when names happen to share
    // prefixes.  Preserve the source order promised by ModuleProgram for all three lists.
    byte(&mut hash, 0x40);
    word(&mut hash, module.dependencies.len() as u64);
    for dependency in &module.dependencies {
        text(&mut hash, dependency);
    }
    byte(&mut hash, 0x41);
    word(&mut hash, module.imports.len() as u64);
    for binding in &module.imports {
        text(&mut hash, &binding.source);
        text(&mut hash, &binding.imported);
        text(&mut hash, &binding.local);
        word(&mut hash, u64::from(binding.local_slot));
    }
    byte(&mut hash, 0x42);
    word(&mut hash, module.exports.len() as u64);
    for binding in &module.exports {
        text(&mut hash, &binding.exported);
        word(&mut hash, u64::from(binding.local_slot));
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    fn program(constants: Vec<Constant>, code: Vec<Instruction>, local_count: u32) -> Program {
        Program {
            version: BYTECODE_VERSION,
            constants,
            code,
            local_count,
            functions: Vec::new(),
        }
    }

    fn module(
        program: Program,
        imports: Vec<kiln_ir::ImportBinding>,
        exports: Vec<kiln_ir::ExportBinding>,
    ) -> ModuleProgram {
        ModuleProgram {
            program,
            dependencies: Vec::new(),
            imports,
            exports,
        }
    }

    #[test]
    fn emits_stable_source_and_path_independent_symbol() {
        let program = program(
            vec![Constant::Number(42.0f64.to_bits())],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        );
        let first = emit(&program).expect("program emits");
        let second = emit(&program).expect("program emits again");
        assert_eq!(first, second);
        assert!(first.symbol.starts_with("kiln_module_"));
        assert_eq!(first.symbol.len(), "kiln_module_".len() + 32);
        assert!(
            first.symbol["kiln_module_".len()..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert!(!first.source.contains("/Users/"));
        assert!(first.source.contains(&format!(
            "int {}(kiln_host *host, kiln_value *result)",
            first.symbol
        )));
    }

    #[test]
    fn module_linkage_is_deterministic_and_part_of_identity() {
        let body = program(
            vec![Constant::Number(1.0f64.to_bits())],
            vec![Instruction::LoadLocal(0), Instruction::Return],
            1,
        );
        let first = module(
            body.clone(),
            vec![kiln_ir::ImportBinding {
                source: "dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            Vec::new(),
        );
        let second = module(
            body,
            vec![kiln_ir::ImportBinding {
                source: "other-dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            Vec::new(),
        );
        let emitted = emit_module(&first).expect("module emits");
        assert_eq!(emitted, emit_module(&first).expect("module emits again"));
        assert_ne!(
            emitted.program_hash,
            emit_module(&second).unwrap().program_hash
        );
        assert!(emitted.source.contains("const kiln_value *imports"));
        assert!(emitted.source.contains("size_t import_count"));
    }

    #[test]
    fn logical_path_symbol_survives_body_edits_and_is_checkout_independent() {
        let first = module(
            program(
                vec![Constant::Number(1.0f64.to_bits())],
                vec![Instruction::Const(0), Instruction::Return],
                0,
            ),
            Vec::new(),
            Vec::new(),
        );
        let second = module(
            program(
                vec![Constant::Number(2.0f64.to_bits())],
                vec![Instruction::Const(0), Instruction::Return],
                0,
            ),
            Vec::new(),
            Vec::new(),
        );
        let one = emit_module_with_logical_path(&first, "src/dep.js").expect("module emits");
        let two = emit_module_with_logical_path(&second, "src/dep.js").expect("module emits");
        let other = emit_module_with_logical_path(&first, "src/other.js").expect("module emits");

        assert_eq!(one.symbol, two.symbol);
        assert_ne!(one.source, two.source);
        assert_ne!(one.symbol, other.symbol);
        assert!(!one.symbol.contains("/"));
        assert!(
            one.symbol
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        );
    }

    #[test]
    fn logical_path_symbol_rejects_non_normalized_or_absolute_input() {
        let module = module(
            program(Vec::new(), vec![Instruction::Return], 0),
            Vec::new(),
            Vec::new(),
        );
        for path in [
            "",
            "/src/dep.js",
            "../dep.js",
            "src/../dep.js",
            "src\\dep.js",
            "C:/dep.js",
        ] {
            assert!(
                matches!(
                    emit_module_with_logical_path(&module, path),
                    Err(EmitError::InvalidProgram { message }) if message.contains("logical module path")
                ),
                "path should be rejected: {path:?}"
            );
        }
    }

    #[test]
    fn rejects_invalid_module_linkage_metadata() {
        let body = program(Vec::new(), vec![Instruction::Return], 1);
        let out_of_bounds = module(
            body.clone(),
            vec![kiln_ir::ImportBinding {
                source: "dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 1,
            }],
            Vec::new(),
        );
        assert!(matches!(
            emit_module(&out_of_bounds),
            Err(EmitError::InvalidProgram { message }) if message.contains("import 0") && message.contains("out of bounds")
        ));
        let duplicate = module(
            body.clone(),
            vec![
                kiln_ir::ImportBinding {
                    source: "a".into(),
                    imported: "x".into(),
                    local: "x".into(),
                    local_slot: 0,
                },
                kiln_ir::ImportBinding {
                    source: "b".into(),
                    imported: "y".into(),
                    local: "y".into(),
                    local_slot: 0,
                },
            ],
            Vec::new(),
        );
        assert!(matches!(
            emit_module(&duplicate),
            Err(EmitError::InvalidProgram { message }) if message.contains("initialized more than once")
        ));
        let bad_export = module(
            body,
            Vec::new(),
            vec![kiln_ir::ExportBinding {
                exported: "value".into(),
                local_slot: 1,
            }],
        );
        assert!(matches!(
            emit_module(&bad_export),
            Err(EmitError::InvalidProgram { message }) if message.contains("export 0") && message.contains("out of bounds")
        ));
    }

    #[test]
    fn emitted_module_initializes_imports_and_publishes_exports() {
        let generated = emit_module(&module(
            program(
                vec![Constant::Number(2.0f64.to_bits())],
                vec![
                    Instruction::LoadLocal(0),
                    Instruction::Const(0),
                    Instruction::Binary(BinaryOp::Add),
                    Instruction::StoreLocal(1),
                    Instruction::LoadLocal(1),
                    Instruction::Return,
                ],
                2,
            ),
            vec![kiln_ir::ImportBinding {
                source: "dep".into(),
                imported: "value".into(),
                local: "input".into(),
                local_slot: 0,
            }],
            vec![kiln_ir::ExportBinding {
                exported: "answer".into(),
                local_slot: 1,
            }],
        ))
        .expect("module emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value input = kiln_c99_number(40.0); kiln_value output; kiln_value result; if ({}(NULL, &result, &input, 1, &output, 1) != 0) return 2; if (output.tag != KILN_VALUE_NUMBER || output.bits != kiln_c99_number(42.0).bits) return 3; if ({}(NULL, &result, NULL, 0, &output, 1) != KILN_C99_INVALID_ARGUMENT) return 4; return 0; }}\n",
            generated.symbol, generated.symbol
        );
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-x", "c", "-", "-lm", "-o"])
            .arg(std::env::temp_dir().join(format!("kiln-backend-c-module-{}", std::process::id())))
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(format!("{}{}", generated.source, wrapper).as_bytes())
            .expect("source is accepted by cc stdin");
        let output_path =
            std::env::temp_dir().join(format!("kiln-backend-c-module-{}", std::process::id()));
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated module C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&output_path)
            .output()
            .expect("generated executable runs");
        let _ = std::fs::remove_file(output_path);
        assert!(
            run.status.success(),
            "generated module returned the wrong result"
        );
    }

    #[test]
    fn emits_every_scalar_constant_and_host_call_through_numeric_abi() {
        let program = program(
            vec![
                Constant::Undefined,
                Constant::Null,
                Constant::Boolean(true),
                Constant::Number((-0.0f64).to_bits()),
                Constant::String("a\0é".into()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::Const(4),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 5,
                },
                Instruction::Return,
            ],
            0,
        );
        let generated = emit(&program).expect("program emits").source;
        assert!(generated.contains("UINT16_C(1), UINT16_C(5)"));
        assert!(generated.contains("typedef int (*kiln_host_call_fn)"));
        assert!(generated.contains("char kiln_module_") || generated.contains("_constant_4"));
        assert!(!generated.contains("printf(") || generated.contains("snprintf("));
        assert!(generated.contains("\\x00\"\""));
    }

    #[test]
    fn rejects_bad_versions_and_stack_shapes_explicitly() {
        let bad_version = Program {
            version: BYTECODE_VERSION + 1,
            ..program(Vec::new(), vec![Instruction::Return], 0)
        };
        assert!(matches!(
            emit(&bad_version),
            Err(EmitError::Unsupported { .. })
        ));
        let underflow = program(Vec::new(), vec![Instruction::Pop, Instruction::Return], 0);
        assert!(matches!(
            emit(&underflow),
            Err(EmitError::InvalidProgram { .. })
        ));
    }

    #[test]
    fn rejects_out_of_bounds_jumps_and_locals() {
        let jump = program(Vec::new(), vec![Instruction::Jump(9)], 0);
        assert!(matches!(emit(&jump), Err(EmitError::InvalidProgram { .. })));
        let local = program(
            Vec::new(),
            vec![Instruction::LoadLocal(0), Instruction::Return],
            0,
        );
        assert!(matches!(
            emit(&local),
            Err(EmitError::InvalidProgram { .. })
        ));
    }

    #[test]
    fn hash_changes_for_semantically_distinct_programs() {
        let one = emit(&program(
            vec![Constant::Number(1)],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ))
        .unwrap();
        let two = emit(&program(
            vec![Constant::Number(2)],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ))
        .unwrap();
        assert_ne!(one.program_hash, two.program_hash);
        assert_ne!(one.symbol, two.symbol);
    }

    #[test]
    fn generated_translation_unit_is_valid_c99() {
        let generated = emit(&program(
            vec![
                Constant::Number(2.0f64.to_bits()),
                Constant::String("hello\0world".into()),
            ],
            vec![
                Instruction::Const(1),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::Add),
                Instruction::Return,
            ],
            0,
        ))
        .expect("program emits");
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-fsyntax-only", "-x", "c", "-"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(generated.source.as_bytes())
            .expect("source is accepted by cc stdin");
        let result = child.wait_with_output().expect("cc exits");
        assert!(
            result.status.success(),
            "generated C did not compile:\n{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn executable_relational_comparison_uses_utf16_code_units() {
        let generated = emit(&program(
            vec![
                Constant::String("😀".into()),
                Constant::String("\u{e000}".into()),
                Constant::String("é".into()),
                Constant::String("e".into()),
                Constant::Boolean(true),
                Constant::Boolean(false),
            ],
            vec![
                // U+1F600 is encoded as D83D DE00, so it sorts before U+E000 in UTF-16.
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::LessThan),
                Instruction::JumpIfFalse(11),
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::Binary(BinaryOp::GreaterThan),
                Instruction::JumpIfFalse(11),
                Instruction::Const(4),
                Instruction::Return,
                Instruction::Const(5),
                Instruction::Return,
            ],
            0,
        ))
        .expect("program emits");
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-utf16-{}-{}",
            std::process::id(),
            generated.program_hash
        ));
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != 0) return 2; return !(result.tag == KILN_VALUE_BOOLEAN && result.bits == 1); }}\n",
            generated.symbol
        );
        let mut child = Command::new("cc")
            .args(["-std=c99", "-pedantic", "-x", "c", "-", "-lm", "-o"])
            .arg(&executable)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for backend tests");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(format!("{}{}", generated.source, wrapper).as_bytes())
            .expect("source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("generated executable runs");
        let _ = std::fs::remove_file(&executable);
        assert!(
            run.status.success(),
            "UTF-16 relational comparison returned the wrong result"
        );
    }
}
