//! Deterministic C99 emission for Kiln's scalar bytecode.
//!
//! This crate is deliberately a backend, not a C runtime.  The generated translation unit owns
//! the value representation and calls a host callback for every [`kiln_ir::Instruction::HostCall`]
//! using the numeric [`kiln_host_abi::HostOp`] value.  In particular, it never writes stdout (or
//! otherwise bakes CLI policy into an object file).

use std::{collections::HashSet, fmt::Write as _};

use kiln_host_abi::{HOST_ABI_VERSION, HostOp};
use kiln_ir::{
    BYTECODE_VERSION, BinaryOp, Constant, FunctionProto, Instruction, ModuleProgram, Program,
    UnaryOp, Upvalue,
};
use kiln_native_number::{NUMBER_FORMAT_VERSION, NUMBER_PARSE_VERSION};

/// Version of the generated C ABI.  This is independent from the bytecode and HostOps versions.
pub const C_ABI_VERSION: u32 = 1;

const _: () = assert!(NUMBER_FORMAT_VERSION == 1);
const _: () = assert!(NUMBER_PARSE_VERSION == 1);

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
    validate_module(module)?;
    let (program_hash, symbol_hash) = hash_module_pair(module);
    let symbol = format!("kiln_module_{program_hash:016x}{symbol_hash:016x}");
    let source = render_module(module, program_hash, &symbol);
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
    validate_module(module)?;
    let (program_hash, _) = hash_module_pair(module);
    let (path_hash, path_hash_seeded) = hash_logical_path_pair(logical_path);
    let symbol = format!("kiln_module_path_{path_hash:016x}{path_hash_seeded:016x}");
    let source = render_module(module, program_hash, &symbol);
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
#include <ctype.h>
#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* C ABI version 1; HostOps ABI version 1. */
#define KILN_NUMBER_FORMAT_VERSION 1
#define KILN_NUMBER_PARSE_VERSION 1
#define KILN_C99_OK 0
#define KILN_C99_INVALID_ARGUMENT (-1)
#define KILN_C99_CONTROL_ERROR (-2)
#define KILN_C99_HOST_ERROR (-3)
#define KILN_C99_OUT_OF_MEMORY (-4)
#define KILN_C99_CALL_DEPTH_ERROR (-5)
#define KILN_C99_UNINITIALIZED_ERROR (-6)
/* A value the program threw, travelling outward until a handler takes it.  Distinct from every
 * other status because those mean "this cannot be done" and this one means "the program said
 * so": a `catch` takes this and must not take the others. */
#define KILN_C99_THROWN (-7)

/* Recursion runs on the C stack, where exhausting it aborts the process instead of reporting
 * anything.  The interpreter's own limit is 10000 frames, but its frames are heap-allocated and
 * these are not: a generated body holds its stack and locals as automatic arrays, so the safe
 * bound is far lower and has to be enforced rather than hoped for. */
#define KILN_C99_MAX_CALL_DEPTH 256

enum kiln_value_tag {
    KILN_VALUE_UNDEFINED = 0,
    KILN_VALUE_NULL = 1,
    KILN_VALUE_BOOLEAN = 2,
    KILN_VALUE_NUMBER = 3,
    KILN_VALUE_STRING = 4,
    /* The two tags below are internal to a generated unit and never cross the embedding
     * boundary: a function is converted to text before any host call and before the module
     * result, and a cell is never a program-visible value at all.  That is what keeps
     * KILN_C_ABI_VERSION at 1 -- the boundary value is still exactly tags 0 through 4. */

    /* bits holds a kiln_closure*. */
    KILN_VALUE_FUNCTION = 5,
    /* A shared box; bits holds a kiln_value*.  It appears only in a local slot the compiler
     * marked captured, and in a closure's upvalue array. */
    KILN_VALUE_CELL = 6,
    /* A `let` or `const` slot before its declaration runs.  Reading one is the temporal dead
     * zone, and it is a distinct state from undefined: `let x; x` is undefined, `x; let x` is
     * an error.  Only slots the program actually marks are ever tested for it. */
    KILN_VALUE_UNINITIALIZED = 7,
    /* A callable the runtime supplies rather than the program.  bits holds the builtin's code
     * below.  It is a separate tag from a closure because it has no body to point at and no
     * captures to carry. */
    KILN_VALUE_BUILTIN = 8,
    /* bits holds a kiln_object*.  Objects and arrays are separate tags because `typeof` and the
     * two index paths differ; they share nothing but the tag space. */
    KILN_VALUE_OBJECT = 9,
    KILN_VALUE_ARRAY = 10,
    /* A method the runtime supplies.  bits holds its code, and *not* its receiver: `a.map` and
     * `b.map` are the same function object in the language, which is what makes
     * `a.map === b.map` true, and `CallMethod` keeps the receiver on the stack anyway. */
    KILN_VALUE_METHOD = 11,
    /* bits holds a kiln_generator*: a body that has been started and not finished. */
    KILN_VALUE_GENERATOR = 12,
    /* bits holds a double* -- an instant in milliseconds since the epoch. A pointer rather than
     * the value itself because a Date is mutable: `setFullYear` writes through it. */
    KILN_VALUE_DATE = 13,
    /* An array slot that was never written, or was deleted.
     *
     * It *reads* as undefined, which is why the marker must never escape into an expression --
     * but it is not one: `1 in [1, , 3]` is false where `1 in [1, undefined, 3]` is true, a
     * console shows `<1 empty item>`, and every element-walking method skips it. */
    KILN_VALUE_HOLE = 14,
    /* bits holds a kiln_collection*. A Map and a Set are one tag because they are one structure:
     * a Set is a Map whose values are never read. */
    KILN_VALUE_COLLECTION = 15,
    /* bits holds a kiln_promise*: a value that arrives once, later. */
    KILN_VALUE_PROMISE = 16,
    /* The `resolve` and `reject` an executor is handed. bits holds a kiln_settler*. */
    KILN_VALUE_SETTLER = 17,
    /* A callable with its receiver fixed. bits holds a kiln_bound*.
     *
     * Needed because a promise reaction is a handler *and* the thing it runs against, and the
     * queue can only hold one value per slot -- so the pair has to be one value. */
    KILN_VALUE_BOUND = 18,
    /* A property key with an identity of its own. bits holds a kiln_symbol*. */
    KILN_VALUE_SYMBOL = 19,
    /* A compiled pattern and its cursor. bits holds a kiln_regexp*. */
    KILN_VALUE_REGEXP = 20,
    /* An arbitrary-precision integer. bits holds the provider's opaque handle. */
    KILN_VALUE_BIGINT = 21
};

#define KILN_C99_BUILTIN_PROMISE 15
#define KILN_C99_BUILTIN_BOOLEAN 16
#define KILN_C99_BUILTIN_REFLECT 17
/* `Object.prototype`, which a program reaches for its methods rather than as an object:
 * `Object.prototype.hasOwnProperty.call(x, k)` is how every transpiled module asks whether a key
 * is own. It answers the three names it carries and nothing else -- it is not yet the object the
 * chain actually ends at, which is what `Object.getPrototypeOf` would need. */
#define KILN_C99_BUILTIN_OBJECT_PROTOTYPE 18
/* ...and the same for the two whose methods a program borrows most:
 * `Array.prototype.slice.call(arguments)` is how a transpiled module makes an array. */
#define KILN_C99_BUILTIN_ARRAY_PROTOTYPE 19
#define KILN_C99_BUILTIN_STRING_PROTOTYPE 20
#define KILN_C99_BUILTIN_SYMBOL 21
#define KILN_C99_BUILTIN_REGEXP 22
/* The rest of the prototype objects a program borrows a method from.
 * `RegExp.prototype.test.call(re, s)` is how a library calls one without trusting the receiver. */
#define KILN_C99_BUILTIN_REGEXP_PROTOTYPE 23
#define KILN_C99_BUILTIN_DATE_PROTOTYPE 24
#define KILN_C99_BUILTIN_NUMBER_PROTOTYPE 25
#define KILN_C99_BUILTIN_FUNCTION_PROTOTYPE 26
/* `Function`, reached for its `prototype` rather than to construct. */
#define KILN_C99_BUILTIN_FUNCTION 27
/* `BigInt`, which converts and never constructs: `new BigInt(1)` is a TypeError in the
 * specification, and the reason is that a BigInt is a primitive. */
#define KILN_C99_BUILTIN_BIGINT 28

/* Array methods, part two.
 *
 * Numbered here rather than beside the first eleven because the block that answers a *namespace*
 * method is guarded by `code >= KILN_C99_METHOD_MATH_FLOOR`, and everything reached through an
 * array receiver has to stay outside it -- as is the block that answers a *string* method. Both
 * guards now end at `KILN_C99_METHOD_ARRAY_AT`, so a new receiver-free method goes below that
 * boundary and a new array method goes above it. The gap is deliberate: none of these codes
 * reaches a cache key, but renumbering the block would still be a diff nobody could review. */
#define KILN_C99_METHOD_ARRAY_AT 200
#define KILN_C99_METHOD_ARRAY_FIND 201
#define KILN_C99_METHOD_ARRAY_FIND_INDEX 202
#define KILN_C99_METHOD_ARRAY_SOME 203
#define KILN_C99_METHOD_ARRAY_EVERY 204
#define KILN_C99_METHOD_ARRAY_SORT 205
#define KILN_C99_METHOD_ARRAY_FLAT 206
#define KILN_C99_METHOD_ARRAY_ENTRIES 207
#define KILN_C99_METHOD_ARRAY_KEYS 208
#define KILN_C99_METHOD_ARRAY_REVERSE 209
#define KILN_C99_METHOD_ARRAY_SHIFT 210
#define KILN_C99_METHOD_ARRAY_UNSHIFT 211
#define KILN_C99_METHOD_ARRAY_FILL 212
#define KILN_C99_METHOD_ARRAY_LAST_INDEX_OF 213
#define KILN_C99_METHOD_ARRAY_SPLICE 214
#define KILN_C99_METHOD_ARRAY_COPY_WITHIN 215
#define KILN_C99_METHOD_ARRAY_TO_STRING 216
#define KILN_C99_METHOD_ARRAY_FLAT_MAP 217
#define KILN_C99_METHOD_ARRAY_FIND_LAST 218
#define KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX 219
#define KILN_C99_METHOD_ARRAY_REDUCE_RIGHT 220

/* String methods, part two, numbered above the array block for the same reason the array block is
 * numbered above the namespace one: the guard that routes a code to a receiver kind is a range,
 * and a new range is cheaper than renumbering an old one. */
#define KILN_C99_METHOD_STRING_CODE_POINT_AT 240
#define KILN_C99_METHOD_STRING_SUBSTR 241
#define KILN_C99_METHOD_STRING_LOCALE_COMPARE 242
#define KILN_C99_METHOD_STRING_NORMALIZE 243
#define KILN_C99_METHOD_STRING_TO_STRING 244
#define KILN_C99_METHOD_STRING_VALUE_OF 245
/* These three only ever take a pattern, so they are intercepted before the string block: they
 * need to call back into the program, and the string block has no interpreter to call with. */
#define KILN_C99_METHOD_STRING_MATCH 246
#define KILN_C99_METHOD_STRING_MATCH_ALL 247
#define KILN_C99_METHOD_STRING_SEARCH 248

#define KILN_C99_BUILTIN_MAP 8
#define KILN_C99_BUILTIN_SET 9
/* The error constructors, contiguous so the kind is the offset: `new TypeError(m)` and
 * `new Error(m)` differ only in the name they write. */
#define KILN_C99_BUILTIN_ERROR 10
#define KILN_C99_BUILTIN_TYPE_ERROR 11
#define KILN_C99_BUILTIN_RANGE_ERROR 12
#define KILN_C99_BUILTIN_REFERENCE_ERROR 13
#define KILN_C99_BUILTIN_SYNTAX_ERROR 14

#define KILN_C99_BUILTIN_DATE 7

/* A body that yielded rather than returned.  Not an error: the caller is the generator's `next`,
 * which turns it into `{ value, done: false }`. */
#define KILN_C99_YIELDED 1

/* `String(x)`.  A template's `${x}` lowers to a call to it rather than to `+`, because the two
 * use different hints: `${x}` asks `toString` first and `+` asks `valueOf` first. */
#define KILN_C99_BUILTIN_STRING 1
#define KILN_C99_BUILTIN_MATH 2
#define KILN_C99_BUILTIN_NUMBER 3
#define KILN_C99_BUILTIN_OBJECT 4
#define KILN_C99_BUILTIN_ARRAY 5
#define KILN_C99_BUILTIN_JSON 6

/* Array methods.  Stable within a generated unit only -- unlike the instruction and constant
 * tags, none of these reaches a cache key. */
#define KILN_C99_METHOD_PUSH 1
#define KILN_C99_METHOD_POP 2
#define KILN_C99_METHOD_JOIN 3
#define KILN_C99_METHOD_INDEX_OF 4
#define KILN_C99_METHOD_INCLUDES 5
#define KILN_C99_METHOD_SLICE 6
#define KILN_C99_METHOD_CONCAT 7
#define KILN_C99_METHOD_MAP 8
#define KILN_C99_METHOD_FILTER 9
#define KILN_C99_METHOD_FOR_EACH 10
#define KILN_C99_METHOD_REDUCE 11

/* String methods.  `slice` and `indexOf` are shared with the array table by name and told apart
 * by the receiver, exactly as the language does. */
#define KILN_C99_METHOD_STRING_SLICE 12
#define KILN_C99_METHOD_SUBSTRING 13
#define KILN_C99_METHOD_STRING_INDEX_OF 14
#define KILN_C99_METHOD_LAST_INDEX_OF 15
#define KILN_C99_METHOD_STRING_INCLUDES 16
#define KILN_C99_METHOD_STARTS_WITH 17
#define KILN_C99_METHOD_ENDS_WITH 18
#define KILN_C99_METHOD_CHAR_AT 19
#define KILN_C99_METHOD_CHAR_CODE_AT 20
#define KILN_C99_METHOD_STRING_AT 21
#define KILN_C99_METHOD_TRIM 22
#define KILN_C99_METHOD_TRIM_START 23
#define KILN_C99_METHOD_TRIM_END 24
#define KILN_C99_METHOD_SPLIT 25
#define KILN_C99_METHOD_REPEAT 26
#define KILN_C99_METHOD_PAD_START 27
#define KILN_C99_METHOD_PAD_END 28
#define KILN_C99_METHOD_TO_UPPER_CASE 29
#define KILN_C99_METHOD_TO_LOWER_CASE 30
#define KILN_C99_METHOD_REPLACE 31
#define KILN_C99_METHOD_REPLACE_ALL 32
#define KILN_C99_METHOD_STRING_CONCAT 33

/* Namespace methods.  Unlike the array and string tables these take no receiver: `Math.floor` is
 * reached through `Math`, but `Math` contributes nothing to the answer. */
#define KILN_C99_METHOD_MATH_FLOOR 34
#define KILN_C99_METHOD_MATH_CEIL 35
#define KILN_C99_METHOD_MATH_ROUND 36
#define KILN_C99_METHOD_MATH_TRUNC 37
#define KILN_C99_METHOD_MATH_ABS 38
#define KILN_C99_METHOD_MATH_SIGN 39
#define KILN_C99_METHOD_MATH_SQRT 40
#define KILN_C99_METHOD_MATH_CBRT 41
#define KILN_C99_METHOD_MATH_POW 42
#define KILN_C99_METHOD_MATH_MIN 43
#define KILN_C99_METHOD_MATH_MAX 44
#define KILN_C99_METHOD_MATH_LOG 45
#define KILN_C99_METHOD_MATH_LOG2 46
#define KILN_C99_METHOD_MATH_LOG10 47
#define KILN_C99_METHOD_MATH_EXP 48
#define KILN_C99_METHOD_MATH_HYPOT 49
#define KILN_C99_METHOD_MATH_ATAN2 50
#define KILN_C99_METHOD_MATH_SIN 51
#define KILN_C99_METHOD_MATH_COS 52
#define KILN_C99_METHOD_MATH_TAN 53
#define KILN_C99_METHOD_MATH_ASIN 54
#define KILN_C99_METHOD_MATH_ACOS 55
#define KILN_C99_METHOD_MATH_ATAN 56
#define KILN_C99_METHOD_IS_INTEGER 57
#define KILN_C99_METHOD_IS_SAFE_INTEGER 58
#define KILN_C99_METHOD_IS_NAN 59
#define KILN_C99_METHOD_IS_FINITE 60
#define KILN_C99_METHOD_OBJECT_KEYS 61
#define KILN_C99_METHOD_OBJECT_VALUES 62
#define KILN_C99_METHOD_OBJECT_ENTRIES 63
#define KILN_C99_METHOD_OBJECT_ASSIGN 64
#define KILN_C99_METHOD_HAS_OWN 65
#define KILN_C99_METHOD_IS_ARRAY 66
#define KILN_C99_METHOD_ARRAY_OF 67
#define KILN_C99_METHOD_OBJECT_CREATE 68
#define KILN_C99_METHOD_JSON_STRINGIFY 69
#define KILN_C99_METHOD_JSON_PARSE 70
#define KILN_C99_METHOD_GENERATOR_NEXT 71
#define KILN_C99_METHOD_GENERATOR_RETURN 72
#define KILN_C99_METHOD_GENERATOR_SELF 73

/* Date readers. Every one of these is UTC, because the realm has one zone and it is UTC
 * (ADR 0015) -- so the local reader and the UTC reader are the same function rather than two
 * that happen to agree. */
#define KILN_C99_METHOD_GET_TIME 74
#define KILN_C99_METHOD_GET_FULL_YEAR 75
#define KILN_C99_METHOD_GET_MONTH 76
#define KILN_C99_METHOD_GET_DATE 77
#define KILN_C99_METHOD_GET_DAY 78
#define KILN_C99_METHOD_GET_HOURS 79
#define KILN_C99_METHOD_GET_MINUTES 80
#define KILN_C99_METHOD_GET_SECONDS 81
#define KILN_C99_METHOD_GET_MILLISECONDS 82
#define KILN_C99_METHOD_GET_TIMEZONE_OFFSET 83
#define KILN_C99_METHOD_TO_ISO_STRING 84
#define KILN_C99_METHOD_TO_JSON 85
#define KILN_C99_METHOD_DATE_TO_STRING 86
#define KILN_C99_METHOD_DATE_NOW 87
#define KILN_C99_METHOD_DATE_UTC 88
#define KILN_C99_METHOD_DATE_PARSE 89

/* The end of every chain is `Object.prototype`, and these are what it carries. */
#define KILN_C99_METHOD_HAS_OWN_PROPERTY 90
#define KILN_C99_METHOD_OBJECT_TO_STRING 91
#define KILN_C99_METHOD_OBJECT_VALUE_OF 92

/* Map and Set. `size` is a property rather than a method, so it is answered by the read. */
#define KILN_C99_METHOD_COLLECTION_GET 93
#define KILN_C99_METHOD_COLLECTION_SET 94
#define KILN_C99_METHOD_COLLECTION_ADD 95
#define KILN_C99_METHOD_COLLECTION_HAS 96
#define KILN_C99_METHOD_COLLECTION_DELETE 97
#define KILN_C99_METHOD_COLLECTION_CLEAR 98
#define KILN_C99_METHOD_COLLECTION_FOR_EACH 99
#define KILN_C99_METHOD_COLLECTION_KEYS 100
#define KILN_C99_METHOD_COLLECTION_VALUES 101
#define KILN_C99_METHOD_COLLECTION_ENTRIES 102
#define KILN_C99_METHOD_COLLECTION_SELF 103
#define KILN_C99_METHOD_ITERATOR_NEXT 104
#define KILN_C99_METHOD_ITERATOR_SELF 105
#define KILN_C99_METHOD_ARRAY_VALUES 106

/* Promise, and the two halves of driving an async function. */
#define KILN_C99_METHOD_THEN 107
#define KILN_C99_METHOD_CATCH 108
#define KILN_C99_METHOD_FINALLY 109
#define KILN_C99_METHOD_PROMISE_RESOLVE 110
#define KILN_C99_METHOD_PROMISE_REJECT 111
#define KILN_C99_METHOD_PROMISE_ALL 112
#define KILN_C99_METHOD_PROMISE_RACE 113
#define KILN_C99_METHOD_ASYNC_RESUME 114
#define KILN_C99_METHOD_ASYNC_THROW 115
#define KILN_C99_METHOD_ALL_STEP 116

/* The global functions. They are namespace methods with no namespace: nothing is reached through
 * before them, so `LoadBuiltin` pushes the method value directly. Two of them share a name with a
 * `Number` method and not its behaviour -- `isNaN('x')` coerces where `Number.isNaN('x')` does
 * not -- so they are separate codes rather than the same one reached two ways. */
#define KILN_C99_METHOD_PARSE_INT 117
#define KILN_C99_METHOD_PARSE_FLOAT 118
#define KILN_C99_METHOD_GLOBAL_IS_NAN 119
#define KILN_C99_METHOD_GLOBAL_IS_FINITE 120

/* Number methods. Read off the value rather than through a `Number.prototype` object, which is
 * what the interpreter does and for the same reason: there is no such object yet. */
#define KILN_C99_METHOD_NUMBER_TO_STRING 121
#define KILN_C99_METHOD_NUMBER_TO_FIXED 122
#define KILN_C99_METHOD_NUMBER_VALUE_OF 123

/* The text-escaping globals. `escape`/`unescape` are Annex B and work in code units; the four URI
 * functions work in UTF-8 bytes and differ only in which characters they leave alone. */
#define KILN_C99_METHOD_ESCAPE 124
#define KILN_C99_METHOD_UNESCAPE 125
#define KILN_C99_METHOD_ENCODE_URI 126
#define KILN_C99_METHOD_ENCODE_URI_COMPONENT 127
#define KILN_C99_METHOD_DECODE_URI 128
#define KILN_C99_METHOD_DECODE_URI_COMPONENT 129

/* `queueMicrotask(fn)`: the queue promises already drain, reached directly. */
#define KILN_C99_METHOD_QUEUE_MICROTASK 130

/* The timer functions. `clearTimeout` and `clearInterval` do the same thing and are two codes
 * because they are two names: a program can print either one. */
#define KILN_C99_METHOD_SET_TIMEOUT 131
#define KILN_C99_METHOD_SET_INTERVAL 132
#define KILN_C99_METHOD_CLEAR_TIMEOUT 133
#define KILN_C99_METHOD_CLEAR_INTERVAL 134

/* The two combinators that do not fail on the first rejection. Named outside the contiguous
 * `Promise` range because that range is how a namespace's methods are found, and it was already
 * closed. `@@rejectStep` is the rejection-side twin of `@@allStep`: `all` and `race` settle their
 * promise on any rejection and need no handler of their own, and these two do. */
#define KILN_C99_METHOD_PROMISE_ALL_SETTLED 135
#define KILN_C99_METHOD_PROMISE_ANY 136
#define KILN_C99_METHOD_REJECT_STEP 137

/* The two halves of `finally`, bound to the handler. They run it and then restore the outcome
 * they were passing through, which is the whole difference from `then(f, f)`. */
#define KILN_C99_METHOD_FINALLY_PASS 138
#define KILN_C99_METHOD_FINALLY_RAISE 139
/* ...and the two that put the outcome back once the handler's own result has settled. Bound to
 * the outcome, so the pair needs no state object either. */
#define KILN_C99_METHOD_FINALLY_RESTORE 140
#define KILN_C99_METHOD_FINALLY_RERAISE 141

/* Resolving a promise *with* a promise. The attach is a job of its own, which is a turn a program
 * can see -- so it is a step rather than something `settle` does on the spot. */
#define KILN_C99_METHOD_ADOPT_STEP 142

/* `Object.freeze` and `Object.isFrozen`, outside the contiguous `Object` range because it was
 * already closed. */
#define KILN_C99_METHOD_OBJECT_FREEZE 143
#define KILN_C99_METHOD_OBJECT_IS_FROZEN 144

/* What every callable answers to, whatever kind of callable it is:
 * `Object.prototype.hasOwnProperty.call(x, k)` is how a transpiled module asks whether a key is
 * own, and `Array.prototype.slice.call(arguments)` is how it makes an array. */
#define KILN_C99_METHOD_FUNCTION_CALL 145
#define KILN_C99_METHOD_FUNCTION_APPLY 146
#define KILN_C99_METHOD_FUNCTION_BIND 147

/* The reflective statics, and the one instance method that walks a chain. */
#define KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF 148
#define KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF 149
#define KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES 150
#define KILN_C99_METHOD_OBJECT_FROM_ENTRIES 151
#define KILN_C99_METHOD_ARRAY_FROM 152
#define KILN_C99_METHOD_IS_PROTOTYPE_OF 153
#define KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE 154

/* `Math`, part two -- numbered here rather than beside the first range because that range ends
 * where `Number`'s begins. Still below the array boundary, because these take no receiver. */
#define KILN_C99_METHOD_MATH_RANDOM 160
#define KILN_C99_METHOD_MATH_SINH 161
#define KILN_C99_METHOD_MATH_COSH 162
#define KILN_C99_METHOD_MATH_TANH 163
#define KILN_C99_METHOD_MATH_ASINH 164
#define KILN_C99_METHOD_MATH_ACOSH 165
#define KILN_C99_METHOD_MATH_ATANH 166
#define KILN_C99_METHOD_MATH_LOG1P 167
#define KILN_C99_METHOD_MATH_EXPM1 168
#define KILN_C99_METHOD_MATH_FROUND 169
#define KILN_C99_METHOD_MATH_CLZ32 170
#define KILN_C99_METHOD_MATH_IMUL 171

/* What a regular expression answers to. Below the array boundary because they take a receiver
 * exactly as the number methods do. */
#define KILN_C99_METHOD_REGEXP_TEST 172
#define KILN_C99_METHOD_REGEXP_EXEC 173
#define KILN_C99_METHOD_REGEXP_TO_STRING 174
/* A collection's own `next`, distinct from the array iterator's because it walks the collection
 * itself rather than a copy of it -- which is the only way a delete during a loop can be seen.
 * Numbered here rather than beside the collection block because the guard that routes a code to
 * a receiver kind is a range, and this has to land inside the one that answers a method reached
 * through an object. */
#define KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT 175
/* `toString` on a string or a boolean, which is the value itself rather than `[object String]`.
 * Library code reaches for it constantly on values it has already coerced. */
#define KILN_C99_METHOD_PRIMITIVE_TO_STRING 176

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

/* A closure is its body plus the boxes it captured.
 *
 * The body is a function pointer rather than an index into the emitting program's table,
 * because a function does not have to be called by the unit that made it: a module exports one,
 * another module imports and calls it, and an index would then be resolved against the wrong
 * table.  `proto` is kept only to name the function in text -- it is the same index the
 * interpreter uses, so both engines print `[Function 3]` for the same function. */
struct kiln_closure;
struct kiln_generator;
typedef int (*kiln_body_fn)(kiln_host *host, kiln_value *result, kiln_value *upvalues,
                            kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                            kiln_value *thrown, struct kiln_generator *frame);
typedef struct kiln_closure {
    kiln_body_fn body;
    /* An arrow captures `new.target` where it was written, for the same reason it captures
     * `this`: it has none of its own. */
    int arrow;
    kiln_value captured_new_target;
    /* ...and its `arguments`, which is the enclosing function's list rather than its own. */
    kiln_value captured_arguments;
    /* Calling this builds a generator object instead of running the body. */
    int generator;
    /* ...and, when this is set, drives it to completion against a promise rather than handing
     * the object to the program. */
    int asynchronous;
    /* ...unless it is *both*, in which case the object is handed over after all and each `next`
     * gets a promise of its own. */
    int async_generator;
    /* What a generator's frame has to be big enough for, known when the body is emitted. */
    uint32_t local_count;
    uint32_t stack_size;
    uint32_t handler_depth;
    /* The object `new f()` links instances to, and the store `f.x = 1` writes into. Both are
     * created on first use: an ordinary function that is never used as a constructor and never
     * carries a property allocates neither. */
    kiln_value prototype;
    kiln_value properties;
    /* For the console, which prints `[Function: name]`.  NULL for an anonymous function, which
     * prints `[Function (anonymous)]` -- a distinct answer, not a missing one. */
    const char *name;
    size_t name_len;
    uint32_t proto;
    uint32_t upvalue_count;
    kiln_value *upvalues;
} kiln_closure;

/* A plain data object: own string-keyed properties, scanned linearly.
 *
 * There is deliberately no shape here.  A shape identity has to be globally stable and never
 * recycled to be sound (see ADR 0021), and inventing one before there is anything to guard is
 * how that goes wrong; a list has no identity to get wrong.  It is also uniformly slow rather
 * than fast-until-megamorphic, which is the honest starting point.
 *
 * Keys point at the generated constant table or at heap bytes from a computed key, and are
 * compared by content, never by pointer -- two spellings of one name must be one property. */
/* A property is a value, or a pair of functions.  Reading an accessor *calls* one, which is why
 * every program-visible read has to go through the runtime rather than being a struct field
 * access -- and why `kiln_c99_get` takes a host and a depth. */
typedef struct kiln_property {
    const char *key;
    size_t key_len;
    /* Whether `Object.keys`, `for...in`, spread and JSON see it. Every own property of an error
     * is readable by name and not enumerated, which is why `JSON.stringify(err)` is `{}`. */
    int enumerable;
    int accessor;
    kiln_value value;
    kiln_value getter;
    kiln_value setter;
} kiln_property;

typedef struct kiln_object {
    kiln_property *properties;
    size_t count;
    size_t capacity;
    /* The next object a property read falls through to, or undefined for none.  Undefined and
     * null are different answers here, exactly as they are to `Object.getPrototypeOf`: one means
     * nothing was recorded and the other means nothing was wanted. */
    kiln_value prototype;
    /* `Object.freeze`. A frozen object ignores every write rather than refusing one: modules are
     * strict and a real engine would throw, but the interpreter is silent here and the two have
     * to agree before either can change. */
    int frozen;
} kiln_object;

/* A prototype chain is built by the program, so its length is the program's business -- but the
 * walk is on the C stack, where running out aborts rather than reports. Bounded, like the
 * interpreter's. */
#define KILN_C99_MAX_PROTOTYPE_DEPTH 10000

/* An array's elements are dense and its `length` is derived, not stored: writing past the end
 * grows it and the gap is filled with holes, which read as undefined and are not the same thing --
 * `1 in [1, , 3]` is false and `[1, , 3].map(f)` does not call `f` for the middle. */
typedef struct kiln_array {
    kiln_value *elements;
    size_t count;
    size_t capacity;
    /* Properties that are not elements: `a.total = 1`, and the `index`, `input` and `groups` a
     * match result carries. Undefined until the first one is written, so an ordinary array is one
     * pointer smaller than it looks and costs nothing. */
    kiln_value properties;
    /* `Object.freeze`. An element write is ignored, like a property write on a frozen object;
     * a method that would change the length throws, because there is no way to do half of it. */
    int frozen;
} kiln_array;

/* A suspended body.
 *
 * A generator does not run when it is called: it hands back this, and each `next` resumes the
 * body where it left off. An automatic C frame cannot be suspended -- it is gone the moment the
 * function returns -- so a generator's locals and operand stack live here instead, and the body
 * is emitted to read them from here rather than from its own frame.
 *
 * Re-entry is a `switch` on `resume` at the top of the body, jumping to a label emitted directly
 * after the `Yield` that produced it. That works because the emitted body is already a flat
 * sequence of labels with `goto` between them: the shape a bytecode interpreter has, and the
 * shape that makes resuming possible at all. */
typedef struct kiln_generator {
    kiln_value function;
    kiln_value receiver;
    /* An async function is compiled as a generator -- `await` suspends exactly as `yield` does --
     * and this is the promise its eventual return value settles. Undefined for a plain one. */
    kiln_value promise;
    int is_async;
    kiln_value *locals;
    kiln_value *stack;
    size_t sp;
    /* Which `Yield` to come back to. Zero before the first `next`, when the body starts at its
     * beginning rather than resuming. */
    size_t resume;
    /* The handler stack, which is part of the state a suspension has to survive: a `try` that
     * stopped catching across a `yield` would surface three frames away. Owned here rather than
     * being automatic, for exactly the reason the operand stack is. */
    size_t *handlers;
    size_t *handler_stack;
    size_t handler_top;
    /* What `next(v)` passed in, which becomes the result of the `yield` that suspended. */
    kiln_value sent;
    /* Whether the suspension that stopped this frame was an `await` rather than a `yield`.
     *
     * They emit identically -- both save the frame and return `YIELDED` -- and for an ordinary
     * generator or an async function nothing has to tell them apart. An async generator does:
     * an `await` between two `yield`s is invisible to the consumer, because the step is not
     * finished. */
    int awaiting;
    int async_generator;
    /* Whether `sent` is a value to resume with or a rejection to raise at the suspension point.
     * A rejected `await` throws *into* the body, so a `try` around it catches -- which is the
     * whole reason the handler stack above has to survive a suspension. */
    int raising;
    int started;
    int done;
} kiln_generator;

/* The parts a civil date is made of. The realm has one zone and it is UTC (ADR 0015), so there
 * is no local variant of any of these to keep in step. */
typedef struct kiln_date_parts {
    int64_t year;
    int month;
    int day;
    int weekday;
    int hour;
    int minute;
    int second;
    int millisecond;
    int valid;
} kiln_date_parts;

/* A Map or a Set.
 *
 * Insertion-ordered and scanned linearly, for the same reason an object's properties are: a hash
 * needs a hash of a *value*, and the identity half of that is a pointer the collector will one
 * day move. Order is observable -- iteration is in insertion order and the specification says so
 * -- so the list is the honest structure and the index is what a hash would accelerate. */
typedef struct kiln_entry {
    kiln_value key;
    kiln_value value;
    /* A deleted entry is a hole, not a removal: compacting here would shift every later entry
     * past a live iterator's cursor and skip one. */
    int dead;
} kiln_entry;

typedef struct kiln_collection {
    kiln_entry *entries;
    /* How many slots have ever been used, dead ones included -- what iteration walks. */
    size_t count;
    /* How many are still there, which is what `size` reports. */
    size_t live;
    size_t capacity;
    int is_map;
} kiln_collection;

/* A promise: a value that arrives once, later, and everything waiting for it.
 *
 * `state` is pending, fulfilled or rejected, and only the first settlement counts -- a promise is
 * handed to the program as two functions and the program may call either twice. */
typedef struct kiln_reaction {
    kiln_value on_fulfilled;
    kiln_value on_rejected;
    /* The promise `then` returned, settled with whatever the handler produces. */
    kiln_value derived;
} kiln_reaction;

typedef struct kiln_promise {
    int state;
    kiln_value value;
    kiln_reaction *reactions;
    size_t count;
    size_t capacity;
} kiln_promise;

#define KILN_C99_PENDING 0
#define KILN_C99_FULFILLED 1
#define KILN_C99_REJECTED 2

typedef struct kiln_settler {
    kiln_value promise;
    int rejects;
} kiln_settler;

/* A compiled pattern, its source text, and the cursor `g` and `y` advance.
 *
 * The engine is behind the provider ABI below rather than written here (ADR 0025): it is the same
 * `regress` the interpreter runs, so the two cannot disagree about a backreference. `last_index`
 * is mutable and program-visible, which is why two evaluations of one literal must not share a
 * `kiln_regexp`. */
typedef struct kiln_regexp {
    void *handle;
    const char *source;
    size_t source_len;
    const char *flags;
    size_t flags_len;
    int global;
    int sticky;
    double last_index;
} kiln_regexp;

/* A symbol: a property key with an identity of its own.
 *
 * `key` is the reserved string the symbol names a property under, so the object model stays
 * string-keyed and nothing else has to learn about symbols. A well-known symbol gets a fixed key,
 * so two mentions of `Symbol.iterator` reach the same property; a unique one gets a serial, so two
 * symbols with the same description do not collide and a program cannot spell either by writing a
 * string. The description is for display and is never part of the key. */
typedef struct kiln_symbol {
    const char *key;
    size_t key_len;
    const char *description;
    size_t description_len;
} kiln_symbol;

typedef struct kiln_bound {
    kiln_value target;
    kiln_value receiver;
    /* `f.bind(o, 1)` fixes the 1 as well as the `o`: the bound arguments come first and the
     * call's own follow them. Owned here, because the call that supplies the rest happens
     * arbitrarily later. */
    kiln_value *arguments;
    size_t count;
} kiln_bound;

/* One entry of the microtask queue: run `handler` against `argument` and settle `derived` with
 * what it produced.  A handler that is not callable passes the value through unchanged, which is
 * what `p.then(undefined, f)` and `p.catch(f)` rely on. */
typedef struct kiln_microtask {
    kiln_value handler;
    kiln_value argument;
    kiln_value derived;
    int rejected;
} kiln_microtask;

/* One scheduled callback.
 *
 * `sequence` breaks ties between timers due at the same instant, so two `setTimeout(f, 0)` calls
 * run in the order they were made rather than in whatever order the array happens to hold them --
 * which is a difference a program can see and therefore one the two engines have to share. */
typedef struct kiln_timer {
    kiln_value callback;
    kiln_value *args;
    size_t argc;
} kiln_timer;

/* A cursor over JSON source, moved forward as text is consumed. */
typedef struct kiln_json_reader {
    const char *bytes;
    size_t len;
    size_t at;
    int depth;
} kiln_json_reader;

/* A growable byte buffer, for the operations that build a string rather than slice one. */
typedef struct kiln_text {
    char *bytes;
    size_t len;
    size_t capacity;
} kiln_text;

/* Internal link-time runtime contract. The final executable/shared library supplies these symbols
 * from the Rust provider archive; they are not part of the public embedding ABI.
 *
 * The offsets `kiln_regexp_search` reports are UTF-16 code units, because that is what the
 * language counts in (ADR 0022) and matching over UTF-16 makes them so without a conversion. */
int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
double kiln_string_to_number(const char *bytes, size_t len);
typedef int (*kiln_realm_work_fn)(void *work, void *host, void *thrown);
typedef void (*kiln_realm_drop_fn)(void *work);
int kiln_realm_is_active(void);
int kiln_realm_queue_microtask(kiln_realm_work_fn run, kiln_realm_drop_fn drop, void *work);
int kiln_realm_set_timeout(double delay_ms, kiln_realm_work_fn run, kiln_realm_drop_fn drop,
                           void *work, uint64_t *out_id);
int kiln_realm_set_interval(double delay_ms, kiln_realm_work_fn run, kiln_realm_drop_fn drop,
                            void *work, uint64_t *out_id);
int kiln_realm_clear_timer(uint64_t id);
int32_t kiln_regexp_compile(const unsigned char *pattern, size_t pattern_len,
                            const unsigned char *flags, size_t flags_len, void **out_handle);
void kiln_regexp_free(void *handle);
int32_t kiln_regexp_search(void *handle, const uint16_t *subject, size_t subject_len, size_t start,
                           int64_t *out_ranges, size_t out_capacity);
size_t kiln_regexp_group_count(const void *handle);
int32_t kiln_regexp_group_name(const void *handle, size_t index, unsigned char *buffer,
                               size_t capacity);
/* Arbitrary-precision integers. The handles are opaque: everything that knows what one contains
 * is on the other side of this boundary, which is the point (ADR 0025). */
int32_t kiln_bigint_parse(const unsigned char *text, size_t len, uint32_t radix, void **out);
int32_t kiln_bigint_from_double(double value, void **out);
void kiln_bigint_free(void *handle);
double kiln_bigint_to_double(const void *handle);
int32_t kiln_bigint_to_string(const void *handle, uint32_t radix, unsigned char *buffer,
                              size_t capacity);
int32_t kiln_bigint_binary(uint32_t operation, const void *left, const void *right, void **out);
int32_t kiln_bigint_negate(const void *handle, void **out);
int32_t kiln_bigint_compare(const void *left, const void *right);
int32_t kiln_bigint_is_zero(const void *handle);


/* Forward declarations.
 *
 * The runtime below is one long sequence of static functions, and the order they happen to be
 * written in is not the order they call each other in: a property read reaches string conversion,
 * string conversion reaches the array join, the join reaches the read again. Declaring them all
 * here once removes the ordering constraint entirely, rather than reshuffling the file every time
 * a new call crosses an old boundary. */
static kiln_value kiln_c99_value(uint32_t tag, uint64_t bits, const char *ptr, size_t len);
static kiln_value kiln_c99_undefined(void);
static kiln_value kiln_c99_null(void);
static kiln_value kiln_c99_boolean(int value);
static kiln_value kiln_c99_number(double value);
static kiln_value kiln_c99_uninitialized(void);
static kiln_value kiln_c99_builtin(uint64_t code);
static kiln_value kiln_c99_method(uint64_t code);
static kiln_value kiln_c99_function(kiln_closure *closure);
static kiln_closure *kiln_c99_closure_of(kiln_value value);
static kiln_value *kiln_c99_cell_of(kiln_value value);
static kiln_object *kiln_c99_object_of(kiln_value value);
static kiln_array *kiln_c99_array_of(kiln_value value);
static double kiln_c99_as_number(kiln_value value);
static int kiln_c99_to_number_checked(kiln_host *host, kiln_value value, double *out, int depth,
                                      kiln_value *thrown);
static int kiln_c99_truthy(kiln_value value);
static int kiln_c99_signbit(double value);
static int kiln_c99_strict_equal(kiln_value left, kiln_value right);
static int kiln_c99_is_callable(kiln_value value);
static int kiln_c99_is_reference(kiln_value value);
static int kiln_c99_crosses_boundary(kiln_value value);
static kiln_value kiln_c99_text(kiln_value value);
static kiln_value kiln_c99_copy_text(const char *text, size_t len);
static kiln_value kiln_c99_inspect(kiln_value value);
static int kiln_c99_reserve(void **items, size_t *capacity, size_t needed, size_t item_size);
static int kiln_c99_text_append(kiln_text *text, const char *bytes, size_t len);
static int kiln_c99_text_append_c(kiln_text *text, const char *literal);
static int kiln_c99_same_name(const char *key, size_t key_len, const char *name);
static int kiln_c99_same_key(const kiln_property *property, const char *key, size_t key_len);
static int kiln_c99_new_object(kiln_value *out);
static int kiln_c99_new_array(kiln_value *out);
static int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown);
static int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_get_own_kind(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_array_push(kiln_value target, kiln_value value);
static int kiln_c99_prototype_object(kiln_value function, kiln_value *out);
static int kiln_c99_to_units(kiln_value value, uint16_t **out, size_t *count);
static kiln_value kiln_c99_from_units(const uint16_t *units, size_t count);
static const char *kiln_c99_method_name(uint64_t code);
static const char *kiln_c99_builtin_name(uint64_t builtin);
static int kiln_c99_is_namespace(uint64_t builtin);
static int kiln_c99_changes_length(uint64_t code);
static uint64_t kiln_c99_callable_method(const char *key, size_t key_len);
static kiln_value kiln_c99_property_key(kiln_value key);
static uint32_t kiln_c99_to_uint32(double value);
static kiln_value kiln_c99_bigint_text(kiln_value value, uint32_t radix);
static kiln_value kiln_c99_bigint_value(void *handle);
static const void *kiln_c99_bigint_one(void);
static int kiln_c99_bigint_constant(void **cache, const char *digits, size_t len,
                                    kiln_value *out);
static int kiln_c99_expand_replacement(kiln_text *out, const uint16_t *pattern,
                                       size_t pattern_count, const uint16_t *subject,
                                       size_t subject_count, const int64_t *ranges, size_t groups);
static kiln_regexp *kiln_c99_regexp_of(kiln_value value);
static int kiln_c99_global_object(kiln_value *out);
static int kiln_c99_new_regexp(kiln_value *out, const char *source, size_t source_len,
                               const char *flags, size_t flags_len, kiln_value *thrown);
static int kiln_c99_string_compare(kiln_value left, kiln_value right);
static uint64_t kiln_c99_array_method(const char *key, size_t key_len);
static uint64_t kiln_c99_string_method(const char *key, size_t key_len);
static int kiln_c99_number_in_radix(kiln_value *out, double value, int radix);
static int kiln_c99_to_fixed(kiln_value *out, double value, int places);
static int kiln_c99_hex_digit(char character);
static int kiln_c99_append_hex(kiln_text *text, unsigned value, int width);
static int kiln_c99_valid_utf8(const char *bytes, size_t len);
static kiln_value kiln_c99_copy_text(const char *bytes, size_t len);
static int64_t kiln_c99_days_from_civil(int64_t year, int64_t month, int64_t day);
static kiln_date_parts kiln_c99_date_parts(double time);
static double kiln_c99_date_time(kiln_value value);
static int kiln_c99_new_date(kiln_value *out, double time);
static int kiln_c99_iso_string(kiln_value *out, double time);
static int kiln_c99_date_to_string(kiln_value *out, double time);
static double kiln_c99_instant_from_parts(const double parts[7]);
static double kiln_c99_parse_date(const char *bytes, size_t len);
static int kiln_c99_pad_number(kiln_text *text, int64_t value, int width);
static int kiln_c99_is_hole(kiln_value value);
static kiln_collection *kiln_c99_collection_of(kiln_value value);
static int kiln_c99_new_collection(kiln_value *out, int is_map);
static const char *kiln_c99_error_name(uint64_t builtin);
static int kiln_c99_new_error(kiln_value *out, uint64_t kind, kiln_value message);
static int kiln_c99_aggregate_error(kiln_value *out, kiln_value reasons);
static kiln_property *kiln_c99_own_private(kiln_value target, const char *key, size_t key_len);
static int kiln_c99_private_error(kiln_value *thrown, const char *key, size_t key_len,
                                  int writing);
static int kiln_c99_set_prototype(kiln_value target, kiln_value prototype);
static int kiln_c99_new_promise(kiln_value *out);
static kiln_promise *kiln_c99_promise_of(kiln_value value);
static int kiln_c99_new_settler(kiln_value *out, kiln_value promise, int rejects);
static int kiln_c99_bind(kiln_value *out, kiln_value target, kiln_value receiver);
static int kiln_c99_bind_with(kiln_value *out, kiln_value target, kiln_value receiver,
                              const kiln_value *arguments, size_t count);
static int kiln_c99_settle(kiln_value promise, kiln_value value, int rejected);
static int kiln_c99_then(kiln_value promise, kiln_value on_fulfilled, kiln_value on_rejected,
                         kiln_value *out);
static int kiln_c99_enqueue(kiln_value handler, kiln_value argument, kiln_value derived,
                            int rejected);
static int kiln_c99_run_microtask(void *work, void *host, void *thrown);
static void kiln_c99_drop_microtask(void *work);
static int kiln_c99_run_timer(void *work, void *host, void *thrown);
static void kiln_c99_drop_timer(void *work);
static int kiln_c99_schedule(kiln_promise *promise, const kiln_reaction *reaction);
static int kiln_c99_async_step(kiln_host *host, kiln_value generator, kiln_value sent,
                               int rejected, int depth, kiln_value *thrown);
static int kiln_c99_async_generator_step(kiln_host *host, kiln_value generator, kiln_value sent,
                                         int rejected, int depth, kiln_value *thrown);
static int kiln_c99_async_generator_settle(kiln_host *host, kiln_value step, kiln_value value,
                                           int finished, int depth, kiln_value *thrown);
static int kiln_c99_new_generator(kiln_value *out, kiln_value function, kiln_value receiver,
                                  const kiln_value *args, size_t argc);
static kiln_generator *kiln_c99_generator_of(kiln_value value);
static int kiln_c99_function_store(kiln_value function, kiln_value *out);
static int kiln_c99_throw_error(kiln_value *thrown, uint64_t kind, const char *message);
static int kiln_c99_binary_checked(uint16_t operation, kiln_host *host, kiln_value left,
                                   kiln_value right, kiln_value *out, int depth,
                                   kiln_value *thrown);
static int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int string_hint,
                                 kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_unary_checked(uint16_t operation, kiln_host *host, kiln_value value,
                                  kiln_value *out, int depth, kiln_value *thrown);
static int kiln_c99_new_iterator(kiln_value *out, kiln_value items);
static double kiln_c99_collection_find(kiln_collection *collection, kiln_value key);
static int kiln_c99_collection_put(kiln_value target, kiln_value key, kiln_value value);
static int kiln_c99_same_value_zero(kiln_value left, kiln_value right);
static int kiln_c99_iterate(kiln_host *host, kiln_value target, kiln_value source, int depth,
                            kiln_value *thrown);
static kiln_value kiln_c99_hole(void);
static int kiln_c99_has_property(kiln_value target, const char *key, size_t key_len, int *answer);
static int kiln_c99_delete(kiln_value target, const char *key, size_t key_len, int *answer);
static int kiln_c99_own_keys(kiln_value target, kiln_value *out);
static double kiln_c99_key_index(const char *key, size_t key_len);
static uint64_t kiln_c99_array_method(const char *key, size_t key_len);
static int kiln_c99_json_value(kiln_host *host, kiln_text *text, kiln_value value,
                               const char *indent, size_t indent_len, const char *current,
                               size_t current_len, int *serialised, int depth,
                               kiln_value *thrown);
static int kiln_c99_json_read(kiln_json_reader *reader, kiln_value *out);
static void kiln_c99_json_space(kiln_json_reader *reader);
static int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee,
                         kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                         kiln_value *thrown);
static int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown);

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

static kiln_object *kiln_c99_object_of(kiln_value value) {
    return (kiln_object *)(uintptr_t)value.bits;
}

static kiln_array *kiln_c99_array_of(kiln_value value) {
    return (kiln_array *)(uintptr_t)value.bits;
}

static int kiln_c99_new_object(kiln_value *out) {
    kiln_object *object = (kiln_object *)malloc(sizeof *object);
    if (object == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    object->properties = NULL;
    object->count = 0;
    object->capacity = 0;
    object->prototype = kiln_c99_undefined();
    object->frozen = 0;
    *out = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_array(kiln_value *out) {
    kiln_array *array = (kiln_array *)malloc(sizeof *array);
    if (array == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    array->elements = NULL;
    array->count = 0;
    array->capacity = 0;
    array->properties = kiln_c99_undefined();
    array->frozen = 0;
    *out = kiln_c99_value(KILN_VALUE_ARRAY, (uint64_t)(uintptr_t)array, NULL, 0);
    return KILN_C99_OK;
}

/* Doubling, so building an object or an array one member at a time stays linear overall. */
static int kiln_c99_reserve(void **items, size_t *capacity, size_t needed, size_t item_size) {
    size_t grown = *capacity == 0 ? 4 : *capacity;
    void *moved;
    if (needed <= *capacity) {
        return KILN_C99_OK;
    }
    while (grown < needed) {
        if (grown > ((size_t)-1) / 2 / item_size) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        grown *= 2;
    }
    moved = realloc(*items, grown * item_size);
    if (moved == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    *items = moved;
    *capacity = grown;
    return KILN_C99_OK;
}

/* The index a key names, or -1.  A key is an index only when it round-trips through the
 * canonical decimal spelling, which is what keeps `a["01"]` a named property and `a["1"]` an
 * element -- the distinction the language draws and the one a naive `atoi` erases. */
static double kiln_c99_key_index(const char *key, size_t key_len) {
    char buffer[64];
    double number;
    int length;
    if (key_len == 0 || key_len > 20) {
        return -1.0;
    }
    number = kiln_string_to_number(key, key_len);
    if (!(number >= 0.0) || floor(number) != number || number > 4294967294.0) {
        return -1.0;
    }
    length = kiln_number_to_js_string(number, buffer, sizeof(buffer));
    if (length < 0 || (size_t)length != key_len || memcmp(buffer, key, key_len) != 0) {
        return -1.0;
    }
    return number;
}

static int kiln_c99_same_name(const char *key, size_t key_len, const char *name) {
    size_t len = strlen(name);
    return len == key_len && memcmp(key, name, len) == 0;
}

static int kiln_c99_same_key(const kiln_property *property, const char *key, size_t key_len) {
    return property->key_len == key_len &&
           (key_len == 0 || memcmp(property->key, key, key_len) == 0);
}

/* A write to an existing key replaces it in place, so insertion order is the order of first
 * appearance -- which is the order `Object.keys` reports and the order a console prints. */
static int kiln_c99_put(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value value, int depth, kiln_value *thrown) {
    kiln_object *object;
    size_t index;
    int status;
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_closure *closure = kiln_c99_closure_of(target);
        /* `f.prototype = {...}` replaces the object `new f()` links instances to, which is a
         * slot on the closure rather than an ordinary property -- a write that landed in the
         * store would be read back by `f.prototype` and ignored by `new`. */
        if (kiln_c99_same_name(key, key_len, "prototype")) {
            closure->prototype = value;
            return KILN_C99_OK;
        }
        if (closure->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&closure->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put(host, closure->properties, key, key_len, value, depth, thrown);
    }
    if (target.tag == KILN_VALUE_REGEXP && kiln_c99_same_name(key, key_len, "lastIndex")) {
        /* The one property of a regexp a program may write, and the reason two evaluations of one
         * literal must not share a `kiln_regexp`. */
        kiln_c99_regexp_of(target)->last_index = kiln_c99_as_number(value);
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* An array is not an object here, but it can still carry a property that is not an
         * element -- which is how `exec` reports `index` and `input`. */
        kiln_array *array = kiln_c99_array_of(target);
        if (kiln_c99_same_name(key, key_len, "length")) {
            /* `length` is derived, not stored, so writing it is a resize: truncating drops the
             * tail and growing fills with holes, which are not undefined elements. */
            double wanted = kiln_c99_as_number(value);
            size_t at;
            if (!(wanted >= 0.0) || floor(wanted) != wanted || wanted > 4294967295.0) {
                /* A RangeError rather than a write that quietly does nothing. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "Invalid array length");
            }
            if (array->frozen) {
                return KILN_C99_OK;
            }
            if ((size_t)wanted <= array->count) {
                array->count = (size_t)wanted;
                return KILN_C99_OK;
            }
            status = kiln_c99_reserve((void **)&array->elements, &array->capacity, (size_t)wanted,
                                      sizeof *array->elements);
            if (status != KILN_C99_OK) {
                return status;
            }
            for (at = array->count; at < (size_t)wanted; ++at) {
                array->elements[at] = kiln_c99_hole();
            }
            array->count = (size_t)wanted;
            return KILN_C99_OK;
        }
        if (array->properties.tag != KILN_VALUE_OBJECT) {
            status = kiln_c99_new_object(&array->properties);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return kiln_c99_put(host, array->properties, key, key_len, value, depth, thrown);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_CONTROL_ERROR;
    }
    object = kiln_c99_object_of(target);
    if (object->frozen) {
        /* Silently, which is what the interpreter does and therefore what this has to do. */
        return KILN_C99_OK;
    }
    /* A setter found anywhere on the chain takes the write; only when there is none does the
     * assignment create an own data property. That asymmetry is what makes `obj.x = 1` on a
     * class with `set x()` run the setter rather than shadow it. */
    {
        kiln_value current = target;
        size_t steps;
        for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_object *walked;
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            walked = kiln_c99_object_of(current);
            for (index = 0; index < walked->count; ++index) {
                if (!kiln_c99_same_key(&walked->properties[index], key, key_len)) {
                    continue;
                }
                if (!walked->properties[index].accessor) {
                    /* A data property on the *prototype* is shadowed by an own one, not
                     * written through, so only an own hit assigns here. */
                    if (steps == 0) {
                        walked->properties[index].value = value;
                        return KILN_C99_OK;
                    }
                    steps = KILN_C99_MAX_PROTOTYPE_DEPTH;
                    break;
                }
                if (walked->properties[index].setter.tag == KILN_VALUE_UNDEFINED) {
                    /* Getter-only: the write is discarded in sloppy mode and a TypeError in
                     * strict. Modules are strict, but a script is not, and this is shared. */
                    return KILN_C99_OK;
                }
                {
                    kiln_value ignored;
                    return kiln_c99_call(host, &ignored, walked->properties[index].setter, target,
                                         &value, 1, depth, thrown);
                }
            }
            if (steps >= KILN_C99_MAX_PROTOTYPE_DEPTH) {
                break;
            }
            current = walked->prototype;
        }
    }
    status = kiln_c99_reserve((void **)&object->properties, &object->capacity, object->count + 1,
                              sizeof *object->properties);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* Integer-like keys come first, in ascending numeric order, and everything else follows in
     * insertion order.  That is the order the language specifies for own properties, so it is
     * the order a console prints and the order `Object.keys` would report -- keeping the list
     * sorted on insert means nothing downstream has to re-derive it. */
    {
        double slot_index = kiln_c99_key_index(key, key_len);
        size_t position = object->count;
        if (slot_index >= 0.0) {
            for (index = 0; index < object->count; ++index) {
                double existing = kiln_c99_key_index(object->properties[index].key,
                                                     object->properties[index].key_len);
                if (existing < 0.0 || existing > slot_index) {
                    break;
                }
            }
            position = index;
        }
        for (index = object->count; index > position; --index) {
            object->properties[index] = object->properties[index - 1];
        }
        object->properties[position].key = key;
        object->properties[position].key_len = key_len;
        /* A `@@` key is the machine's, not the program's: a private member, an error's kind, a
         * combinator's bookkeeping. Nothing that enumerates own string properties may see one, and
         * marking it here is one place rather than at each of the seven walks. The program cannot
         * spell such a key -- `obj["@@x"]` is a different property only because nothing in the
         * language produces this prefix. */
        object->properties[position].enumerable =
            !(key_len >= 2 && key[0] == '@' && key[1] == '@');
        object->properties[position].accessor = 0;
        object->properties[position].value = value;
        object->properties[position].getter = kiln_c99_undefined();
        object->properties[position].setter = kiln_c99_undefined();
    }
    object->count += 1;
    return KILN_C99_OK;
}

static kiln_generator *kiln_c99_generator_of(kiln_value value) {
    return (kiln_generator *)(uintptr_t)value.bits;
}

/* The frame a generator will run in, filled with its arguments and nothing else.
 *
 * Both arrays are sized from the body's own counts, so a suspension has somewhere to put every
 * local and every operand the verifier proved it can hold. */
static int kiln_c99_new_generator(kiln_value *out, kiln_value function, kiln_value receiver,
                                  const kiln_value *args, size_t argc) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    kiln_generator *frame = (kiln_generator *)malloc(sizeof *frame);
    size_t locals = closure->local_count == 0 ? 1 : closure->local_count;
    size_t depth = closure->stack_size == 0 ? 1 : closure->stack_size;
    size_t index;
    if (frame == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    frame->locals = (kiln_value *)malloc(locals * sizeof *frame->locals);
    frame->stack = (kiln_value *)malloc(depth * sizeof *frame->stack);
    /* One handler slot per instruction is the bound the emitter uses for an automatic frame, and
     * it is the same bound here: a body cannot register more handlers than it has instructions. */
    frame->handlers = (size_t *)malloc((closure->handler_depth == 0 ? 1 : closure->handler_depth) *
                                       sizeof *frame->handlers);
    frame->handler_stack =
        (size_t *)malloc((closure->handler_depth == 0 ? 1 : closure->handler_depth) *
                         sizeof *frame->handler_stack);
    if (frame->locals == NULL || frame->stack == NULL || frame->handlers == NULL ||
        frame->handler_stack == NULL) {
        free(frame->locals);
        free(frame->stack);
        free(frame->handlers);
        free(frame->handler_stack);
        free(frame);
        return KILN_C99_OUT_OF_MEMORY;
    }
    frame->handler_top = 0;
    for (index = 0; index < locals; ++index) {
        frame->locals[index] = kiln_c99_undefined();
    }
    for (index = 0; index < argc && index < locals; ++index) {
        frame->locals[index] = args[index];
    }
    frame->function = function;
    frame->receiver = receiver;
    frame->sp = 0;
    frame->resume = 0;
    frame->sent = kiln_c99_undefined();
    frame->awaiting = 0;
    frame->async_generator = 0;
    frame->raising = 0;
    frame->promise = kiln_c99_undefined();
    frame->is_async = 0;
    frame->started = 0;
    frame->done = 0;
    *out = kiln_c99_value(KILN_VALUE_GENERATOR, (uint64_t)(uintptr_t)frame, NULL, 0);
    return KILN_C99_OK;
}

/* The instant a set of UTC parts names, with every part allowed out of range.
 *
 * Out-of-range parts roll over rather than clamping: `Date.UTC(2026, 12)` is January of 2027.
 * That falls out of normalising the month into a year offset and then counting in whole units,
 * and it is what the specification says -- clamping would quietly produce a different date than
 * the program asked for. */
static double kiln_c99_instant_from_parts(const double parts[7]) {
    double months;
    double year;
    double month;
    double days;
    size_t index;
    for (index = 0; index < 7; ++index) {
        if (!isfinite(parts[index])) {
            return 0.0 / 0.0;
        }
    }
    months = floor(parts[1]);
    year = trunc(parts[0]) + floor(months / 12.0);
    month = months - floor(months / 12.0) * 12.0;
    /* Beyond what a civil date can name; the calendar arithmetic would overflow. */
    if (fabs(year) > 400000.0) {
        return 0.0 / 0.0;
    }
    days = (double)kiln_c99_days_from_civil((int64_t)year, (int64_t)month + 1, 1) +
           (parts[2] - 1.0);
    return days * 86400000.0 + parts[3] * 3600000.0 + parts[4] * 60000.0 + parts[5] * 1000.0 +
           parts[6];
}

/* Whether an array method changes how many elements there are.
 *
 * `sort` and `reverse` are not on the list: they write elements a frozen array already refuses,
 * and the length they leave is the one they were given. */
static int kiln_c99_changes_length(uint64_t code) {
    return code == KILN_C99_METHOD_PUSH || code == KILN_C99_METHOD_POP ||
           code == KILN_C99_METHOD_ARRAY_SHIFT || code == KILN_C99_METHOD_ARRAY_UNSHIFT ||
           code == KILN_C99_METHOD_ARRAY_SPLICE;
}

static kiln_value kiln_c99_method(uint64_t code) {
    return kiln_c99_value(KILN_VALUE_METHOD, code, NULL, 0);
}

/* The name a console shows, which is the JavaScript name rather than the implementation's --
 * `[Function: push]`, the same text the interpreter produces. */
static const char *kiln_c99_method_name(uint64_t code) {
    switch (code) {
    case KILN_C99_METHOD_PUSH: return "push";
    case KILN_C99_METHOD_POP: return "pop";
    case KILN_C99_METHOD_JOIN: return "join";
    case KILN_C99_METHOD_INDEX_OF: return "indexOf";
    case KILN_C99_METHOD_INCLUDES: return "includes";
    case KILN_C99_METHOD_SLICE: return "slice";
    case KILN_C99_METHOD_CONCAT: return "concat";
    case KILN_C99_METHOD_ARRAY_AT: return "at";
    case KILN_C99_METHOD_ARRAY_FIND: return "find";
    case KILN_C99_METHOD_ARRAY_FIND_INDEX: return "findIndex";
    case KILN_C99_METHOD_ARRAY_SOME: return "some";
    case KILN_C99_METHOD_ARRAY_EVERY: return "every";
    case KILN_C99_METHOD_ARRAY_SORT: return "sort";
    case KILN_C99_METHOD_ARRAY_FLAT: return "flat";
    case KILN_C99_METHOD_ARRAY_ENTRIES: return "entries";
    case KILN_C99_METHOD_ARRAY_KEYS: return "keys";
    case KILN_C99_METHOD_ARRAY_REVERSE: return "reverse";
    case KILN_C99_METHOD_ARRAY_SHIFT: return "shift";
    case KILN_C99_METHOD_ARRAY_UNSHIFT: return "unshift";
    case KILN_C99_METHOD_ARRAY_FILL: return "fill";
    case KILN_C99_METHOD_ARRAY_LAST_INDEX_OF: return "lastIndexOf";
    case KILN_C99_METHOD_ARRAY_SPLICE: return "splice";
    case KILN_C99_METHOD_ARRAY_COPY_WITHIN: return "copyWithin";
    case KILN_C99_METHOD_ARRAY_TO_STRING: return "toString";
    case KILN_C99_METHOD_ARRAY_FLAT_MAP: return "flatMap";
    case KILN_C99_METHOD_ARRAY_FIND_LAST: return "findLast";
    case KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX: return "findLastIndex";
    case KILN_C99_METHOD_ARRAY_REDUCE_RIGHT: return "reduceRight";
    case KILN_C99_METHOD_MAP: return "map";
    case KILN_C99_METHOD_FILTER: return "filter";
    case KILN_C99_METHOD_FOR_EACH: return "forEach";
    case KILN_C99_METHOD_REDUCE: return "reduce";
    case KILN_C99_METHOD_STRING_MATCH: return "match";
    case KILN_C99_METHOD_STRING_MATCH_ALL: return "matchAll";
    case KILN_C99_METHOD_STRING_SEARCH: return "search";
    case KILN_C99_METHOD_STRING_CODE_POINT_AT: return "codePointAt";
    case KILN_C99_METHOD_STRING_SUBSTR: return "substr";
    case KILN_C99_METHOD_STRING_LOCALE_COMPARE: return "localeCompare";
    case KILN_C99_METHOD_STRING_NORMALIZE: return "normalize";
    case KILN_C99_METHOD_STRING_TO_STRING: return "toString";
    case KILN_C99_METHOD_STRING_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_STRING_SLICE: return "slice";
    case KILN_C99_METHOD_SUBSTRING: return "substring";
    case KILN_C99_METHOD_STRING_INDEX_OF: return "indexOf";
    case KILN_C99_METHOD_LAST_INDEX_OF: return "lastIndexOf";
    case KILN_C99_METHOD_STRING_INCLUDES: return "includes";
    case KILN_C99_METHOD_STARTS_WITH: return "startsWith";
    case KILN_C99_METHOD_ENDS_WITH: return "endsWith";
    case KILN_C99_METHOD_CHAR_AT: return "charAt";
    case KILN_C99_METHOD_CHAR_CODE_AT: return "charCodeAt";
    case KILN_C99_METHOD_STRING_AT: return "at";
    case KILN_C99_METHOD_TRIM: return "trim";
    case KILN_C99_METHOD_TRIM_START: return "trimStart";
    case KILN_C99_METHOD_TRIM_END: return "trimEnd";
    case KILN_C99_METHOD_SPLIT: return "split";
    case KILN_C99_METHOD_REPEAT: return "repeat";
    case KILN_C99_METHOD_PAD_START: return "padStart";
    case KILN_C99_METHOD_PAD_END: return "padEnd";
    case KILN_C99_METHOD_TO_UPPER_CASE: return "toUpperCase";
    case KILN_C99_METHOD_TO_LOWER_CASE: return "toLowerCase";
    case KILN_C99_METHOD_REPLACE: return "replace";
    case KILN_C99_METHOD_REPLACE_ALL: return "replaceAll";
    case KILN_C99_METHOD_STRING_CONCAT: return "concat";
    case KILN_C99_METHOD_MATH_FLOOR: return "floor";
    case KILN_C99_METHOD_MATH_CEIL: return "ceil";
    case KILN_C99_METHOD_MATH_ROUND: return "round";
    case KILN_C99_METHOD_MATH_TRUNC: return "trunc";
    case KILN_C99_METHOD_MATH_ABS: return "abs";
    case KILN_C99_METHOD_MATH_SIGN: return "sign";
    case KILN_C99_METHOD_MATH_SQRT: return "sqrt";
    case KILN_C99_METHOD_MATH_CBRT: return "cbrt";
    case KILN_C99_METHOD_MATH_POW: return "pow";
    case KILN_C99_METHOD_MATH_MIN: return "min";
    case KILN_C99_METHOD_MATH_MAX: return "max";
    case KILN_C99_METHOD_MATH_LOG: return "log";
    case KILN_C99_METHOD_MATH_LOG2: return "log2";
    case KILN_C99_METHOD_MATH_LOG10: return "log10";
    case KILN_C99_METHOD_MATH_EXP: return "exp";
    case KILN_C99_METHOD_MATH_HYPOT: return "hypot";
    case KILN_C99_METHOD_MATH_ATAN2: return "atan2";
    case KILN_C99_METHOD_MATH_SIN: return "sin";
    case KILN_C99_METHOD_MATH_COS: return "cos";
    case KILN_C99_METHOD_MATH_TAN: return "tan";
    case KILN_C99_METHOD_MATH_ASIN: return "asin";
    case KILN_C99_METHOD_MATH_ACOS: return "acos";
    case KILN_C99_METHOD_MATH_ATAN: return "atan";
    case KILN_C99_METHOD_QUEUE_MICROTASK: return "queueMicrotask";
    case KILN_C99_METHOD_SET_TIMEOUT: return "setTimeout";
    case KILN_C99_METHOD_SET_INTERVAL: return "setInterval";
    case KILN_C99_METHOD_CLEAR_TIMEOUT: return "clearTimeout";
    case KILN_C99_METHOD_CLEAR_INTERVAL: return "clearInterval";
    case KILN_C99_METHOD_ESCAPE: return "escape";
    case KILN_C99_METHOD_UNESCAPE: return "unescape";
    case KILN_C99_METHOD_ENCODE_URI: return "encodeURI";
    case KILN_C99_METHOD_ENCODE_URI_COMPONENT: return "encodeURIComponent";
    case KILN_C99_METHOD_DECODE_URI: return "decodeURI";
    case KILN_C99_METHOD_DECODE_URI_COMPONENT: return "decodeURIComponent";
    case KILN_C99_METHOD_NUMBER_TO_STRING: return "toString";
    case KILN_C99_METHOD_NUMBER_TO_FIXED: return "toFixed";
    case KILN_C99_METHOD_NUMBER_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_PARSE_INT: return "parseInt";
    case KILN_C99_METHOD_PARSE_FLOAT: return "parseFloat";
    case KILN_C99_METHOD_GLOBAL_IS_NAN: return "isNaN";
    case KILN_C99_METHOD_GLOBAL_IS_FINITE: return "isFinite";
    case KILN_C99_METHOD_IS_INTEGER: return "isInteger";
    case KILN_C99_METHOD_IS_SAFE_INTEGER: return "isSafeInteger";
    case KILN_C99_METHOD_IS_NAN: return "isNaN";
    case KILN_C99_METHOD_IS_FINITE: return "isFinite";
    case KILN_C99_METHOD_OBJECT_KEYS: return "keys";
    case KILN_C99_METHOD_OBJECT_VALUES: return "values";
    case KILN_C99_METHOD_OBJECT_ENTRIES: return "entries";
    case KILN_C99_METHOD_OBJECT_ASSIGN: return "assign";
    case KILN_C99_METHOD_HAS_OWN: return "hasOwn";
    case KILN_C99_METHOD_IS_ARRAY: return "isArray";
    case KILN_C99_METHOD_ARRAY_OF: return "of";
    case KILN_C99_METHOD_OBJECT_CREATE: return "create";
    case KILN_C99_METHOD_JSON_STRINGIFY: return "stringify";
    case KILN_C99_METHOD_JSON_PARSE: return "parse";
    case KILN_C99_METHOD_GENERATOR_NEXT: return "next";
    case KILN_C99_METHOD_GENERATOR_RETURN: return "return";
    /* `[Symbol.iterator]()` on a generator hands back the generator, which is what lets one be
     * spread, destructured or walked by `for...of` -- and what makes walking it twice walk a
     * generator that is already finished. */
    case KILN_C99_METHOD_GENERATOR_SELF: return "@@iterator";
    case KILN_C99_METHOD_GET_TIME: return "getTime";
    case KILN_C99_METHOD_GET_FULL_YEAR: return "getFullYear";
    case KILN_C99_METHOD_GET_MONTH: return "getMonth";
    case KILN_C99_METHOD_GET_DATE: return "getDate";
    case KILN_C99_METHOD_GET_DAY: return "getDay";
    case KILN_C99_METHOD_GET_HOURS: return "getHours";
    case KILN_C99_METHOD_GET_MINUTES: return "getMinutes";
    case KILN_C99_METHOD_GET_SECONDS: return "getSeconds";
    case KILN_C99_METHOD_GET_MILLISECONDS: return "getMilliseconds";
    case KILN_C99_METHOD_GET_TIMEZONE_OFFSET: return "getTimezoneOffset";
    case KILN_C99_METHOD_TO_ISO_STRING: return "toISOString";
    case KILN_C99_METHOD_TO_JSON: return "toJSON";
    case KILN_C99_METHOD_DATE_TO_STRING: return "toString";
    case KILN_C99_METHOD_DATE_NOW: return "now";
    case KILN_C99_METHOD_DATE_UTC: return "UTC";
    case KILN_C99_METHOD_DATE_PARSE: return "parse";
    case KILN_C99_METHOD_HAS_OWN_PROPERTY: return "hasOwnProperty";
    case KILN_C99_METHOD_OBJECT_TO_STRING: return "toString";
    case KILN_C99_METHOD_OBJECT_VALUE_OF: return "valueOf";
    case KILN_C99_METHOD_COLLECTION_GET: return "get";
    case KILN_C99_METHOD_COLLECTION_SET: return "set";
    case KILN_C99_METHOD_COLLECTION_ADD: return "add";
    case KILN_C99_METHOD_COLLECTION_HAS: return "has";
    case KILN_C99_METHOD_COLLECTION_DELETE: return "delete";
    case KILN_C99_METHOD_COLLECTION_CLEAR: return "clear";
    case KILN_C99_METHOD_COLLECTION_FOR_EACH: return "forEach";
    case KILN_C99_METHOD_COLLECTION_KEYS: return "keys";
    case KILN_C99_METHOD_COLLECTION_VALUES: return "values";
    case KILN_C99_METHOD_COLLECTION_ENTRIES: return "entries";
    case KILN_C99_METHOD_COLLECTION_SELF: return "@@iterator";
    case KILN_C99_METHOD_ITERATOR_NEXT: return "next";
    case KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT: return "next";
    case KILN_C99_METHOD_PRIMITIVE_TO_STRING: return "toString";
    case KILN_C99_METHOD_ITERATOR_SELF: return "@@iterator";
    case KILN_C99_METHOD_ARRAY_VALUES: return "values";
    case KILN_C99_METHOD_THEN: return "then";
    case KILN_C99_METHOD_CATCH: return "catch";
    case KILN_C99_METHOD_FINALLY: return "finally";
    case KILN_C99_METHOD_PROMISE_RESOLVE: return "resolve";
    case KILN_C99_METHOD_PROMISE_REJECT: return "reject";
    case KILN_C99_METHOD_PROMISE_ALL: return "all";
    case KILN_C99_METHOD_PROMISE_RACE: return "race";
    /* Not reachable by name: these are the two handlers an `await` attaches to what it is
     * waiting on, and nothing looks them up. */
    case KILN_C99_METHOD_ASYNC_RESUME: return "@@resume";
    case KILN_C99_METHOD_ASYNC_THROW: return "@@throw";
    case KILN_C99_METHOD_PROMISE_ALL_SETTLED: return "allSettled";
    case KILN_C99_METHOD_PROMISE_ANY: return "any";
    case KILN_C99_METHOD_ALL_STEP: return "@@allStep";
    case KILN_C99_METHOD_REJECT_STEP: return "@@rejectStep";
    case KILN_C99_METHOD_FINALLY_PASS: return "@@finallyPass";
    case KILN_C99_METHOD_FINALLY_RAISE: return "@@finallyRaise";
    case KILN_C99_METHOD_FINALLY_RESTORE: return "@@finallyRestore";
    case KILN_C99_METHOD_FINALLY_RERAISE: return "@@finallyReraise";
    case KILN_C99_METHOD_ADOPT_STEP: return "@@adopt";
    case KILN_C99_METHOD_REGEXP_TEST: return "test";
    case KILN_C99_METHOD_REGEXP_EXEC: return "exec";
    case KILN_C99_METHOD_REGEXP_TO_STRING: return "toString";
    case KILN_C99_METHOD_MATH_RANDOM: return "random";
    case KILN_C99_METHOD_MATH_SINH: return "sinh";
    case KILN_C99_METHOD_MATH_COSH: return "cosh";
    case KILN_C99_METHOD_MATH_TANH: return "tanh";
    case KILN_C99_METHOD_MATH_ASINH: return "asinh";
    case KILN_C99_METHOD_MATH_ACOSH: return "acosh";
    case KILN_C99_METHOD_MATH_ATANH: return "atanh";
    case KILN_C99_METHOD_MATH_LOG1P: return "log1p";
    case KILN_C99_METHOD_MATH_EXPM1: return "expm1";
    case KILN_C99_METHOD_MATH_FROUND: return "fround";
    case KILN_C99_METHOD_MATH_CLZ32: return "clz32";
    case KILN_C99_METHOD_MATH_IMUL: return "imul";
    case KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF: return "getPrototypeOf";
    case KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF: return "setPrototypeOf";
    case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES: return "getOwnPropertyNames";
    case KILN_C99_METHOD_OBJECT_FROM_ENTRIES: return "fromEntries";
    case KILN_C99_METHOD_ARRAY_FROM: return "from";
    case KILN_C99_METHOD_IS_PROTOTYPE_OF: return "isPrototypeOf";
    case KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE: return "propertyIsEnumerable";
    case KILN_C99_METHOD_FUNCTION_CALL: return "call";
    case KILN_C99_METHOD_FUNCTION_APPLY: return "apply";
    case KILN_C99_METHOD_FUNCTION_BIND: return "bind";
    case KILN_C99_METHOD_OBJECT_FREEZE: return "freeze";
    case KILN_C99_METHOD_OBJECT_IS_FROZEN: return "isFrozen";
    default: return NULL;
    }
}

/* The state `Math.random` advances, and the constants that advance it.
 *
 * SplitMix64: a full-period generator with good statistical properties in five lines, which is
 * what a language builtin needs. Seeded from a constant rather than from the environment, so the
 * same program produces the same sequence in both engines and on every machine -- the same trade
 * the virtual clock makes, and the reason an artifact hash stays honest. See ADR 0026. */
static uint64_t kiln_c99_random_state = UINT64_C(0x9E3779B97F4A7C15);

static double kiln_c99_random(void) {
    uint64_t mixed;
    kiln_c99_random_state += UINT64_C(0x9E3779B97F4A7C15);
    mixed = kiln_c99_random_state;
    mixed = (mixed ^ (mixed >> 30)) * UINT64_C(0xBF58476D1CE4E5B9);
    mixed = (mixed ^ (mixed >> 27)) * UINT64_C(0x94D049BB133111EB);
    mixed = mixed ^ (mixed >> 31);
    /* 53 random bits, which is every bit the mantissa has. */
    return (double)(mixed >> 11) / 9007199254740992.0;
}

/* One hex digit's value, or -1. Used by every unescaping path, none of which may accept the
 * `0x` prefixes or the locale-dependent classifications that `strtol` would. */
static int kiln_c99_hex_digit(char character) {
    if (character >= '0' && character <= '9') {
        return character - '0';
    }
    if (character >= 'a' && character <= 'f') {
        return character - 'a' + 10;
    }
    if (character >= 'A' && character <= 'F') {
        return character - 'A' + 10;
    }
    return -1;
}

static int kiln_c99_append_hex(kiln_text *text, unsigned value, int width) {
    static const char alphabet[] = "0123456789ABCDEF";
    char digits[4];
    int place;
    for (place = width - 1; place >= 0; --place) {
        digits[place] = alphabet[value & 0xFu];
        value >>= 4;
    }
    return kiln_c99_text_append(text, digits, (size_t)width);
}

/* Whether a byte span is well-formed UTF-8.
 *
 * `decodeURI` is the only place this is asked: percent-escapes name bytes, and a program can name
 * bytes that are not a string. Rust's `String::from_utf8` is what rejects them on the interpreter
 * side, and this is that check -- overlong forms, surrogates and out-of-range scalars included,
 * because accepting any of them would let the two engines hold different strings. */
static int kiln_c99_valid_utf8(const char *bytes, size_t len) {
    size_t at = 0;
    while (at < len) {
        unsigned char lead = (unsigned char)bytes[at];
        uint32_t codepoint;
        size_t extra;
        size_t index;
        if (lead < 0x80u) {
            at += 1;
            continue;
        }
        if (lead >= 0xC2u && lead <= 0xDFu) {
            extra = 1;
            codepoint = lead & 0x1Fu;
        } else if (lead >= 0xE0u && lead <= 0xEFu) {
            extra = 2;
            codepoint = lead & 0x0Fu;
        } else if (lead >= 0xF0u && lead <= 0xF4u) {
            extra = 3;
            codepoint = lead & 0x07u;
        } else {
            /* A continuation byte on its own, or an overlong two-byte lead. */
            return 0;
        }
        if (at + extra >= len) {
            /* The sequence runs off the end. */
            return 0;
        }
        for (index = 1; index <= extra; ++index) {
            unsigned char next = (unsigned char)bytes[at + index];
            if ((next & 0xC0u) != 0x80u) {
                return 0;
            }
            codepoint = (codepoint << 6) | (next & 0x3Fu);
        }
        if (codepoint > UINT32_C(0x10FFFF) ||
            (codepoint >= UINT32_C(0xD800) && codepoint <= UINT32_C(0xDFFF)) ||
            (extra == 2 && codepoint < UINT32_C(0x800)) ||
            (extra == 3 && codepoint < UINT32_C(0x10000))) {
            return 0;
        }
        at += extra + 1;
    }
    return 1;
}

/* `Number.prototype.toFixed`, rounded on the value's *exact* decimal expansion.
 *
 * Not the printf `%.*f` conversion: it reads the process locale's decimal point, and an embedded
 * module runs in whatever locale its host set. The same objection is why a number's shortest
 * spelling comes from the number provider (ADR 0018); this is that rule applied to the other
 * direction.
 *
 * Not `floor(value * 10^f + 0.5)` either, which is the obvious arithmetic and is wrong:
 * `(1.45).toFixed(1)` is `"1.4"` because 1.45 is really 1.44999..., and multiplying by ten rounds
 * that up to exactly 14.5 before any rounding decision is made. The digits that decide have to be
 * the value's own.
 *
 * So the expansion is computed exactly. A binary64 is `m * 2^e` with `m` under 2^53, and for
 * negative `e` that is `m * 5^(-e)` with the point `-e` digits from the right -- an exact integer
 * a decimal digit array can hold, and repeated multiplication by five is the whole of the
 * arithmetic. 5^1074 is 751 digits, `m` adds sixteen, and values at or above 1e21 are handed back
 * to `ToString` by the specification, so the array is bounded and so is the loop. */
static int kiln_c99_to_fixed(kiln_value *out, double value, int places) {
    /* Little-endian decimal digits: `digits[0]` is the least significant. */
    unsigned char digits[1200];
    size_t count = 0;
    size_t fractional;
    size_t index;
    kiln_text text = {NULL, 0, 0};
    double mantissa;
    uint64_t scaled;
    int exponent;
    int negative = value < 0.0;
    int round_up;
    if (!isfinite(value) || fabs(value) >= 1e21) {
        *out = kiln_c99_text(kiln_c99_number(value));
        return out->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
    }
    mantissa = frexp(fabs(value), &exponent);
    scaled = (uint64_t)ldexp(mantissa, 53);
    exponent -= 53;
    while (scaled != 0 && count < sizeof digits) {
        digits[count++] = (unsigned char)(scaled % 10u);
        scaled /= 10u;
    }
    if (count == 0) {
        digits[count++] = 0;
    }
    if (exponent >= 0) {
        int step;
        for (step = 0; step < exponent; ++step) {
            unsigned carry = 0;
            for (index = 0; index < count; ++index) {
                unsigned product = (unsigned)digits[index] * 2u + carry;
                digits[index] = (unsigned char)(product % 10u);
                carry = product / 10u;
            }
            while (carry != 0 && count < sizeof digits) {
                digits[count++] = (unsigned char)(carry % 10u);
                carry /= 10u;
            }
        }
        fractional = 0;
    } else {
        int step;
        for (step = 0; step < -exponent; ++step) {
            unsigned carry = 0;
            for (index = 0; index < count; ++index) {
                unsigned product = (unsigned)digits[index] * 5u + carry;
                digits[index] = (unsigned char)(product % 10u);
                carry = product / 10u;
            }
            while (carry != 0 && count < sizeof digits) {
                digits[count++] = (unsigned char)(carry % 10u);
                carry /= 10u;
            }
        }
        fractional = (size_t)(-exponent);
    }
    /* Pad so there is a digit at every place the answer names, above the point and below it. */
    while (count < fractional + 1 && count < sizeof digits) {
        digits[count++] = 0;
    }
    if (fractional <= (size_t)places) {
        round_up = 0;
    } else {
        /* Round half *up*: "if there are two such n, pick the larger n". Only the deciding digit
         * is consulted, because at or above five the answer is up whatever follows. */
        round_up = digits[fractional - (size_t)places - 1] >= 5;
        for (index = 0; index < fractional - (size_t)places; ++index) {
            digits[index] = 0;
        }
        memmove(digits, digits + (fractional - (size_t)places),
                count - (fractional - (size_t)places));
        count -= fractional - (size_t)places;
        fractional = (size_t)places;
    }
    while (fractional < (size_t)places && count < sizeof digits) {
        /* The value terminated above the requested precision, so the rest is zeros. */
        memmove(digits + 1, digits, count);
        digits[0] = 0;
        count += 1;
        fractional += 1;
    }
    if (round_up) {
        for (index = 0; index < count; ++index) {
            if (digits[index] == 9) {
                digits[index] = 0;
                continue;
            }
            digits[index] += 1;
            break;
        }
        if (index == count && count < sizeof digits) {
            digits[count++] = 1;
        }
    }
    while (count > fractional + 1 && digits[count - 1] == 0) {
        /* A leading zero above the point is not part of the spelling, but the one before the
         * point is: `.5` is `0.5`. */
        count -= 1;
    }
    /* The sign survives rounding: `(-0.0004).toFixed(2)` is `"-0.00"`, which says the value was
     * negative rather than that it was nonzero. Negative zero is not negative here, so
     * `(-0).toFixed(2)` is `"0.00"`. */
    if (negative && kiln_c99_text_append_c(&text, "-") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = count; index > 0; --index) {
        char character = (char)('0' + digits[index - 1]);
        if (index == fractional && kiln_c99_text_append_c(&text, ".") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (kiln_c99_text_append(&text, &character, 1) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

/* A number written in a base other than ten, which `Number.prototype.toString` is the only way
 * to ask for. Digits are produced by repeated division rather than by the number formatter,
 * because the formatter only knows base ten -- and the fraction is bounded at 52 digits, which is
 * every bit a double has to say. */
static int kiln_c99_number_in_radix(kiln_value *out, double value, int radix) {
    kiln_text text = {NULL, 0, 0};
    static const char alphabet[] = "0123456789abcdefghijklmnopqrstuvwxyz";
    char digits[1120];
    size_t count = 0;
    double whole;
    double fraction;
    int negative;
    if (!isfinite(value)) {
        *out = kiln_c99_text(kiln_c99_number(value));
        return KILN_C99_OK;
    }
    negative = value < 0.0;
    value = fabs(value);
    whole = trunc(value);
    fraction = value - whole;
    while (whole >= 1.0 && count < sizeof digits) {
        digits[count++] = alphabet[(int)fmod(whole, (double)radix)];
        whole = trunc(whole / (double)radix);
    }
    if (negative && kiln_c99_text_append_c(&text, "-") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (count == 0 && kiln_c99_text_append_c(&text, "0") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    while (count > 0) {
        count -= 1;
        if (kiln_c99_text_append(&text, digits + count, 1) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (fraction > 0.0) {
        int place;
        if (kiln_c99_text_append_c(&text, ".") != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (place = 0; place < 52 && fraction > 0.0; ++place) {
            double digit;
            char character;
            fraction *= (double)radix;
            digit = trunc(fraction);
            character = alphabet[(int)digit];
            fraction -= digit;
            if (kiln_c99_text_append(&text, &character, 1) != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

/* The JavaScript name of a runtime-supplied value, which is what a console shows and what
 * `String(x)` produces. The interpreter derives its equivalent from the variant's spelling; here
 * the tags carry no spelling to derive from, so this is the derivation -- and
 * `conformance/differential/builtin-names.js` is what checks the two agree with node. */
static const char *kiln_c99_builtin_name(uint64_t builtin) {
    switch (builtin) {
    case KILN_C99_BUILTIN_STRING: return "String";
    case KILN_C99_BUILTIN_MATH: return "Math";
    case KILN_C99_BUILTIN_NUMBER: return "Number";
    case KILN_C99_BUILTIN_BIGINT: return "BigInt";
    case KILN_C99_BUILTIN_OBJECT: return "Object";
    case KILN_C99_BUILTIN_ARRAY: return "Array";
    case KILN_C99_BUILTIN_JSON: return "JSON";
    case KILN_C99_BUILTIN_DATE: return "Date";
    case KILN_C99_BUILTIN_MAP: return "Map";
    case KILN_C99_BUILTIN_SET: return "Set";
    case KILN_C99_BUILTIN_PROMISE: return "Promise";
    case KILN_C99_BUILTIN_BOOLEAN: return "Boolean";
    case KILN_C99_BUILTIN_REFLECT: return "Reflect";
    case KILN_C99_BUILTIN_OBJECT_PROTOTYPE: return "Object";
    case KILN_C99_BUILTIN_ARRAY_PROTOTYPE: return "Array";
    case KILN_C99_BUILTIN_STRING_PROTOTYPE: return "String";
    case KILN_C99_BUILTIN_SYMBOL: return "Symbol";
    case KILN_C99_BUILTIN_REGEXP: return "RegExp";
    case KILN_C99_BUILTIN_REGEXP_PROTOTYPE: return "RegExp";
    case KILN_C99_BUILTIN_DATE_PROTOTYPE: return "Date";
    case KILN_C99_BUILTIN_NUMBER_PROTOTYPE: return "Number";
    case KILN_C99_BUILTIN_FUNCTION_PROTOTYPE: return "Function";
    case KILN_C99_BUILTIN_FUNCTION: return "Function";
    default: {
        /* Every other tag is an error constructor; a tag with no name at all is a tag added
         * without one, which shows as anonymous rather than dereferencing a null. */
        const char *name = kiln_c99_error_name(builtin);
        return name == NULL ? "anonymous" : name;
    }
    }
}

/* Expand a replacement template against one match.
 *
 * `$$` is a literal dollar, `$&` the match, `` $` `` what came before it, `$'` what came after,
 * and `$1`..`$99` a capture group -- and a `$` in front of anything else stands for itself. The
 * template is a template even when the pattern was a plain string, which is why this is shared
 * rather than living in the regexp path: `"a".replace("a", "$&$&")` is `"aa"`.
 *
 * `ranges` is the group offsets a search reported, with the whole match at index zero. A literal
 * replacement passes the occurrence as the only group, which is what makes one function serve. */
static int kiln_c99_expand_replacement(kiln_text *out, const uint16_t *pattern,
                                       size_t pattern_count, const uint16_t *subject,
                                       size_t subject_count, const int64_t *ranges,
                                       size_t groups) {
    size_t at = 0;
    while (at < pattern_count) {
        uint16_t unit = pattern[at];
        size_t taken = 1;
        const uint16_t *piece = NULL;
        size_t piece_count = 0;
        uint16_t literal = unit;
        if (unit == (uint16_t)'$' && at + 1 < pattern_count) {
            uint16_t next = pattern[at + 1];
            if (next == (uint16_t)'$') {
                taken = 2;
            } else if (next == (uint16_t)'&') {
                piece = subject + (size_t)ranges[0];
                piece_count = (size_t)(ranges[1] - ranges[0]);
                taken = 2;
            } else if (next == (uint16_t)'`') {
                piece = subject;
                piece_count = (size_t)ranges[0];
                taken = 2;
            } else if (next == (uint16_t)'\'') {
                piece = subject + (size_t)ranges[1];
                piece_count = subject_count - (size_t)ranges[1];
                taken = 2;
            } else if (next >= (uint16_t)'0' && next <= (uint16_t)'9') {
                /* Two digits when two would name a group that exists, one otherwise -- so `$12`
                 * is group 12 in a pattern with twelve and group 1 followed by a `2` in one
                 * without. */
                size_t wanted = (size_t)(next - (uint16_t)'0');
                taken = 2;
                if (at + 2 < pattern_count && pattern[at + 2] >= (uint16_t)'0' &&
                    pattern[at + 2] <= (uint16_t)'9') {
                    size_t wider = wanted * 10 + (size_t)(pattern[at + 2] - (uint16_t)'0');
                    if (wider < groups) {
                        wanted = wider;
                        taken = 3;
                    }
                }
                if (wanted == 0 || wanted >= groups) {
                    /* Not a group: the text stands for itself. */
                    taken = 1;
                } else if (ranges[wanted * 2] < 0) {
                    /* A group that did not participate contributes nothing. */
                    piece = subject;
                    piece_count = 0;
                } else {
                    piece = subject + (size_t)ranges[wanted * 2];
                    piece_count = (size_t)(ranges[wanted * 2 + 1] - ranges[wanted * 2]);
                }
            }
        }
        if (piece != NULL) {
            kiln_value text = kiln_c99_from_units(piece, piece_count);
            if (text.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append(out, text.ptr, text.len) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        } else {
            kiln_value text = kiln_c99_from_units(&literal, 1);
            if (text.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append(out, text.ptr, text.len) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        at += taken;
    }
    return KILN_C99_OK;
}

/* The constructor the running call was reached through, or undefined.
 *
 * A module-level value with save-and-restore around every call, rather than a field on a frame:
 * the body signature is fixed by the embedding ABI, and the C stack already gives the discipline
 * this needs -- each call restores what it found. */
static kiln_value kiln_c99_new_target;

/* Set when a `super(...)` is about to call, and cleared by the call that honours it.
 *
 * `super()` keeps the `new.target` of the call that is building the instance, which is how a base
 * constructor can name the subclass it is being built for -- so exactly one call has to *not*
 * compute a fresh one. */
static int kiln_c99_inheriting_new_target;

/* The running call's own arguments, and the list an arrow sees instead.
 *
 * An arrow has no `arguments` of its own -- `arguments` inside one is the enclosing function's,
 * which is a *different list* from the arrow's own rest parameter. So an arrow captures the
 * visible list where it was written, exactly as it captures `this` and `new.target`, and the
 * three are saved and restored together around every call for the same reason.
 *
 * The raw pointer and count rather than an array, because a call must not pay to materialise an
 * `arguments` nothing asked for; the array is built where one is actually read. */
static const kiln_value *kiln_c99_frame_args;
static size_t kiln_c99_frame_argc;
static kiln_value kiln_c99_captured_arguments;
static int kiln_c99_frame_is_arrow;

/* The `arguments` this frame can see: an arrow's captured list, or this call's own. */
static int kiln_c99_visible_arguments(kiln_value *out) {
    size_t at;
    int status;
    if (kiln_c99_frame_is_arrow) {
        *out = kiln_c99_captured_arguments;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_array(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    for (at = 0; at < kiln_c99_frame_argc; ++at) {
        status = kiln_c99_array_push(*out, kiln_c99_frame_args[at]);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    return KILN_C99_OK;
}

/* The realm's global object, created on first use.
 *
 * A real object rather than a tag, because a program *writes* to it: `globalThis.x = 1` has to be
 * found again by `globalThis.x`, and a builtin has nowhere to keep it. The names it starts with
 * are non-enumerable, which is what node does and what stops `Object.keys(globalThis)` from being
 * a list of the realm's furniture -- a program's own additions are enumerable, which is the
 * distinction that makes the question worth asking. */
static kiln_value kiln_c99_globals;

static int kiln_c99_global_object(kiln_value *out) {
    static const struct {
        const char *name;
        uint64_t builtin;
    } constructors[] = {{"String", KILN_C99_BUILTIN_STRING},
                        {"Number", KILN_C99_BUILTIN_NUMBER},
                        {"BigInt", KILN_C99_BUILTIN_BIGINT},
                        {"Boolean", KILN_C99_BUILTIN_BOOLEAN},
                        {"Object", KILN_C99_BUILTIN_OBJECT},
                        {"Array", KILN_C99_BUILTIN_ARRAY},
                        {"Symbol", KILN_C99_BUILTIN_SYMBOL},
                        {"RegExp", KILN_C99_BUILTIN_REGEXP},
                        {"Date", KILN_C99_BUILTIN_DATE},
                        {"Map", KILN_C99_BUILTIN_MAP},
                        {"Set", KILN_C99_BUILTIN_SET},
                        {"Promise", KILN_C99_BUILTIN_PROMISE},
                        {"Math", KILN_C99_BUILTIN_MATH},
                        {"JSON", KILN_C99_BUILTIN_JSON},
                        {"Reflect", KILN_C99_BUILTIN_REFLECT},
                        {"Error", KILN_C99_BUILTIN_ERROR},
                        {"TypeError", KILN_C99_BUILTIN_TYPE_ERROR},
                        {"RangeError", KILN_C99_BUILTIN_RANGE_ERROR},
                        {"ReferenceError", KILN_C99_BUILTIN_REFERENCE_ERROR},
                        {"SyntaxError", KILN_C99_BUILTIN_SYNTAX_ERROR}};
    static const struct {
        const char *name;
        uint64_t code;
    } functions[] = {{"parseInt", KILN_C99_METHOD_PARSE_INT},
                     {"parseFloat", KILN_C99_METHOD_PARSE_FLOAT},
                     {"isNaN", KILN_C99_METHOD_GLOBAL_IS_NAN},
                     {"isFinite", KILN_C99_METHOD_GLOBAL_IS_FINITE},
                     {"escape", KILN_C99_METHOD_ESCAPE},
                     {"unescape", KILN_C99_METHOD_UNESCAPE},
                     {"encodeURI", KILN_C99_METHOD_ENCODE_URI},
                     {"encodeURIComponent", KILN_C99_METHOD_ENCODE_URI_COMPONENT},
                     {"decodeURI", KILN_C99_METHOD_DECODE_URI},
                     {"decodeURIComponent", KILN_C99_METHOD_DECODE_URI_COMPONENT},
                     {"queueMicrotask", KILN_C99_METHOD_QUEUE_MICROTASK},
                     {"setTimeout", KILN_C99_METHOD_SET_TIMEOUT},
                     {"setInterval", KILN_C99_METHOD_SET_INTERVAL},
                     {"clearTimeout", KILN_C99_METHOD_CLEAR_TIMEOUT},
                     {"clearInterval", KILN_C99_METHOD_CLEAR_INTERVAL}};
    size_t index;
    int status;
    if (kiln_c99_globals.tag == KILN_VALUE_OBJECT) {
        *out = kiln_c99_globals;
        return KILN_C99_OK;
    }
    status = kiln_c99_new_object(&kiln_c99_globals);
    if (status != KILN_C99_OK) {
        return status;
    }
    for (index = 0; index < sizeof constructors / sizeof *constructors && status == KILN_C99_OK;
         ++index) {
        status = kiln_c99_put(NULL, kiln_c99_globals, constructors[index].name,
                              strlen(constructors[index].name),
                              kiln_c99_builtin(constructors[index].builtin), 0, NULL);
    }
    for (index = 0; index < sizeof functions / sizeof *functions && status == KILN_C99_OK;
         ++index) {
        status = kiln_c99_put(NULL, kiln_c99_globals, functions[index].name,
                              strlen(functions[index].name),
                              kiln_c99_method(functions[index].code), 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, kiln_c99_globals, "NaN", 3, kiln_c99_number(0.0 / 0.0), 0,
                              NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, kiln_c99_globals, "Infinity", 8, kiln_c99_number(1.0 / 0.0), 0,
                              NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, kiln_c99_globals, "undefined", 9, kiln_c99_undefined(), 0,
                              NULL);
    }
    /* It holds itself under both of its names, which is how a program reaches it without a
     * keyword: `global.Object === Object` is the standard test for "am I in a realm that has
     * one". */
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, kiln_c99_globals, "globalThis", 10, kiln_c99_globals, 0, NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, kiln_c99_globals, "global", 6, kiln_c99_globals, 0, NULL);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        kiln_object *object = kiln_c99_object_of(kiln_c99_globals);
        size_t slot;
        for (slot = 0; slot < object->count; ++slot) {
            object->properties[slot].enumerable = 0;
        }
    }
    *out = kiln_c99_globals;
    return KILN_C99_OK;
}

/* A BigInt written out in `radix`.
 *
 * The provider owns the digits, so this asks twice: once for the length and once for the bytes.
 * A negative answer is the length it wanted, which is what makes one retry enough. */
static kiln_value kiln_c99_bigint_text(kiln_value value, uint32_t radix) {
    const void *handle = (const void *)(uintptr_t)value.bits;
    char inline_buffer[64];
    int32_t written = kiln_bigint_to_string(handle, radix, (unsigned char *)inline_buffer,
                                            sizeof inline_buffer);
    if (written >= 0) {
        return kiln_c99_copy_text(inline_buffer, (size_t)written);
    }
    {
        size_t needed = (size_t)(-(int64_t)written);
        char *wide = (char *)malloc(needed == 0 ? 1 : needed);
        kiln_value text;
        if (wide == NULL) {
            return kiln_c99_undefined();
        }
        written = kiln_bigint_to_string(handle, radix, (unsigned char *)wide, needed);
        if (written < 0) {
            free(wide);
            return kiln_c99_undefined();
        }
        text = kiln_c99_copy_text(wide, (size_t)written);
        free(wide);
        return text;
    }
}

/* A BigInt literal, parsed on first use and kept.
 *
 * The handle is shared by every load of the same literal, which is sound because a BigInt is
 * immutable: the provider's arithmetic always allocates its answer rather than writing back. */
static int kiln_c99_bigint_constant(void **cache, const char *digits, size_t len,
                                    kiln_value *out) {
    if (*cache == NULL) {
        void *handle = NULL;
        if (kiln_bigint_parse((const unsigned char *)digits, len, 10, &handle) != 0) {
            return KILN_C99_CONTROL_ERROR;
        }
        *cache = handle;
    }
    *out = kiln_c99_bigint_value(*cache);
    return KILN_C99_OK;
}

/* `1n`, built once. Nothing frees a BigInt handle here, so a shared one is safe to keep. */
static const void *kiln_c99_bigint_one(void) {
    static void *one = NULL;
    if (one == NULL) {
        static const unsigned char digit[] = {'1'};
        if (kiln_bigint_parse(digit, sizeof digit, 10, &one) != 0) {
            return NULL;
        }
    }
    return one;
}

/* Wrap a handle the provider just produced. */
static kiln_value kiln_c99_bigint_value(void *handle) {
    return kiln_c99_value(KILN_VALUE_BIGINT, (uint64_t)(uintptr_t)handle, NULL, 0);
}

static kiln_regexp *kiln_c99_regexp_of(kiln_value value) {
    return (kiln_regexp *)(uintptr_t)value.bits;
}

/* Compile a pattern, or report that the engine rejected it.
 *
 * A rejection is a SyntaxError rather than a refusal, because that is what `new RegExp` with the
 * same text produces -- and a literal the frontend parsed can still be one the engine will not
 * take, so the program has to be able to catch it. */
static int kiln_c99_new_regexp(kiln_value *out, const char *source, size_t source_len,
                               const char *flags, size_t flags_len, kiln_value *thrown) {
    kiln_regexp *regexp;
    void *handle = NULL;
    size_t index;
    if (kiln_regexp_compile((const unsigned char *)source, source_len,
                            (const unsigned char *)flags, flags_len, &handle) != 0) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                    "Invalid regular expression");
    }
    regexp = (kiln_regexp *)malloc(sizeof *regexp);
    if (regexp == NULL) {
        kiln_regexp_free(handle);
        return KILN_C99_OUT_OF_MEMORY;
    }
    regexp->handle = handle;
    regexp->source = source;
    regexp->source_len = source_len;
    regexp->flags = flags;
    regexp->flags_len = flags_len;
    regexp->global = 0;
    regexp->sticky = 0;
    regexp->last_index = 0.0;
    for (index = 0; index < flags_len; ++index) {
        if (flags[index] == 'g') {
            regexp->global = 1;
        } else if (flags[index] == 'y') {
            regexp->sticky = 1;
        }
    }
    *out = kiln_c99_value(KILN_VALUE_REGEXP, (uint64_t)(uintptr_t)regexp, NULL, 0);
    return KILN_C99_OK;
}

/* One search, and the match it found.
 *
 * Returns 1 for a match, 0 for none. A sticky pattern must match exactly at the cursor, not merely
 * at or after it -- the engine reports the first match from `start`, and that check is what turns
 * it into the stickier promise. */
static int kiln_c99_regexp_find(kiln_regexp *regexp, const uint16_t *units, size_t count,
                                size_t start, int64_t *ranges, size_t capacity, size_t *groups,
                                int *matched) {
    *matched = 0;
    *groups = 0;
    if (start > count) {
        return KILN_C99_OK;
    }
    if (kiln_regexp_search(regexp->handle, units, count, start, ranges, capacity) != 1) {
        return KILN_C99_OK;
    }
    if (regexp->sticky && ranges[0] != (int64_t)start) {
        return KILN_C99_OK;
    }
    *groups = kiln_regexp_group_count(regexp->handle);
    if (*groups > capacity) {
        *groups = capacity;
    }
    *matched = 1;
    return KILN_C99_OK;
}

static kiln_symbol *kiln_c99_symbol_of(kiln_value value) {
    return (kiln_symbol *)(uintptr_t)value.bits;
}

/* The specification's own symbols. One instance each, so equality is the pointer's exactly as it
 * is for a unique symbol -- there is no second kind of comparison to keep in step. */
static const kiln_symbol kiln_c99_well_known[] = {
    {"@@iterator", 10, "Symbol.iterator", 15},
    {"@@asyncIterator", 15, "Symbol.asyncIterator", 20},
    {"@@hasInstance", 13, "Symbol.hasInstance", 18},
    {"@@toStringTag", 13, "Symbol.toStringTag", 18},
    {"@@toPrimitive", 13, "Symbol.toPrimitive", 18}};

/* The string a value names a property under.
 *
 * Distinct from `kiln_c99_text`: a symbol keys a property under its reserved slot, not under its
 * description, and conflating the two would make `obj[Symbol.iterator]` and
 * `obj["Symbol(Symbol.iterator)"]` the same property. */
static kiln_value kiln_c99_property_key(kiln_value key) {
    if (key.tag == KILN_VALUE_SYMBOL) {
        kiln_symbol *symbol = kiln_c99_symbol_of(key);
        return kiln_c99_value(KILN_VALUE_STRING, 0, symbol->key, symbol->key_len);
    }
    return kiln_c99_text(key);
}

/* `Symbol(description)`. Two calls make two different symbols even given the same description,
 * so the identity is the allocation's and the reserved key carries a serial. */
static int kiln_c99_new_symbol(kiln_value *out, kiln_value description) {
    static uint64_t next_serial;
    kiln_symbol *symbol = (kiln_symbol *)malloc(sizeof *symbol);
    kiln_text key = {NULL, 0, 0};
    char digits[24];
    int written;
    if (symbol == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    next_serial += 1;
    written = kiln_number_to_js_string((double)next_serial, digits, sizeof digits);
    if (written < 0 || kiln_c99_text_append_c(&key, "@@symbol:") != KILN_C99_OK ||
        kiln_c99_text_append(&key, digits, (size_t)written) != KILN_C99_OK) {
        free(key.bytes);
        free(symbol);
        return KILN_C99_OUT_OF_MEMORY;
    }
    symbol->key = key.bytes;
    symbol->key_len = key.len;
    if (description.tag == KILN_VALUE_UNDEFINED) {
        /* `Symbol()` has no description, which prints as `Symbol()` rather than
         * `Symbol(undefined)`. */
        symbol->description = NULL;
        symbol->description_len = 0;
    } else {
        kiln_value text = kiln_c99_text(description);
        if (text.tag != KILN_VALUE_STRING) {
            free(key.bytes);
            free(symbol);
            return KILN_C99_OUT_OF_MEMORY;
        }
        symbol->description = text.ptr;
        symbol->description_len = text.len;
    }
    *out = kiln_c99_value(KILN_VALUE_SYMBOL, (uint64_t)(uintptr_t)symbol, NULL, 0);
    return KILN_C99_OK;
}

/* `call`, `apply` and `bind`, which every callable answers to whatever kind of callable it is.
 * Not a range lookup, because the three are not contiguous with anything and a function's *own*
 * property of the same name has to win -- so each caller asks after its own store. */
static uint64_t kiln_c99_callable_method(const char *key, size_t key_len) {
    if (kiln_c99_same_name(key, key_len, "call")) {
        return KILN_C99_METHOD_FUNCTION_CALL;
    }
    if (kiln_c99_same_name(key, key_len, "apply")) {
        return KILN_C99_METHOD_FUNCTION_APPLY;
    }
    if (kiln_c99_same_name(key, key_len, "bind")) {
        return KILN_C99_METHOD_FUNCTION_BIND;
    }
    return 0;
}

/* A namespace is an object holding functions rather than a callable, which decides both how it
 * prints and what `typeof` answers about it. */
static int kiln_c99_is_namespace(uint64_t builtin) {
    return builtin == KILN_C99_BUILTIN_MATH || builtin == KILN_C99_BUILTIN_JSON ||
           builtin == KILN_C99_BUILTIN_REFLECT ||
           builtin == KILN_C99_BUILTIN_OBJECT_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_ARRAY_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_STRING_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_REGEXP_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_DATE_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_NUMBER_PROTOTYPE ||
           builtin == KILN_C99_BUILTIN_FUNCTION_PROTOTYPE;
}

/* The method a name resolves to on a given receiver, or 0.  One table, so a read and a call
 * cannot disagree about which names exist -- and the range searched is what tells `slice` on an
 * array from `slice` on a string. */
static uint64_t kiln_c99_lookup_method(const char *key, size_t key_len, uint64_t first,
                                       uint64_t last) {
    uint64_t code;
    for (code = first; code <= last; ++code) {
        const char *name = kiln_c99_method_name(code);
        size_t len;
        if (name == NULL) {
            continue;
        }
        len = strlen(name);
        if (len == key_len && memcmp(name, key, len) == 0) {
            return code;
        }
    }
    return 0;
}

static uint64_t kiln_c99_array_method(const char *key, size_t key_len) {
    uint64_t code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_PUSH,
                                           KILN_C99_METHOD_REDUCE);
    if (code != 0) {
        return code;
    }
    /* Two ranges rather than one, because the second had to be numbered above the namespace
     * block. Still one table, which is what keeps a read and a call agreeing about which names
     * exist. */
    return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_ARRAY_AT,
                                  KILN_C99_METHOD_ARRAY_REDUCE_RIGHT);
}

static uint64_t kiln_c99_string_method(const char *key, size_t key_len) {
    uint64_t code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_STRING_CODE_POINT_AT,
                                           KILN_C99_METHOD_STRING_SEARCH);
    if (code != 0) {
        return code;
    }
    return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_STRING_SLICE,
                                  KILN_C99_METHOD_STRING_CONCAT);
}

/* A namespace's own methods, by which namespace it is: `Math.min` and `Number.isNaN` are
 * different functions and the ranges keep them apart. */
static uint64_t kiln_c99_namespace_method(uint64_t builtin, const char *key, size_t key_len) {
    switch (builtin) {
    case KILN_C99_BUILTIN_MATH: {
        uint64_t code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_MATH_RANDOM,
                                               KILN_C99_METHOD_MATH_IMUL);
        if (code != 0) {
            return code;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_MATH_FLOOR,
                                      KILN_C99_METHOD_MATH_ATAN);
    }
    case KILN_C99_BUILTIN_NUMBER:
        /* `Number.parseInt` and the global `parseInt` are the same function in the language, so
         * they are the same code here. Named outside the contiguous range because the range is
         * how a namespace's methods are found, and these two belong to two namespaces. */
        if (kiln_c99_same_name(key, key_len, "parseInt")) {
            return KILN_C99_METHOD_PARSE_INT;
        }
        if (kiln_c99_same_name(key, key_len, "parseFloat")) {
            return KILN_C99_METHOD_PARSE_FLOAT;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_IS_INTEGER,
                                      KILN_C99_METHOD_IS_FINITE);
    case KILN_C99_BUILTIN_OBJECT:
        if (kiln_c99_same_name(key, key_len, "create")) {
            return KILN_C99_METHOD_OBJECT_CREATE;
        }
        if (kiln_c99_same_name(key, key_len, "freeze")) {
            return KILN_C99_METHOD_OBJECT_FREEZE;
        }

        if (kiln_c99_same_name(key, key_len, "isFrozen")) {
            return KILN_C99_METHOD_OBJECT_IS_FROZEN;
        }
        if (kiln_c99_same_name(key, key_len, "getPrototypeOf")) {
            return KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF;
        }
        if (kiln_c99_same_name(key, key_len, "setPrototypeOf")) {
            return KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF;
        }
        if (kiln_c99_same_name(key, key_len, "getOwnPropertyNames")) {
            return KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES;
        }
        if (kiln_c99_same_name(key, key_len, "fromEntries")) {
            return KILN_C99_METHOD_OBJECT_FROM_ENTRIES;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_OBJECT_KEYS,
                                      KILN_C99_METHOD_HAS_OWN);
    case KILN_C99_BUILTIN_ARRAY:
        if (kiln_c99_same_name(key, key_len, "from")) {
            return KILN_C99_METHOD_ARRAY_FROM;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_IS_ARRAY,
                                      KILN_C99_METHOD_ARRAY_OF);
    case KILN_C99_BUILTIN_JSON:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_JSON_STRINGIFY,
                                      KILN_C99_METHOD_JSON_PARSE);
    case KILN_C99_BUILTIN_DATE:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_DATE_NOW,
                                      KILN_C99_METHOD_DATE_PARSE);
    case KILN_C99_BUILTIN_OBJECT_PROTOTYPE:
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            return 0;
        }
        if (kiln_c99_same_name(key, key_len, "isPrototypeOf")) {
            return KILN_C99_METHOD_IS_PROTOTYPE_OF;
        }
        if (kiln_c99_same_name(key, key_len, "propertyIsEnumerable")) {
            return KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                      KILN_C99_METHOD_OBJECT_VALUE_OF);
    case KILN_C99_BUILTIN_ARRAY_PROTOTYPE:
        return kiln_c99_array_method(key, key_len);
    case KILN_C99_BUILTIN_STRING_PROTOTYPE:
        return kiln_c99_string_method(key, key_len);
    case KILN_C99_BUILTIN_REGEXP_PROTOTYPE:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_REGEXP_TEST,
                                      KILN_C99_METHOD_REGEXP_TO_STRING);
    case KILN_C99_BUILTIN_DATE_PROTOTYPE:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_GET_TIME,
                                      KILN_C99_METHOD_DATE_TO_STRING);
    case KILN_C99_BUILTIN_NUMBER_PROTOTYPE:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_NUMBER_TO_STRING,
                                      KILN_C99_METHOD_NUMBER_VALUE_OF);
    case KILN_C99_BUILTIN_FUNCTION_PROTOTYPE:
        return kiln_c99_callable_method(key, key_len);
    case KILN_C99_BUILTIN_PROMISE:
        if (kiln_c99_same_name(key, key_len, "allSettled")) {
            return KILN_C99_METHOD_PROMISE_ALL_SETTLED;
        }
        if (kiln_c99_same_name(key, key_len, "any")) {
            return KILN_C99_METHOD_PROMISE_ANY;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_PROMISE_RESOLVE,
                                      KILN_C99_METHOD_PROMISE_RACE);
    default:
        return 0;
    }
}

static kiln_value kiln_c99_builtin(uint64_t code) {
    return kiln_c99_value(KILN_VALUE_BUILTIN, code, NULL, 0);
}

/* A closure and a builtin are both callable, and every operator that coerces treats them alike:
 * `valueOf` hands the object back, so `toString` decides. */
static int kiln_c99_is_callable(kiln_value value) {
    return value.tag == KILN_VALUE_FUNCTION || value.tag == KILN_VALUE_BUILTIN ||
           value.tag == KILN_VALUE_METHOD || value.tag == KILN_VALUE_SETTLER ||
           value.tag == KILN_VALUE_BOUND;
}

/* Every reference value reduces to a primitive before a coercing operator, and none of them can
 * do it by calling: this backend has no user `valueOf` to reach, because a property holding a
 * function is refused at the point it would be called. So reduction is conversion to text. */
static int kiln_c99_is_reference(kiln_value value) {
    /* What a coercing operator has to reduce to a primitive first. A Date is a reference and is
     * deliberately *not* here: `as_number` already gives its instant, which is what `valueOf`
     * would produce, so reducing it to text first would turn every date subtraction into NaN. */
    return kiln_c99_is_callable(value) || value.tag == KILN_VALUE_OBJECT ||
           value.tag == KILN_VALUE_ARRAY || value.tag == KILN_VALUE_GENERATOR ||
           value.tag == KILN_VALUE_COLLECTION;
}

/* Whether a value has to be converted before it crosses the embedding boundary.
 *
 * A different question from the one above, and asked the other way round on purpose: the
 * boundary carries the five scalar tags and nothing else, so everything above the last of them
 * needs converting. Listing what *cannot* cross means a tag added later is covered the day it is
 * added -- an allowlist of what needs conversion is how `console.log([1])` came to hand the host
 * a raw array, and a Date would have been next. */
static int kiln_c99_crosses_boundary(kiln_value value) {
    return value.tag > KILN_VALUE_STRING;
}

/* Negative zero compares equal to zero, so the sign is the only thing that tells them apart --
 * and a console shows the difference where `String` does not. */
static int kiln_c99_signbit(double value) {
    return signbit(value) != 0;
}

static kiln_value kiln_c99_hole(void) {
    return kiln_c99_value(KILN_VALUE_HOLE, 0, NULL, 0);
}

static int kiln_c99_is_hole(kiln_value value) {
    return value.tag == KILN_VALUE_HOLE;
}

static kiln_value kiln_c99_uninitialized(void) {
    return kiln_c99_value(KILN_VALUE_UNINITIALIZED, 0, NULL, 0);
}

static kiln_value kiln_c99_null(void) {
    return kiln_c99_value(KILN_VALUE_NULL, 0, NULL, 0);
}

static kiln_value kiln_c99_boolean(int value) {
    return kiln_c99_value(KILN_VALUE_BOOLEAN, value ? 1u : 0u, NULL, 0);
}

static kiln_value kiln_c99_function(kiln_closure *closure) {
    return kiln_c99_value(KILN_VALUE_FUNCTION, (uint64_t)(uintptr_t)closure, NULL, 0);
}

static kiln_closure *kiln_c99_closure_of(kiln_value value) {
    return (kiln_closure *)(uintptr_t)value.bits;
}

static kiln_value *kiln_c99_cell_of(kiln_value value) {
    return (kiln_value *)(uintptr_t)value.bits;
}

/* Turn a slot into a box in place.  Whatever the slot already held becomes the box's first
 * contents, which is what makes `let x = 1;` followed by a closure over `x` see the 1. */
static int kiln_c99_make_cell(kiln_value *slot) {
    kiln_value *box = (kiln_value *)malloc(sizeof *box);
    if (box == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    *box = *slot;
    *slot = kiln_c99_value(KILN_VALUE_CELL, (uint64_t)(uintptr_t)box, NULL, 0);
    return KILN_C99_OK;
}

/* The marker goes *through* an existing box rather than over it.  The cell is created before
 * the marker is written -- it has to exist before a hoisted function can capture it -- so
 * overwriting the slot here would drop the box every capture already points at. */
static void kiln_c99_mark_uninitialized(kiln_value *slot) {
    if (slot->tag == KILN_VALUE_CELL) {
        *kiln_c99_cell_of(*slot) = kiln_c99_uninitialized();
    } else {
        *slot = kiln_c99_uninitialized();
    }
}

/* The captured values are boxes, copied by pointer rather than by contents: two closures over one
 * variable have to see each other's writes, which is the whole reason a cell exists. */
static int kiln_c99_make_closure(kiln_value *out, kiln_body_fn body, const char *name,
                                 size_t name_len, uint32_t proto, int generator, int asynchronous,
                                 int async_generator, int arrow, uint32_t local_count,
                                 uint32_t stack_size,
                                 uint32_t handler_depth, const kiln_value *captured,
                                 uint32_t count) {
    kiln_closure *closure = (kiln_closure *)malloc(sizeof *closure);
    kiln_value *upvalues = NULL;
    uint32_t index;
    if (closure == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    if (count != 0) {
        upvalues = (kiln_value *)malloc((size_t)count * sizeof *upvalues);
        if (upvalues == NULL) {
            free(closure);
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < count; ++index) {
            upvalues[index] = captured[index];
        }
    }
    closure->body = body;
    closure->generator = generator;
    closure->asynchronous = asynchronous;
    closure->local_count = local_count;
    closure->stack_size = stack_size;
    closure->handler_depth = handler_depth;
    closure->arrow = arrow;
    closure->async_generator = async_generator;
    /* Captured whether or not it is wanted, because whether it is wanted is a property of the
     * closure and the value is a property of *here*. */
    closure->captured_new_target = kiln_c99_new_target;
    closure->captured_arguments = kiln_c99_undefined();
    if (arrow) {
        /* Only an arrow, and only at creation: the list belongs to a frame that will be gone
         * before the arrow is called, so it has to be copied out now rather than pointed at. */
        int captured = kiln_c99_visible_arguments(&closure->captured_arguments);
        if (captured != KILN_C99_OK) {
            free(closure);
            return captured;
        }
    }
    closure->prototype = kiln_c99_undefined();
    closure->properties = kiln_c99_undefined();
    closure->name = name;
    closure->name_len = name_len;
    closure->proto = proto;
    closure->upvalue_count = count;
    closure->upvalues = upvalues;
    *out = kiln_c99_function(closure);
    return KILN_C99_OK;
}

/* Recursion runs on the C stack here, so depth is carried explicitly and checked before the
 * frame is entered rather than discovered by crashing. */
/* The receiver is `this` inside the body.  A plain call passes undefined, which is what `this`
 * is in a module: modules are strict, and strict mode does not substitute the global object. */
static int kiln_c99_call(kiln_host *host, kiln_value *result, kiln_value callee,
                         kiln_value receiver, const kiln_value *args, size_t argc, int depth,
                         kiln_value *thrown) {
    kiln_closure *closure;
    if (!kiln_c99_is_callable(callee)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "is not a function");
    }
    if (depth >= KILN_C99_MAX_CALL_DEPTH) {
        /* A `RangeError` the program can catch, which is what a real engine raises when the
         * stack runs out -- and what a recursive descent parser catches to report a nesting
         * limit rather than dying. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Maximum call stack size exceeded");
    }
    if (callee.tag == KILN_VALUE_BOUND) {
        /* Binding is transparent to the call: the receiver is the one that was fixed, whatever
         * the target turns out to be, and the fixed arguments come before the call's own. */
        kiln_bound *bound = (kiln_bound *)(uintptr_t)callee.bits;
        kiln_value *joined;
        int status;
        if (bound->count == 0) {
            return kiln_c99_call(host, result, bound->target, bound->receiver, args, argc, depth,
                                 thrown);
        }
        joined = (kiln_value *)malloc((bound->count + argc) * sizeof *joined);
        if (joined == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(joined, bound->arguments, bound->count * sizeof *joined);
        if (argc != 0) {
            memcpy(joined + bound->count, args, argc * sizeof *joined);
        }
        status = kiln_c99_call(host, result, bound->target, bound->receiver, joined,
                               bound->count + argc, depth, thrown);
        free(joined);
        return status;
    }
    if (callee.tag == KILN_VALUE_SETTLER) {
        /* `resolve` and `reject` from an executor. Calling one settles its promise and evaluates
         * to undefined, however many times the program calls it. */
        kiln_settler *settler = (kiln_settler *)(uintptr_t)callee.bits;
        *result = kiln_c99_undefined();
        return kiln_c99_settle(settler->promise, argc > 0 ? args[0] : kiln_c99_undefined(),
                               settler->rejects);
    }
    if (callee.tag == KILN_VALUE_METHOD) {
        return kiln_c99_invoke_method(host, result, callee.bits, receiver, args, argc, depth,
                                      thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN) {
        static const char empty[] = "";
        switch (callee.bits) {
        case KILN_C99_BUILTIN_STRING: {
            /* `String()` with no argument is the empty string, not `"undefined"`. */
            kiln_value subject;
            int status;
            if (argc == 0) {
                *result = kiln_c99_value(KILN_VALUE_STRING, 0, empty, 0);
                return KILN_C99_OK;
            }
            /* The *string* hint: `toString` before `valueOf`, which is the other way round from
             * every operator. `${x}` compiles to this call for exactly that reason -- a template
             * is not `"" + x`, and for an object with both methods the two disagree. */
            status = kiln_c99_to_primitive(host, args[0], 1, &subject, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_text(subject);
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_NUMBER: {
            double converted;
            int status;
            /* `Number()` with no argument is zero, not NaN: there is nothing to fail to convert. */
            if (argc == 0) {
                *result = kiln_c99_number(0.0);
                return KILN_C99_OK;
            }
            /* The *number* hint, the reverse of the `String` builtin above: `Number([6])` is 6
             * because an array declines `valueOf` and its `toString` answers "6". */
            status = kiln_c99_to_number_checked(host, args[0], &converted, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_number(converted);
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_BIGINT: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_number(0.0);
            void *handle = NULL;
            if (subject.tag == KILN_VALUE_BIGINT) {
                *result = subject;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_STRING) {
                /* A string that is not an integer is a SyntaxError, unlike `Number("x")` which
                 * is NaN -- a BigInt has no NaN to be. */
                if (kiln_bigint_parse((const unsigned char *)subject.ptr, subject.len, 10,
                                      &handle) != 0) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_SYNTAX_ERROR,
                                                "Cannot convert this string to a BigInt");
                }
                *result = kiln_c99_bigint_value(handle);
                return KILN_C99_OK;
            }
            /* Only an exact integer converts: `BigInt(1.5)` is a RangeError rather than a
             * rounding, because rounding is the thing this type exists to avoid. */
            if (kiln_bigint_from_double(kiln_c99_as_number(subject), &handle) != 0) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "The number is not a safe integer");
            }
            *result = kiln_c99_bigint_value(handle);
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_SYMBOL:
            return kiln_c99_new_symbol(result, argc > 0 ? args[0] : kiln_c99_undefined());
        case KILN_C99_BUILTIN_REGEXP: {
            /* `RegExp(re)` keeps the original's source *and* its flags unless new ones are
             * given, which is the difference from `new RegExp(String(re))`. */
            kiln_value pattern = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value source;
            kiln_value flags;
            if (pattern.tag == KILN_VALUE_REGEXP) {
                kiln_regexp *existing = kiln_c99_regexp_of(pattern);
                source = kiln_c99_value(KILN_VALUE_STRING, 0, existing->source,
                                        existing->source_len);
                flags = argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED
                            ? kiln_c99_text(args[1])
                            : kiln_c99_value(KILN_VALUE_STRING, 0, existing->flags,
                                             existing->flags_len);
            } else {
                source = argc > 0 ? kiln_c99_text(pattern) : kiln_c99_from_units(NULL, 0);
                flags = argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED
                            ? kiln_c99_text(args[1])
                            : kiln_c99_from_units(NULL, 0);
            }
            if (source.tag != KILN_VALUE_STRING || flags.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            return kiln_c99_new_regexp(result, source.ptr, source.len, flags.ptr, flags.len,
                                       thrown);
        }
        case KILN_C99_BUILTIN_BOOLEAN:
            /* `Boolean(x)` is `!!x` spelled as a call, and `Boolean()` is false. */
            *result = kiln_c99_boolean(
                argc == 0 ? 0 : kiln_c99_truthy(args[0]));
            return KILN_C99_OK;
        default:
            return KILN_C99_CONTROL_ERROR;
        }
    }
    closure = kiln_c99_closure_of(callee);
    if (closure->generator) {
        /* Calling a generator function does not run it: it builds the object that will. An
         * async function is compiled as one -- `await` suspends exactly as `yield` does -- and
         * differs only in who drives it, which is this rather than the program. */
        kiln_value built;
        int status = kiln_c99_new_generator(&built, callee, receiver, args, argc);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!closure->asynchronous || closure->async_generator) {
            /* An async generator is handed over too: it awaits *and* yields, so nothing can drive
             * it to completion -- each `next` drives one step and gets a promise of its own. */
            if (closure->async_generator) {
                kiln_c99_generator_of(built)->async_generator = 1;
            }
            *result = built;
            return KILN_C99_OK;
        }
        {
            kiln_generator *frame = kiln_c99_generator_of(built);
            frame->is_async = 1;
            status = kiln_c99_new_promise(&frame->promise);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = frame->promise;
            /* The body starts *now*, synchronously, and runs until its first `await` -- which is
             * what makes the code before the first one behave like an ordinary call. */
            return kiln_c99_async_step(host, built, kiln_c99_undefined(), 0, depth, thrown);
        }
    }
    {
        /* `new.target` is the constructor this call named -- nothing, for an ordinary call, and an
         * arrow's captured one, because an arrow has none of its own. `super(...)` is the one call
         * that keeps what it found: the base constructor is being run *for* the subclass. */
        kiln_value saved = kiln_c99_new_target;
        const kiln_value *saved_args = kiln_c99_frame_args;
        size_t saved_argc = kiln_c99_frame_argc;
        kiln_value saved_captured = kiln_c99_captured_arguments;
        int saved_is_arrow = kiln_c99_frame_is_arrow;
        int status;
        if (kiln_c99_inheriting_new_target) {
            kiln_c99_inheriting_new_target = 0;
        } else {
            kiln_c99_new_target =
                closure->arrow ? closure->captured_new_target : kiln_c99_undefined();
        }
        kiln_c99_frame_is_arrow = closure->arrow;
        if (closure->arrow) {
            kiln_c99_captured_arguments = closure->captured_arguments;
        } else {
            kiln_c99_frame_args = args;
            kiln_c99_frame_argc = argc;
        }
        status = closure->body(host, result, closure->upvalues, receiver, args, argc, depth + 1,
                               thrown, NULL);
        kiln_c99_new_target = saved;
        kiln_c99_frame_args = saved_args;
        kiln_c99_frame_argc = saved_argc;
        kiln_c99_captured_arguments = saved_captured;
        kiln_c99_frame_is_arrow = saved_is_arrow;
        return status;
    }
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
    case KILN_VALUE_BIGINT:
        /* Lossy on purpose and only where the language asks for it: `Number(big)` and a
         * comparison against a number. Arithmetic never comes through here -- mixing is a
         * TypeError, which is what stops the precision being lost quietly. */
        return kiln_bigint_to_double((const void *)(uintptr_t)value.bits);
    case KILN_VALUE_NULL:
        return 0.0;
    case KILN_VALUE_BOOLEAN:
        return value.bits ? 1.0 : 0.0;
    case KILN_VALUE_NUMBER:
        memcpy(&number, &value.bits, sizeof(number));
        return number;
    case KILN_VALUE_STRING:
        return kiln_string_to_number(value.ptr, value.len);
    /* `end - start` on two dates is the difference in milliseconds, which is the whole reason
     * `valueOf` on a Date is its instant. */
    case KILN_VALUE_DATE:
        return kiln_c99_date_time(value);
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
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_BUILTIN:
    case KILN_VALUE_METHOD:
    case KILN_VALUE_SYMBOL:
    /* An object is always truthy, empty or not. */
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_DATE:
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_REGEXP:
    case KILN_VALUE_PROMISE:
        return 1;
    case KILN_VALUE_BIGINT:
        /* Truthiness follows zero, not sign: `-1n` is truthy and `0n` is not. */
        return !kiln_bigint_is_zero((const void *)(uintptr_t)value.bits);
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


static int kiln_c99_text_append(kiln_text *text, const char *bytes, size_t len) {
    int status;
    if (len == 0) {
        return KILN_C99_OK;
    }
    status = kiln_c99_reserve((void **)&text->bytes, &text->capacity, text->len + len, 1);
    if (status != KILN_C99_OK) {
        return status;
    }
    memcpy(text->bytes + text->len, bytes, len);
    text->len += len;
    return KILN_C99_OK;
}

static int kiln_c99_text_append_c(kiln_text *text, const char *literal) {
    return kiln_c99_text_append(text, literal, strlen(literal));
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
        int len = kiln_number_to_js_string(number, buffer, sizeof(buffer));
        if (len < 0 || (size_t)len >= sizeof(buffer)) {
            return kiln_c99_undefined();
        }
        return kiln_c99_copy_text(buffer, (size_t)len);
    }
    case KILN_VALUE_OBJECT: {
        static const char object_text[] = "[object Object]";
        kiln_value kind;
        kiln_value shown;
        /* An error converts to `Name: message`, not `[object Object]`: it is the one object with
         * a text form of its own, and every log line that prints one depends on it. */
        if (kiln_c99_get(NULL, value, "@@error", 7, &kind, 0, NULL) == KILN_C99_OK &&
            kind.tag == KILN_VALUE_NUMBER &&
            kiln_c99_get(NULL, value, "stack", 5, &shown, 0, NULL) == KILN_C99_OK &&
            shown.tag == KILN_VALUE_STRING) {
            return shown;
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, object_text, sizeof(object_text) - 1);
    }
    case KILN_VALUE_DATE: {
        /* `String(date)` is the human form, not the ISO one: the hint is a string and
         * `toString` decides. Arithmetic reaches the instant through `as_number` instead. */
        kiln_value shown;
        if (kiln_c99_date_to_string(&shown, kiln_c99_date_time(value)) != KILN_C99_OK) {
            return kiln_c99_undefined();
        }
        return shown;
    }
    case KILN_VALUE_ARRAY: {
        /* `String([1, , 3])` is "1,,3": the separator is a comma and a nullish element
         * contributes nothing, which is `Array.prototype.join` with its default. */
        kiln_array *array = kiln_c99_array_of(value);
        kiln_text joined = {NULL, 0, 0};
        size_t index;
        for (index = 0; index < array->count; ++index) {
            kiln_value element = array->elements[index];
            if (index != 0 && kiln_c99_text_append(&joined, ",", 1) != KILN_C99_OK) {
                free(joined.bytes);
                return kiln_c99_undefined();
            }
            if (element.tag == KILN_VALUE_UNDEFINED || element.tag == KILN_VALUE_NULL ||
                kiln_c99_is_hole(element)) {
                continue;
            }
            {
                kiln_value shown = kiln_c99_text(element);
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&joined, shown.ptr, shown.len) != KILN_C99_OK) {
                    free(joined.bytes);
                    return kiln_c99_undefined();
                }
            }
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, joined.bytes, joined.len);
    }
    case KILN_VALUE_BIGINT: {
        /* No `n` suffix: that is the literal's spelling, not the value's -- `String(1n)` is
         * `"1"`. A console adds it back, which is a different question and answered there. */
        return kiln_c99_bigint_text(value, 10);
    }
    case KILN_VALUE_REGEXP: {
        /* `String(/a/g)` is the literal as written, which is what `toString` returns. */
        kiln_regexp *regexp = kiln_c99_regexp_of(value);
        kiln_text built = {NULL, 0, 0};
        if (kiln_c99_text_append_c(&built, "/") != KILN_C99_OK ||
            kiln_c99_text_append(&built, regexp->source, regexp->source_len) != KILN_C99_OK ||
            kiln_c99_text_append_c(&built, "/") != KILN_C99_OK ||
            kiln_c99_text_append(&built, regexp->flags, regexp->flags_len) != KILN_C99_OK) {
            free(built.bytes);
            return kiln_c99_undefined();
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
    }
    case KILN_VALUE_SYMBOL: {
        /* `String(sym)` spells the symbol out. Concatenation is a different question -- the
         * language throws for `sym + ""` -- and neither engine draws that line yet. */
        kiln_symbol *symbol = kiln_c99_symbol_of(value);
        kiln_text built = {NULL, 0, 0};
        if (kiln_c99_text_append_c(&built, "Symbol(") != KILN_C99_OK ||
            kiln_c99_text_append(&built, symbol->description, symbol->description_len) !=
                KILN_C99_OK ||
            kiln_c99_text_append_c(&built, ")") != KILN_C99_OK) {
            free(built.bytes);
            return kiln_c99_undefined();
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
    }
    case KILN_VALUE_METHOD:
    case KILN_VALUE_BUILTIN: {
        /* The interpreter names a runtime-supplied callable by the native it resolves to, and
         * this has to be the same text for the same program. A namespace converts as the object
         * it is -- `String(Math)` is `[object Math]`, not a function's name. */
        kiln_text built = {NULL, 0, 0};
        const char *name = value.tag == KILN_VALUE_METHOD ? kiln_c99_method_name(value.bits)
                                                          : kiln_c99_builtin_name(value.bits);
        if (name == NULL) {
            return kiln_c99_undefined();
        }
        if (kiln_c99_text_append_c(&built, value.tag == KILN_VALUE_BUILTIN &&
                                                   kiln_c99_is_namespace(value.bits)
                                               ? "[object "
                                               : "[Function: ") != KILN_C99_OK ||
            kiln_c99_text_append_c(&built, name) != KILN_C99_OK ||
            kiln_c99_text_append_c(&built, "]") != KILN_C99_OK) {
            free(built.bytes);
            return kiln_c99_undefined();
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
    }
    case KILN_VALUE_FUNCTION: {
        /* `Function.prototype.toString` returns the source text, which needs the span and the
         * original source -- neither of which reaches a backend.  Naming the body by its index
         * is what the interpreter does, and the index is the same on both sides. */
        static const char prefix[] = "[Function ";
        char buffer[80];
        int len = kiln_number_to_js_string((double)kiln_c99_closure_of(value)->proto,
                                           buffer + sizeof(prefix) - 1,
                                           sizeof(buffer) - sizeof(prefix));
        if (len < 0) {
            return kiln_c99_undefined();
        }
        memcpy(buffer, prefix, sizeof(prefix) - 1);
        buffer[sizeof(prefix) - 1 + (size_t)len] = ']';
        return kiln_c99_copy_text(buffer, sizeof(prefix) + (size_t)len);
    }
    default:
        return kiln_c99_undefined();
    }
}

static kiln_value kiln_c99_concat(kiln_value left, kiln_value right) {
    kiln_value l = kiln_c99_text(left);
    kiln_value r = kiln_c99_text(right);
    char *copy;
    if ((l.len != 0 && l.ptr == NULL) || (r.len != 0 && r.ptr == NULL) ||
        l.len > (size_t)-1 - r.len) {
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
    if (left.tag == KILN_VALUE_BIGINT) {
        /* A BigInt is a primitive: two are equal when the numbers are, however they were built.
         * `1n === 1n` is true and the two handles are different allocations. */
        return kiln_bigint_compare((const void *)(uintptr_t)left.bits,
                                   (const void *)(uintptr_t)right.bits) == 0;
    }
    if (left.tag == KILN_VALUE_STRING) {
        return left.len == right.len &&
               (left.len == 0 || (left.ptr != NULL && right.ptr != NULL &&
                                  memcmp(left.ptr, right.ptr, left.len) == 0));
    }
    return left.bits == right.bits;
}

static int kiln_c99_loose_equal(kiln_value left, kiln_value right) {
    if (left.tag == right.tag) {
        return kiln_c99_strict_equal(left, right);
    }
    if ((left.tag == KILN_VALUE_UNDEFINED || left.tag == KILN_VALUE_NULL) &&
        (right.tag == KILN_VALUE_UNDEFINED || right.tag == KILN_VALUE_NULL)) {
        return 1;
    }
    if (left.tag == KILN_VALUE_UNDEFINED || left.tag == KILN_VALUE_NULL ||
        right.tag == KILN_VALUE_UNDEFINED || right.tag == KILN_VALUE_NULL) {
        return 0;
    }
    if (left.tag == KILN_VALUE_BOOLEAN) {
        left = kiln_c99_number(left.bits != 0 ? 1.0 : 0.0);
    }
    if (right.tag == KILN_VALUE_BOOLEAN) {
        right = kiln_c99_number(right.bits != 0 ? 1.0 : 0.0);
    }
    if ((left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_STRING) ||
        (left.tag == KILN_VALUE_STRING && right.tag == KILN_VALUE_NUMBER)) {
        return kiln_c99_as_number(left) == kiln_c99_as_number(right);
    }
    /* `==` crosses the numeric types where `===` does not: `1n == 1` is true and `1n === 1` is
     * false, which is the whole reason both operators exist. */
    if (left.tag == KILN_VALUE_BIGINT || right.tag == KILN_VALUE_BIGINT) {
        if (left.tag == KILN_VALUE_NUMBER || right.tag == KILN_VALUE_NUMBER ||
            left.tag == KILN_VALUE_STRING || right.tag == KILN_VALUE_STRING) {
            return kiln_c99_as_number(left) == kiln_c99_as_number(right);
        }
    }
    return kiln_c99_strict_equal(left, right);
}

static uint32_t kiln_c99_to_uint32(double value) {
    double wrapped;
    if (!isfinite(value) || value == 0.0) {
        return UINT32_C(0);
    }
    wrapped = fmod(trunc(value), 4294967296.0);
    if (wrapped < 0.0) {
        wrapped += 4294967296.0;
    }
    return (uint32_t)wrapped;
}

static double kiln_c99_signed32(uint32_t value) {
    return value <= UINT32_C(0x7fffffff)
        ? (double)value
        : (double)value - 4294967296.0;
}

static uint32_t kiln_c99_shift_right(uint32_t value, uint32_t count) {
    count &= UINT32_C(31);
    if (count == 0 || (value & UINT32_C(0x80000000)) == 0) {
        return value >> count;
    }
    return (value >> count) | (~UINT32_C(0) << (UINT32_C(32) - count));
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

/* `length` in UTF-16 code units, which is what JavaScript counts and is neither bytes nor
 * characters. */
static size_t kiln_c99_string_length(kiln_value value) {
    kiln_c99_utf16_iter iterator = {value.ptr, value.len, 0, 0, 0};
    size_t units = 0;
    uint16_t unit;
    while (kiln_c99_utf16_next(&iterator, &unit)) {
        units += 1;
    }
    return units;
}

/* An array index, or -1 for anything else.  A key is an index when it round-trips through the
 * canonical decimal spelling, which is what keeps `a["01"]` a named property and `a["1"]` an
 * element -- the distinction the language draws and the one a naive `atoi` erases. */
static double kiln_c99_array_index(kiln_value key) {
    double number;
    if (key.tag == KILN_VALUE_NUMBER) {
        number = kiln_c99_as_number(key);
    } else if (key.tag == KILN_VALUE_STRING) {
        char buffer[64];
        int length;
        number = kiln_string_to_number(key.ptr, key.len);
        if (!isfinite(number) || number < 0.0 || floor(number) != number) {
            return -1.0;
        }
        length = kiln_number_to_js_string(number, buffer, sizeof(buffer));
        if (length < 0 || (size_t)length != key.len ||
            memcmp(buffer, key.ptr, key.len) != 0) {
            return -1.0;
        }
        return number;
    } else {
        return -1.0;
    }
    if (!isfinite(number) || number < 0.0 || floor(number) != number ||
        number > 4294967294.0) {
        return -1.0;
    }
    return number;
}

/* The object `new f()` gives instances, created on first use.
 *
 * It carries `constructor` back to the function, which is what makes `x.constructor === C` true
 * and what a console reads to print `C {}` rather than `{}`. The two point at each other, so a
 * constructor that is never collected keeps its prototype alive -- and nothing here collects. */
static int kiln_c99_prototype_object(kiln_value function, kiln_value *out) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    if (closure->prototype.tag == KILN_VALUE_UNDEFINED) {
        int status = kiln_c99_new_object(&closure->prototype);
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_put(NULL, closure->prototype, "constructor", 11, function, 0, NULL);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    *out = closure->prototype;
    return KILN_C99_OK;
}

/* The store `f.x = 1` writes into. Separate from the prototype object: a static belongs to the
 * function and an instance method belongs to what it constructs. */
static int kiln_c99_function_store(kiln_value function, kiln_value *out) {
    kiln_closure *closure = kiln_c99_closure_of(function);
    if (closure->properties.tag == KILN_VALUE_UNDEFINED) {
        int status = kiln_c99_new_object(&closure->properties);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    *out = closure->properties;
    return KILN_C99_OK;
}

/* Read a named property.  A missing one is undefined; a read *through* undefined or null is an
 * error, because that is a TypeError in the language and answering undefined would hide it. */
static int kiln_c99_get_own_kind(kiln_host *host, kiln_value target, const char *key,
                                 size_t key_len, kiln_value *out, int depth, kiln_value *thrown) {
    static const char length_key[] = "length";
    size_t index;
    int is_length = key_len == sizeof(length_key) - 1 &&
                    memcmp(key, length_key, sizeof(length_key) - 1) == 0;
    switch (target.tag) {
    case KILN_VALUE_OBJECT: {
        kiln_value current = target;
        size_t steps;
        /* Own properties first, then the chain. Iterative and bounded rather than recursive:
         * the walk is on the C stack, and a program can build a chain long enough to exhaust
         * it. */
        for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
            kiln_object *object;
            /* A constructor's statics live in its own store, so a chain that reaches one
             * continues *into* that store rather than stopping at the function. */
            if (current.tag == KILN_VALUE_FUNCTION) {
                current = kiln_c99_closure_of(current)->properties;
            }
            if (current.tag != KILN_VALUE_OBJECT) {
                break;
            }
            object = kiln_c99_object_of(current);
            for (index = 0; index < object->count; ++index) {
                if (!kiln_c99_same_key(&object->properties[index], key, key_len)) {
                    continue;
                }
                if (!object->properties[index].accessor) {
                    *out = object->properties[index].value;
                    return KILN_C99_OK;
                }
                /* Reading an accessor is a call, and the receiver is the object the read
                 * started from -- not the one the accessor was found on. */
                if (object->properties[index].getter.tag == KILN_VALUE_UNDEFINED) {
                    *out = kiln_c99_undefined();
                    return KILN_C99_OK;
                }
                return kiln_c99_call(host, out, object->properties[index].getter, target, NULL, 0,
                                     depth, thrown);
            }
            current = object->prototype;
        }
        /* The chain ends at `Object.prototype`, whose members every object has -- unless the
         * program asked for a bare dictionary with `Object.create(null)`, which is exactly what
         * that asks for. */
        if (kiln_c99_object_of(target)->prototype.tag != KILN_VALUE_NULL) {
            uint64_t inherited = kiln_c99_same_name(key, key_len, "isPrototypeOf")
                                     ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                                 : kiln_c99_same_name(key, key_len, "propertyIsEnumerable")
                                     ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                                     : kiln_c99_lookup_method(key, key_len,
                                                              KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                                              KILN_C99_METHOD_OBJECT_VALUE_OF);
            if (inherited != 0) {
                *out = kiln_c99_method(inherited);
                return KILN_C99_OK;
            }
            if (kiln_c99_same_name(key, key_len, "constructor")) {
                /* An error already records its kind, and `error.constructor.name` is the ordinary
                 * way a program asks which one it caught -- so the answer comes from the kind
                 * rather than from the fact that an error is an object. */
                kiln_object *own = kiln_c99_object_of(target);
                size_t slot;
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT);
                for (slot = 0; slot < own->count; ++slot) {
                    if (kiln_c99_same_key(&own->properties[slot], "@@error", 7)) {
                        *out = kiln_c99_builtin(
                            (uint64_t)kiln_c99_as_number(own->properties[slot].value));
                        break;
                    }
                }
                return KILN_C99_OK;
            }
        }
        *out = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    case KILN_VALUE_FUNCTION: {
        kiln_closure *closure = kiln_c99_closure_of(target);
        if (kiln_c99_same_name(key, key_len, "prototype")) {
            return kiln_c99_prototype_object(target, out);
        }
        if (kiln_c99_same_name(key, key_len, "name")) {
            *out = closure->name == NULL ? kiln_c99_from_units(NULL, 0)
                                         : kiln_c99_copy_text(closure->name, closure->name_len);
            return KILN_C99_OK;
        }
        if (closure->properties.tag == KILN_VALUE_OBJECT) {
            kiln_object *store = kiln_c99_object_of(closure->properties);
            size_t index;
            for (index = 0; index < store->count; ++index) {
                if (kiln_c99_same_key(&store->properties[index], key, key_len)) {
                    /* An assigned property wins, exactly as an own property shadows an
                     * inherited one -- which is what `Function.prototype.call` is. */
                    return kiln_c99_get(host, closure->properties, key, key_len, out, depth,
                                        thrown);
                }
            }
        }
        {
            uint64_t callable = kiln_c99_callable_method(key, key_len);
            if (callable != 0) {
                *out = kiln_c99_method(callable);
                return KILN_C99_OK;
            }
        }
        if (closure->properties.tag != KILN_VALUE_OBJECT) {
            *out = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        return kiln_c99_get(host, closure->properties, key, key_len, out, depth, thrown);
    }
    case KILN_VALUE_REGEXP: {
        kiln_regexp *regexp = kiln_c99_regexp_of(target);
        size_t index;
        /* The flag letters, in the order the specification lists their accessors. */
        static const char known[] = "dgimsuvy";
        static const char *const names[] = {"hasIndices", "global",  "ignoreCase", "multiline",
                                            "dotAll",     "unicode", "unicodeSets", "sticky"};
        if (kiln_c99_same_name(key, key_len, "source")) {
            *out = kiln_c99_value(KILN_VALUE_STRING, 0, regexp->source, regexp->source_len);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "flags")) {
            *out = kiln_c99_value(KILN_VALUE_STRING, 0, regexp->flags, regexp->flags_len);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "lastIndex")) {
            *out = kiln_c99_number(regexp->last_index);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP);
            return KILN_C99_OK;
        }
        /* Each flag is a boolean named after itself, read off the flag *string* rather than from
         * a field per flag -- so a flag the backend does nothing else with still reads back. */
        for (index = 0; index + 1 < sizeof known; ++index) {
            size_t at;
            int present = 0;
            if (!kiln_c99_same_name(key, key_len, names[index])) {
                continue;
            }
            for (at = 0; at < regexp->flags_len; ++at) {
                present = present || regexp->flags[at] == known[index];
            }
            *out = kiln_c99_boolean(present);
            return KILN_C99_OK;
        }
        {
            uint64_t code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_REGEXP_TEST,
                                                   KILN_C99_METHOD_REGEXP_TO_STRING);
            if (code != 0) {
                *out = kiln_c99_method(code);
                return KILN_C99_OK;
            }
        }
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_ARRAY: {
        uint64_t code;
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY);
            return KILN_C99_OK;
        }
        if (is_length) {
            *out = kiln_c99_number((double)kiln_c99_array_of(target)->count);
            return KILN_C99_OK;
        }
        /* `for...of` asks for this and then drives `next`, where spread walks the array
         * directly. Both have to work, and they are different code paths, so an array needs the
         * protocol spelled out even though nothing else about it is unusual. */
        if (kiln_c99_same_name(key, key_len, "@@iterator") ||
            kiln_c99_same_name(key, key_len, "values")) {
            *out = kiln_c99_method(KILN_C99_METHOD_ARRAY_VALUES);
            return KILN_C99_OK;
        }
        code = kiln_c99_array_method(key, key_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        {
            /* A property that is not an element: `a.total`, or the `index` a match result
             * carries. Only a key the store actually holds is answered, so a *missing* name is
             * still the error below rather than undefined. */
            kiln_array *self_array = kiln_c99_array_of(target);
            if (self_array->properties.tag == KILN_VALUE_OBJECT) {
                kiln_object *store = kiln_c99_object_of(self_array->properties);
                size_t slot;
                for (slot = 0; slot < store->count; ++slot) {
                    if (kiln_c99_same_key(&store->properties[slot], key, key_len)) {
                        *out = store->properties[slot].value;
                        return KILN_C99_OK;
                    }
                }
            }
        }
        /* A method this backend does not have.  Answering undefined would turn `a.sort(f)` into
         * "undefined is not a function" three frames from the cause, so it says so here. */
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_BOOLEAN:
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_BOOLEAN);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "toString") ||
            kiln_c99_same_name(key, key_len, "toLocaleString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_PRIMITIVE_TO_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "valueOf")) {
            *out = kiln_c99_method(KILN_C99_METHOD_OBJECT_VALUE_OF);
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    case KILN_VALUE_STRING: {
        uint64_t code;
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "toString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_PRIMITIVE_TO_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "valueOf")) {
            *out = kiln_c99_method(KILN_C99_METHOD_OBJECT_VALUE_OF);
            return KILN_C99_OK;
        }
        if (is_length) {
            *out = kiln_c99_number((double)kiln_c99_string_length(target));
            return KILN_C99_OK;
        }
        code = kiln_c99_string_method(key, key_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_PROMISE: {
        uint64_t code;
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE);
            return KILN_C99_OK;
        }
        code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_THEN,
                                      KILN_C99_METHOD_FINALLY);
        if (code == 0) {
            return KILN_C99_CONTROL_ERROR;
        }
        *out = kiln_c99_method(code);
        return KILN_C99_OK;
    }
    case KILN_VALUE_COLLECTION: {
        kiln_collection *collection = kiln_c99_collection_of(target);
        uint64_t code;
        if (kiln_c99_same_name(key, key_len, "size")) {
            *out = kiln_c99_number((double)collection->live);
            return KILN_C99_OK;
        }
        code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_COLLECTION_GET,
                                      KILN_C99_METHOD_COLLECTION_SELF);
        /* A Set has no `get` or `set`, and a Map has no `add`: offering one would let a program
         * call something that has no meaning for the receiver it was reached through. */
        if (code == 0 ||
            (collection->is_map && code == KILN_C99_METHOD_COLLECTION_ADD) ||
            (!collection->is_map && (code == KILN_C99_METHOD_COLLECTION_GET ||
                                     code == KILN_C99_METHOD_COLLECTION_SET))) {
            return KILN_C99_CONTROL_ERROR;
        }
        *out = kiln_c99_method(code);
        return KILN_C99_OK;
    }
    case KILN_VALUE_DATE: {
        static const struct {
            const char *name;
            uint64_t code;
        } ALIASES[] = {
            {"valueOf", KILN_C99_METHOD_GET_TIME},
            {"getUTCFullYear", KILN_C99_METHOD_GET_FULL_YEAR},
            {"getUTCMonth", KILN_C99_METHOD_GET_MONTH},
            {"getUTCDate", KILN_C99_METHOD_GET_DATE},
            {"getUTCDay", KILN_C99_METHOD_GET_DAY},
            {"getUTCHours", KILN_C99_METHOD_GET_HOURS},
            {"getUTCMinutes", KILN_C99_METHOD_GET_MINUTES},
            {"getUTCSeconds", KILN_C99_METHOD_GET_SECONDS},
            {"getUTCMilliseconds", KILN_C99_METHOD_GET_MILLISECONDS},
            {"toUTCString", KILN_C99_METHOD_DATE_TO_STRING},
            {"toLocaleString", KILN_C99_METHOD_DATE_TO_STRING},
            {"toLocaleDateString", KILN_C99_METHOD_DATE_TO_STRING},
            {"toLocaleTimeString", KILN_C99_METHOD_DATE_TO_STRING},
            {"toDateString", KILN_C99_METHOD_DATE_TO_STRING},
            {"toTimeString", KILN_C99_METHOD_DATE_TO_STRING},
        };
        uint64_t code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_GET_TIME,
                                               KILN_C99_METHOD_DATE_TO_STRING);
        size_t alias;
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_DATE);
            return KILN_C99_OK;
        }
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        for (alias = 0; alias < sizeof ALIASES / sizeof *ALIASES; ++alias) {
            if (kiln_c99_same_name(key, key_len, ALIASES[alias].name)) {
                *out = kiln_c99_method(ALIASES[alias].code);
                return KILN_C99_OK;
            }
        }
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_GENERATOR: {
        uint64_t code;
        if (kiln_c99_same_name(key, key_len, "@@asyncIterator")) {
            /* Only an async generator has one, and answering for a sync one would make
             * `for await` take the wrong loop. */
            if (!kiln_c99_generator_of(target)->async_generator) {
                return KILN_C99_CONTROL_ERROR;
            }
            *out = kiln_c99_method(KILN_C99_METHOD_GENERATOR_SELF);
            return KILN_C99_OK;
        }
        code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_GENERATOR_NEXT,
                                      KILN_C99_METHOD_GENERATOR_SELF);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_BOUND:
    case KILN_VALUE_METHOD: {
        uint64_t callable;
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION);
            return KILN_C99_OK;
        }
        callable = kiln_c99_callable_method(key, key_len);
        if (callable != 0) {
            *out = kiln_c99_method(callable);
            return KILN_C99_OK;
        }
        if (target.tag == KILN_VALUE_BOUND) {
            return KILN_C99_CONTROL_ERROR;
        }
        /* `parseInt.name`. A method has no other property worth answering, and a wrong answer
         * here reads as a missing function three frames later. */
        if (kiln_c99_same_name(key, key_len, "name")) {
            const char *method = kiln_c99_method_name(target.bits);
            *out = method == NULL ? kiln_c99_from_units(NULL, 0)
                                  : kiln_c99_copy_text(method, strlen(method));
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_BUILTIN: {
        uint64_t code = kiln_c99_namespace_method(target.bits, key, key_len);
        if (code == 0 && !kiln_c99_is_namespace(target.bits)) {
            /* A constructor is a function, so it answers to these too. A namespace is not. */
            code = kiln_c99_callable_method(key, key_len);
        }
        if (code == 0) {
            /* ...and everything inherits from `Object.prototype`, which is where
             * `Object.hasOwnProperty.call(x, k)` finds its function. Only the three a program
             * borrows: `toString` and `valueOf` would shadow what a constructor already answers
             * with, and answering `[object Function]` for `String(Object)` would be worse than
             * not answering at all. */
            code = kiln_c99_same_name(key, key_len, "hasOwnProperty") ? KILN_C99_METHOD_HAS_OWN_PROPERTY
                   : kiln_c99_same_name(key, key_len, "isPrototypeOf") ? KILN_C99_METHOD_IS_PROTOTYPE_OF
                   : kiln_c99_same_name(key, key_len, "propertyIsEnumerable")
                       ? KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE
                       : 0;
        }
        /* `RangeError.name` is how `error.constructor.name` gets its answer, which is the ordinary
         * way a program asks what kind of error it caught. A namespace is not a constructor and
         * has no name to give. */
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            /* A prototype table names its own constructor, which is how `[].constructor` and
             * `Array.prototype.constructor` reach the same value. */
            static const struct {
                uint64_t table;
                uint64_t constructor;
            } owners[] = {{KILN_C99_BUILTIN_OBJECT_PROTOTYPE, KILN_C99_BUILTIN_OBJECT},
                          {KILN_C99_BUILTIN_ARRAY_PROTOTYPE, KILN_C99_BUILTIN_ARRAY},
                          {KILN_C99_BUILTIN_STRING_PROTOTYPE, KILN_C99_BUILTIN_STRING},
                          {KILN_C99_BUILTIN_REGEXP_PROTOTYPE, KILN_C99_BUILTIN_REGEXP},
                          {KILN_C99_BUILTIN_DATE_PROTOTYPE, KILN_C99_BUILTIN_DATE},
                          {KILN_C99_BUILTIN_NUMBER_PROTOTYPE, KILN_C99_BUILTIN_NUMBER},
                          {KILN_C99_BUILTIN_FUNCTION_PROTOTYPE, KILN_C99_BUILTIN_FUNCTION}};
            size_t owner;
            for (owner = 0; owner < sizeof owners / sizeof *owners; ++owner) {
                if (target.bits == owners[owner].table) {
                    *out = kiln_c99_builtin(owners[owner].constructor);
                    return KILN_C99_OK;
                }
            }
        }
        if (!kiln_c99_is_namespace(target.bits) && kiln_c99_same_name(key, key_len, "name")) {
            const char *builtin = kiln_c99_builtin_name(target.bits);
            *out = kiln_c99_copy_text(builtin, strlen(builtin));
            return KILN_C99_OK;
        }
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        /* A namespace's constants are values rather than methods, so they are answered here
         * instead of through a call. */
        if (target.bits == KILN_C99_BUILTIN_SYMBOL) {
            size_t index;
            static const char *const names[] = {"iterator", "asyncIterator", "hasInstance",
                                                "toStringTag", "toPrimitive"};
            for (index = 0; index < sizeof names / sizeof *names; ++index) {
                if (kiln_c99_same_name(key, key_len, names[index])) {
                    *out = kiln_c99_value(KILN_VALUE_SYMBOL,
                                          (uint64_t)(uintptr_t)&kiln_c99_well_known[index], NULL,
                                          0);
                    return KILN_C99_OK;
                }
            }
            return KILN_C99_CONTROL_ERROR;
        }
        if (kiln_c99_same_name(key, key_len, "prototype")) {
            /* Reached for its methods rather than as an object: these are not yet the objects the
             * chains actually end at, which is what `Object.getPrototypeOf` would need. */
            if (target.bits == KILN_C99_BUILTIN_OBJECT) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (target.bits == KILN_C99_BUILTIN_ARRAY) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (target.bits == KILN_C99_BUILTIN_STRING) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_STRING_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (target.bits == KILN_C99_BUILTIN_REGEXP) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (target.bits == KILN_C99_BUILTIN_DATE) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_DATE_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (target.bits == KILN_C99_BUILTIN_NUMBER) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (target.bits == KILN_C99_BUILTIN_FUNCTION) {
                *out = kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
                return KILN_C99_OK;
            }
        }
        if (target.bits == KILN_C99_BUILTIN_MATH) {
            if (kiln_c99_same_name(key, key_len, "PI")) {
                *out = kiln_c99_number(3.14159265358979311600);
            } else if (kiln_c99_same_name(key, key_len, "E")) {
                *out = kiln_c99_number(2.71828182845904509080);
            } else if (kiln_c99_same_name(key, key_len, "LN2")) {
                *out = kiln_c99_number(0.69314718055994530942);
            } else if (kiln_c99_same_name(key, key_len, "LN10")) {
                *out = kiln_c99_number(2.30258509299404568402);
            } else if (kiln_c99_same_name(key, key_len, "SQRT2")) {
                *out = kiln_c99_number(1.41421356237309504880);
            } else if (kiln_c99_same_name(key, key_len, "SQRT1_2")) {
                *out = kiln_c99_number(0.70710678118654752440);
            } else if (kiln_c99_same_name(key, key_len, "LOG2E")) {
                *out = kiln_c99_number(1.44269504088896340736);
            } else if (kiln_c99_same_name(key, key_len, "LOG10E")) {
                *out = kiln_c99_number(0.43429448190325182765);
            } else {
                return KILN_C99_CONTROL_ERROR;
            }
            return KILN_C99_OK;
        }
        if (target.bits == KILN_C99_BUILTIN_NUMBER) {
            if (kiln_c99_same_name(key, key_len, "MAX_SAFE_INTEGER")) {
                *out = kiln_c99_number(9007199254740991.0);
            } else if (kiln_c99_same_name(key, key_len, "MIN_SAFE_INTEGER")) {
                *out = kiln_c99_number(-9007199254740991.0);
            } else if (kiln_c99_same_name(key, key_len, "EPSILON")) {
                *out = kiln_c99_number(2.220446049250313e-16);
            } else if (kiln_c99_same_name(key, key_len, "POSITIVE_INFINITY")) {
                *out = kiln_c99_number(1.0 / 0.0);
            } else if (kiln_c99_same_name(key, key_len, "NEGATIVE_INFINITY")) {
                *out = kiln_c99_number(-1.0 / 0.0);
            } else if (kiln_c99_same_name(key, key_len, "NaN")) {
                *out = kiln_c99_number(0.0 / 0.0);
            } else {
                return KILN_C99_CONTROL_ERROR;
            }
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_NULL:
        /* A TypeError the program can catch, not a refusal: `try { undefined.x } catch (e) { ... }`
         * is ordinary code, and a fatal status would make it unwritable. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    target.tag == KILN_VALUE_NULL
                                        ? "Cannot read properties of null"
                                        : "Cannot read properties of undefined");
    case KILN_VALUE_BIGINT:
        /* `toString` and `valueOf` share the number codes: the dispatch reads the receiver, and
         * the receiver is what tells the two types apart. */
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_BIGINT);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "toString") ||
            kiln_c99_same_name(key, key_len, "toLocaleString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_NUMBER_TO_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "valueOf")) {
            *out = kiln_c99_method(KILN_C99_METHOD_NUMBER_VALUE_OF);
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    case KILN_VALUE_NUMBER:
        if (kiln_c99_same_name(key, key_len, "constructor")) {
            *out = kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "toString")) {
            *out = kiln_c99_method(KILN_C99_METHOD_NUMBER_TO_STRING);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "toFixed")) {
            *out = kiln_c99_method(KILN_C99_METHOD_NUMBER_TO_FIXED);
            return KILN_C99_OK;
        }
        if (kiln_c99_same_name(key, key_len, "valueOf")) {
            *out = kiln_c99_method(KILN_C99_METHOD_NUMBER_VALUE_OF);
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    default:
        /* A boolean or callable has no own properties here. */
        return KILN_C99_CONTROL_ERROR;
    }
}

/* A read, with one rule about the machine's own keys layered over it.
 *
 * A `@@` key is a *protocol probe*: `for await` asks every subject for `@@asyncIterator` before
 * deciding which loop to run, and "this value does not have one" is a real answer. An unknown name
 * a *program* wrote is a different thing -- a method this backend does not have -- and stays the
 * error it was, because answering undefined would turn `a.sort(f)` into "undefined is not a
 * function" three frames from the cause. */
static int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown) {
    int status = kiln_c99_get_own_kind(host, target, key, key_len, out, depth, thrown);
    if (status == KILN_C99_CONTROL_ERROR && key_len >= 2 && key[0] == '@' && key[1] == '@') {
        *out = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    return status;
}

static int kiln_c99_get_computed(kiln_host *host, kiln_value target, kiln_value key,
                                 kiln_value *out, int depth, kiln_value *thrown) {
    double index = kiln_c99_array_index(key);
    if (target.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(target);
        if (index < 0.0) {
            return kiln_c99_get(host, target, key.tag == KILN_VALUE_STRING ? key.ptr : NULL,
                                key.tag == KILN_VALUE_STRING ? key.len : 0, out, depth, thrown);
        }
        *out = (size_t)index < array->count && !kiln_c99_is_hole(array->elements[(size_t)index])
                   ? array->elements[(size_t)index]
                   : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_STRING && index >= 0.0) {
        kiln_c99_utf16_iter iterator = {target.ptr, target.len, 0, 0, 0};
        size_t position = 0;
        uint16_t unit;
        while (kiln_c99_utf16_next(&iterator, &unit)) {
            if (position == (size_t)index) {
                /* One code unit, re-encoded.  A lone surrogate is what indexing a string
                 * actually yields, and it has no scalar value, so it is written as the
                 * three-byte form of its own code point rather than paired with anything. */
                char bytes[3];
                bytes[0] = (char)(0xE0 | (unit >> 12));
                bytes[1] = (char)(0x80 | ((unit >> 6) & 0x3F));
                bytes[2] = (char)(0x80 | (unit & 0x3F));
                if (unit < 0x80) {
                    char one = (char)unit;
                    *out = kiln_c99_copy_text(&one, 1);
                } else {
                    *out = kiln_c99_copy_text(bytes, 3);
                }
                return KILN_C99_OK;
            }
            position += 1;
        }
        *out = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    {
        /* An object key is a string, whatever it was written as -- and a symbol's is the
         * reserved slot it names rather than the way it prints. */
        kiln_value text = kiln_c99_property_key(key);
        if (text.tag != KILN_VALUE_STRING) {
            return KILN_C99_CONTROL_ERROR;
        }
        return kiln_c99_get(host, target, text.ptr, text.len, out, depth, thrown);
    }
}

static int kiln_c99_set_computed(kiln_host *host, kiln_value target, kiln_value key,
                                 kiln_value value, int depth, kiln_value *thrown) {
    double index = kiln_c99_array_index(key);
    if (target.tag == KILN_VALUE_ARRAY && index >= 0.0) {
        kiln_array *array = kiln_c99_array_of(target);
        size_t slot = (size_t)index;
        size_t fill;
        int status;
        if (array->frozen) {
            /* Silently, which is what a write to a frozen object does and therefore what this
             * has to do -- the two are the same rule seen from different sides. */
            return KILN_C99_OK;
        }
        status = kiln_c99_reserve((void **)&array->elements, &array->capacity, slot + 1,
                                  sizeof *array->elements);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* Growing leaves holes, which read as undefined and are not the same thing. */
        for (fill = array->count; fill < slot; ++fill) {
            array->elements[fill] = kiln_c99_hole();
        }
        array->elements[slot] = value;
        if (slot + 1 > array->count) {
            array->count = slot + 1;
        }
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT && target.tag != KILN_VALUE_ARRAY &&
        target.tag != KILN_VALUE_FUNCTION) {
        return KILN_C99_CONTROL_ERROR;
    }
    {
        /* Not an index, so it is an ordinary key -- `a["length"] = 0` and `a["tag"] = "x"` are
         * the same write `a.length` and `a.tag` are, reached by a different spelling. */
        kiln_value text = kiln_c99_property_key(key);
        if (text.tag != KILN_VALUE_STRING) {
            return KILN_C99_CONTROL_ERROR;
        }
        return kiln_c99_put(host, target, text.ptr, text.len, value, depth, thrown);
    }
}

static int kiln_c99_array_push(kiln_value target, kiln_value value) {
    kiln_array *array = kiln_c99_array_of(target);
    int status = kiln_c99_reserve((void **)&array->elements, &array->capacity, array->count + 1,
                                  sizeof *array->elements);
    if (status != KILN_C99_OK) {
        return status;
    }
    array->elements[array->count] = value;
    array->count += 1;
    return KILN_C99_OK;
}

/* What a console shows, which is not what `String(x)` gives: `String({})` is "[object Object]"
 * and a console prints the contents.  The interpreter does this on the host side; the native
 * path has to do it inside the generated unit, because a reference never crosses the embedding
 * boundary -- so this is the one place the two engines have to agree character for character.
 *
 * Node's format, which the interpreter already matches: quoted strings only when nested, keys
 * quoted unless they are identifiers, and a placeholder past two levels of nesting. */
#define KILN_C99_INSPECT_DEPTH 2


/* The quote is whichever the string does not already contain, so the common case needs no
 * escaping at all: "it's" rather than 'it\'s'.  Single, then double, then backtick, which is
 * node's order of preference.  Bytes rather than characters is safe here: every byte this
 * inspects is ASCII, and a UTF-8 continuation byte can never equal one. */
static int kiln_c99_quote(kiln_text *text, const char *bytes, size_t len) {
    char quote = '\'';
    size_t index;
    int has_single = 0;
    int has_double = 0;
    int has_backtick = 0;
    int has_template = 0;
    for (index = 0; index < len; ++index) {
        if (bytes[index] == '\'') has_single = 1;
        if (bytes[index] == '"') has_double = 1;
        if (bytes[index] == '`') has_backtick = 1;
        if (bytes[index] == '$' && index + 1 < len && bytes[index + 1] == '{') has_template = 1;
    }
    if (has_single) {
        if (!has_double) {
            quote = '"';
        } else if (!has_backtick && !has_template) {
            quote = '`';
        }
    }
    if (kiln_c99_text_append(text, &quote, 1) != KILN_C99_OK) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = 0; index < len; ++index) {
        char byte = bytes[index];
        const char *escape = NULL;
        char pair[2];
        char hex[4];
        static const char digits[] = "0123456789ABCDEF";
        if (byte == quote || byte == '\\') {
            pair[0] = '\\';
            pair[1] = byte;
            if (kiln_c99_text_append(text, pair, 2) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        switch (byte) {
        case '\n': escape = "\\n"; break;
        case '\r': escape = "\\r"; break;
        case '\t': escape = "\\t"; break;
        case '\b': escape = "\\b"; break;
        case '\f': escape = "\\f"; break;
        default: break;
        }
        if (escape != NULL) {
            if (kiln_c99_text_append_c(text, escape) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        if ((unsigned char)byte < 0x20) {
            hex[0] = '\\';
            hex[1] = 'x';
            hex[2] = digits[((unsigned char)byte >> 4) & 0xF];
            hex[3] = digits[(unsigned char)byte & 0xF];
            if (kiln_c99_text_append(text, hex, 4) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        if (kiln_c99_text_append(text, &byte, 1) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    return kiln_c99_text_append(text, &quote, 1);
}

static int kiln_c99_inspect_key(kiln_text *text, const char *key, size_t len) {
    size_t index;
    int identifier = len != 0 && (isalpha((unsigned char)key[0]) || key[0] == '_');
    for (index = 1; identifier && index < len; ++index) {
        identifier = isalnum((unsigned char)key[index]) || key[index] == '_';
    }
    if (identifier) {
        return kiln_c99_text_append(text, key, len);
    }
    return kiln_c99_quote(text, key, len);
}

static int kiln_c99_inspect_into(kiln_text *text, kiln_value value, int depth);

static int kiln_c99_inspect_members(kiln_text *text, kiln_value value, int depth) {
    size_t index;
    if (value.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(value);
        kiln_object *extra = array->properties.tag == KILN_VALUE_OBJECT
                                 ? kiln_c99_object_of(array->properties)
                                 : NULL;
        if (array->count == 0 && (extra == NULL || extra->count == 0)) {
            return kiln_c99_text_append_c(text, "[]");
        }
        if (kiln_c99_text_append_c(text, "[ ") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < array->count; ++index) {
            if (index != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_is_hole(array->elements[index])) {
                /* A run of holes is reported as a count rather than repeated, which is what
                 * makes a sparse array readable at all. */
                size_t run = 1;
                while (index + run < array->count &&
                       kiln_c99_is_hole(array->elements[index + run])) {
                    run += 1;
                }
                if (kiln_c99_text_append_c(text, "<") != KILN_C99_OK ||
                    kiln_c99_pad_number(text, (int64_t)run, 1) != KILN_C99_OK ||
                    kiln_c99_text_append_c(text, run == 1 ? " empty item>" : " empty items>") !=
                        KILN_C99_OK) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
                index += run - 1;
                continue;
            }
            if (kiln_c99_inspect_into(text, array->elements[index], depth + 1) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        /* A property that is not an element is shown after the elements, which is where a console
         * puts the `index` and `input` a match result carries. */
        for (index = 0; extra != NULL && index < extra->count; ++index) {
            if (!extra->properties[index].enumerable) {
                continue;
            }
            if ((array->count != 0 || index != 0) &&
                kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_inspect_key(text, extra->properties[index].key,
                                     extra->properties[index].key_len) != KILN_C99_OK ||
                kiln_c99_text_append_c(text, ": ") != KILN_C99_OK ||
                kiln_c99_inspect_into(text, extra->properties[index].value, depth + 1) !=
                    KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        return kiln_c99_text_append_c(text, " ]");
    }
    {
        kiln_object *object = kiln_c99_object_of(value);
        size_t enumerated = 0;
        /* A class instance shows the name of the class that made it, which is how a console
         * distinguishes two objects of the same shape. A plain object's constructor is `Object`,
         * and naming that would be noise rather than information. */
        if (object->prototype.tag == KILN_VALUE_OBJECT) {
            kiln_object *table = kiln_c99_object_of(object->prototype);
            size_t slot;
            for (slot = 0; slot < table->count; ++slot) {
                kiln_value maker = table->properties[slot].value;
                if (!kiln_c99_same_key(&table->properties[slot], "constructor", 11) ||
                    maker.tag != KILN_VALUE_FUNCTION) {
                    continue;
                }
                {
                    kiln_closure *closure = kiln_c99_closure_of(maker);
                    if (closure->name == NULL || closure->name_len == 0 ||
                        kiln_c99_same_name(closure->name, closure->name_len, "Object")) {
                        break;
                    }
                    if (kiln_c99_text_append(text, closure->name, closure->name_len) !=
                            KILN_C99_OK ||
                        kiln_c99_text_append_c(text, " ") != KILN_C99_OK) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                break;
            }
        }
        for (index = 0; index < object->count; ++index) {
            enumerated += (size_t)(object->properties[index].enumerable != 0);
        }
        if (enumerated == 0) {
            return kiln_c99_text_append_c(text, "{}");
        }
        if (kiln_c99_text_append_c(text, "{ ") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < object->count; ++index) {
            if (!object->properties[index].enumerable) {
                continue;
            }
            if (index != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_inspect_key(text, object->properties[index].key,
                                     object->properties[index].key_len) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_text_append_c(text, ": ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (object->properties[index].accessor) {
                /* Named rather than called: showing a value here would run the program's code
                 * because something was printed. */
                const char *shown =
                    object->properties[index].getter.tag == KILN_VALUE_UNDEFINED  ? "[Setter]"
                    : object->properties[index].setter.tag == KILN_VALUE_UNDEFINED ? "[Getter]"
                                                                                   : "[Getter/Setter]";
                if (kiln_c99_text_append_c(text, shown) != KILN_C99_OK) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
                continue;
            }
            if (kiln_c99_inspect_into(text, object->properties[index].value, depth + 1) !=
                KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        return kiln_c99_text_append_c(text, " }");
    }
}

static int kiln_c99_inspect_into(kiln_text *text, kiln_value value, int depth) {
    switch (value.tag) {
    case KILN_VALUE_STRING:
        return kiln_c99_quote(text, value.ptr, value.len);
    case KILN_VALUE_NUMBER: {
        /* `-0` prints as itself here, where `String(-0)` is "0": a console shows the value
         * rather than converting it. */
        double number = kiln_c99_as_number(value);
        if (number == 0.0 && kiln_c99_signbit(number)) {
            return kiln_c99_text_append_c(text, "-0");
        }
        break;
    }
    case KILN_VALUE_BIGINT: {
        /* The `n` belongs to the console, not to the value: `String(1n)` is "1" and
         * `console.log(1n)` is `1n`, and the suffix is the only difference between them. */
        kiln_value digits = kiln_c99_bigint_text(value, 10);
        if (digits.tag != KILN_VALUE_STRING ||
            kiln_c99_text_append(text, digits.ptr, digits.len) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "n");
    }
    case KILN_VALUE_FUNCTION: {
        kiln_closure *closure = kiln_c99_closure_of(value);
        if (closure->name == NULL) {
            return kiln_c99_text_append_c(text, "[Function (anonymous)]");
        }
        if (kiln_c99_text_append_c(text, "[Function: ") != KILN_C99_OK ||
            kiln_c99_text_append(text, closure->name, closure->name_len) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "]");
    }
    case KILN_VALUE_DATE: {
        /* A console shows the ISO form, which is what the interpreter shows. */
        kiln_value iso;
        if (kiln_c99_iso_string(&iso, kiln_c99_date_time(value)) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (iso.tag != KILN_VALUE_STRING) {
            return kiln_c99_text_append_c(text, "Invalid Date");
        }
        return kiln_c99_text_append(text, iso.ptr, iso.len);
    }
    case KILN_VALUE_COLLECTION: {
        /* A console shows what a collection holds rather than the bookkeeping that makes it
         * work, with the count in front because a Map's contents do not otherwise say how many
         * there are. */
        kiln_collection *collection = kiln_c99_collection_of(value);
        size_t index;
        if (kiln_c99_text_append_c(text, collection->is_map ? "Map(" : "Set(") != KILN_C99_OK ||
            kiln_c99_pad_number(text, (int64_t)collection->live, 1) != KILN_C99_OK ||
            kiln_c99_text_append_c(text, collection->live == 0 ? ") {}" : ") { ") !=
                KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (collection->live == 0) {
            return KILN_C99_OK;
        }
        {
        size_t shown = 0;
        for (index = 0; index < collection->count; ++index) {
            if (collection->entries[index].dead) {
                continue;
            }
            if (shown++ != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (collection->is_map) {
                if (kiln_c99_inspect_into(text, collection->entries[index].key, depth + 1) !=
                        KILN_C99_OK ||
                    kiln_c99_text_append_c(text, " => ") != KILN_C99_OK) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            if (kiln_c99_inspect_into(text, collection->entries[index].value, depth + 1) !=
                KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        }
        return kiln_c99_text_append_c(text, " }");
    }
    case KILN_VALUE_PROMISE: {
        kiln_promise *promise = kiln_c99_promise_of(value);
        if (promise->state == KILN_C99_PENDING) {
            return kiln_c99_text_append_c(text, "Promise { <pending> }");
        }
        if (kiln_c99_text_append_c(text, promise->state == KILN_C99_REJECTED
                                             ? "Promise { <rejected> "
                                             : "Promise { ") != KILN_C99_OK ||
            kiln_c99_inspect_into(text, promise->value, depth + 1) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, " }");
    }
    case KILN_VALUE_GENERATOR:
        return kiln_c99_text_append_c(text, "Object [Generator] {}");
    case KILN_VALUE_HOLE:
        return kiln_c99_text_append_c(text, "undefined");
    case KILN_VALUE_BUILTIN: {
        const char *name = kiln_c99_builtin_name(value.bits);
        if (kiln_c99_is_namespace(value.bits)) {
            /* `Object [Math] {}`: a namespace shows as the object it is, with no members --
             * node lists none of the built-in ones either. */
            if (kiln_c99_text_append_c(text, "Object [") != KILN_C99_OK ||
                kiln_c99_text_append_c(text, name) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            return kiln_c99_text_append_c(text, "] {}");
        }
        if (kiln_c99_text_append_c(text, "[Function: ") != KILN_C99_OK ||
            kiln_c99_text_append_c(text, name) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "]");
    }
    case KILN_VALUE_METHOD: {
        const char *name = kiln_c99_method_name(value.bits);
        if (name == NULL) {
            return kiln_c99_text_append_c(text, "[Function (anonymous)]");
        }
        if (kiln_c99_text_append_c(text, "[Function: ") != KILN_C99_OK ||
            kiln_c99_text_append_c(text, name) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        return kiln_c99_text_append_c(text, "]");
    }
    case KILN_VALUE_OBJECT:
        if (depth > KILN_C99_INSPECT_DEPTH) {
            return kiln_c99_text_append_c(text, "[Object]");
        }
        return kiln_c99_inspect_members(text, value, depth);
    case KILN_VALUE_ARRAY:
        if (depth > KILN_C99_INSPECT_DEPTH) {
            return kiln_c99_text_append_c(text, "[Array]");
        }
        return kiln_c99_inspect_members(text, value, depth);
    default:
        break;
    }
    {
        kiln_value shown = kiln_c99_text(value);
        if (shown.tag != KILN_VALUE_STRING) {
            return KILN_C99_CONTROL_ERROR;
        }
        return kiln_c99_text_append(text, shown.ptr, shown.len);
    }
}

/* A top-level string prints as itself; only a nested one is quoted. */
static kiln_value kiln_c99_inspect(kiln_value value) {
    kiln_text text = {NULL, 0, 0};
    if (value.tag == KILN_VALUE_STRING) {
        return value;
    }
    if (kiln_c99_inspect_into(&text, value, 0) != KILN_C99_OK) {
        free(text.bytes);
        return kiln_c99_undefined();
    }
    return kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
}

/* Every position-taking string method counts UTF-16 code units (ADR 0022), so each of them
 * converts once, works on units, and re-encodes.  One conversion pair rather than a dozen
 * ad-hoc walks is what keeps them agreeing with each other and with the interpreter.
 *
 * The caller owns the returned array and frees it. */
static int kiln_c99_to_units(kiln_value value, uint16_t **out, size_t *count) {
    kiln_c99_utf16_iter iterator = {value.ptr, value.len, 0, 0, 0};
    uint16_t *units = NULL;
    size_t capacity = 0;
    size_t length = 0;
    uint16_t unit;
    while (kiln_c99_utf16_next(&iterator, &unit)) {
        if (kiln_c99_reserve((void **)&units, &capacity, length + 1, sizeof *units) !=
            KILN_C99_OK) {
            free(units);
            return KILN_C99_OUT_OF_MEMORY;
        }
        units[length] = unit;
        length += 1;
    }
    *out = units;
    *count = length;
    return KILN_C99_OK;
}

/* Re-encode, pairing surrogates back up.  An *unpaired* surrogate has no scalar value, so it
 * becomes U+FFFD -- a kiln string cannot hold one, which is the divergence ADR 0022 records and
 * the interpreter's `String::from_utf16_lossy` makes the same substitution. */
static kiln_value kiln_c99_from_units(const uint16_t *units, size_t count) {
    kiln_text text = {NULL, 0, 0};
    size_t index;
    for (index = 0; index < count; ++index) {
        uint32_t point = units[index];
        char bytes[4];
        size_t width;
        if (point >= 0xD800 && point <= 0xDBFF && index + 1 < count && units[index + 1] >= 0xDC00 &&
            units[index + 1] <= 0xDFFF) {
            point = 0x10000 + ((point - 0xD800) << 10) + (units[index + 1] - 0xDC00);
            index += 1;
        } else if (point >= 0xD800 && point <= 0xDFFF) {
            point = 0xFFFD;
        }
        if (point < 0x80) {
            bytes[0] = (char)point;
            width = 1;
        } else if (point < 0x800) {
            bytes[0] = (char)(0xC0 | (point >> 6));
            bytes[1] = (char)(0x80 | (point & 0x3F));
            width = 2;
        } else if (point < 0x10000) {
            bytes[0] = (char)(0xE0 | (point >> 12));
            bytes[1] = (char)(0x80 | ((point >> 6) & 0x3F));
            bytes[2] = (char)(0x80 | (point & 0x3F));
            width = 3;
        } else {
            bytes[0] = (char)(0xF0 | (point >> 18));
            bytes[1] = (char)(0x80 | ((point >> 12) & 0x3F));
            bytes[2] = (char)(0x80 | ((point >> 6) & 0x3F));
            bytes[3] = (char)(0x80 | (point & 0x3F));
            width = 4;
        }
        if (kiln_c99_text_append(&text, bytes, width) != KILN_C99_OK) {
            free(text.bytes);
            return kiln_c99_undefined();
        }
    }
    return kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
}

/* The first position at or after `from` where `needle` occurs, or -1. */
static double kiln_c99_units_find(const uint16_t *haystack, size_t haystack_len,
                                  const uint16_t *needle, size_t needle_len, size_t from) {
    size_t at;
    if (needle_len > haystack_len) {
        return -1.0;
    }
    for (at = from; at + needle_len <= haystack_len; ++at) {
        if (memcmp(haystack + at, needle, needle_len * sizeof *needle) == 0) {
            return (double)at;
        }
    }
    return -1.0;
}

/* A code unit that `trim` removes: the specification's WhiteSpace plus LineTerminator. */
static int kiln_c99_is_trimmed(uint16_t unit) {
    switch (unit) {
    case 0x09: case 0x0A: case 0x0B: case 0x0C: case 0x0D: case 0x20:
    case 0xA0: case 0x1680: case 0x2000: case 0x2001: case 0x2002: case 0x2003:
    case 0x2004: case 0x2005: case 0x2006: case 0x2007: case 0x2008: case 0x2009:
    case 0x200A: case 0x2028: case 0x2029: case 0x202F: case 0x205F: case 0x3000:
    case 0xFEFF:
        return 1;
    default:
        return 0;
    }
}

/* `includes` uses SameValueZero, which differs from `===` in exactly one place: NaN matches
 * itself.  `indexOf` uses `===` and so never finds one. */
static int kiln_c99_same_value_zero(kiln_value left, kiln_value right) {
    if (left.tag == KILN_VALUE_NUMBER && right.tag == KILN_VALUE_NUMBER) {
        double a = kiln_c99_as_number(left);
        double b = kiln_c99_as_number(right);
        if (isnan(a) && isnan(b)) {
            return 1;
        }
    }
    return kiln_c99_strict_equal(left, right);
}

/* A relative index: negative counts from the end, and the result is clamped into range.  This is
 * `slice`'s rule and it is shared by every method that takes one. */
static size_t kiln_c99_relative_index(kiln_value value, size_t length, size_t fallback) {
    double index;
    if (value.tag == KILN_VALUE_UNDEFINED) {
        return fallback;
    }
    index = kiln_c99_as_number(value);
    if (isnan(index)) {
        return 0;
    }
    if (index < 0.0) {
        index += (double)length;
        return index < 0.0 ? 0 : (size_t)index;
    }
    return index > (double)length ? length : (size_t)index;
}

/* A position argument in code units: negative counts from the end for `slice` and `at`, and
 * `substring` clamps instead -- which is `relative` false. */
static size_t kiln_c99_unit_position(kiln_value value, size_t length, size_t fallback,
                                     int relative) {
    double index;
    if (value.tag == KILN_VALUE_UNDEFINED) {
        return fallback;
    }
    index = kiln_c99_as_number(value);
    if (isnan(index) || index < 0.0) {
        if (!relative || isnan(index)) {
            return 0;
        }
        index += (double)length;
        return index < 0.0 ? 0 : (size_t)index;
    }
    return index > (double)length ? length : (size_t)index;
}

/* The string methods.  Split out from the array ones because they share nothing but the dispatch
 * -- every one of these works on code units and every one of those works on elements. */
static int kiln_c99_invoke_string_method(kiln_value *result, uint64_t code, kiln_value receiver,
                                         const kiln_value *args, size_t argc) {
    uint16_t *units = NULL;
    uint16_t *needle = NULL;
    size_t count = 0;
    size_t needle_count = 0;
    int status = kiln_c99_to_units(receiver, &units, &count);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* Most of these take a string argument, and converting it once here keeps each case short.
     * A case that does not want one simply ignores it. */
    if (argc > 0) {
        kiln_value shown = kiln_c99_text(args[0]);
        if (shown.tag == KILN_VALUE_STRING) {
            status = kiln_c99_to_units(shown, &needle, &needle_count);
            if (status != KILN_C99_OK) {
                free(units);
                return status;
            }
        }
    }
    status = KILN_C99_OK;
    switch (code) {
    case KILN_C99_METHOD_STRING_SLICE:
    case KILN_C99_METHOD_SUBSTRING: {
        int relative = code == KILN_C99_METHOD_STRING_SLICE;
        size_t start = kiln_c99_unit_position(argc > 0 ? args[0] : kiln_c99_undefined(), count, 0,
                                              relative);
        size_t end = kiln_c99_unit_position(argc > 1 ? args[1] : kiln_c99_undefined(), count,
                                            count, relative);
        /* `substring` swaps its ends when they are the wrong way round; `slice` yields nothing.
         * The two look alike and agree on almost no edge case. */
        if (!relative && start > end) {
            size_t swap = start;
            start = end;
            end = swap;
        }
        *result = start >= end ? kiln_c99_from_units(NULL, 0)
                               : kiln_c99_from_units(units + start, end - start);
        break;
    }
    case KILN_C99_METHOD_STRING_INDEX_OF:
        *result = kiln_c99_number(kiln_c99_units_find(units, count, needle, needle_count, 0));
        break;
    case KILN_C99_METHOD_LAST_INDEX_OF: {
        double found = -1.0;
        double at;
        size_t from = 0;
        while ((at = kiln_c99_units_find(units, count, needle, needle_count, from)) >= 0.0) {
            found = at;
            from = (size_t)at + 1;
        }
        *result = kiln_c99_number(found);
        break;
    }
    case KILN_C99_METHOD_STRING_INCLUDES:
        *result = kiln_c99_boolean(
            kiln_c99_units_find(units, count, needle, needle_count, 0) >= 0.0);
        break;
    case KILN_C99_METHOD_STARTS_WITH:
        *result = kiln_c99_boolean(needle_count <= count &&
                                   memcmp(units, needle, needle_count * sizeof *units) == 0);
        break;
    case KILN_C99_METHOD_ENDS_WITH:
        *result = kiln_c99_boolean(needle_count <= count &&
                                   memcmp(units + count - needle_count, needle,
                                          needle_count * sizeof *units) == 0);
        break;
    case KILN_C99_METHOD_CHAR_AT:
    case KILN_C99_METHOD_CHAR_CODE_AT:
    case KILN_C99_METHOD_STRING_AT: {
        double index = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        if (isnan(index)) {
            index = 0.0;
        }
        /* `at` accepts a negative index, where `charAt` clamps and `charCodeAt` answers NaN --
         * three spellings of "the character there" that differ only at the edges. */
        if (code == KILN_C99_METHOD_STRING_AT && index < 0.0) {
            index += (double)count;
        }
        if (index < 0.0 || index >= (double)count) {
            *result = code == KILN_C99_METHOD_CHAR_AT      ? kiln_c99_from_units(NULL, 0)
                      : code == KILN_C99_METHOD_STRING_AT  ? kiln_c99_undefined()
                                                           : kiln_c99_number(0.0 / 0.0);
        } else if (code == KILN_C99_METHOD_CHAR_CODE_AT) {
            *result = kiln_c99_number((double)units[(size_t)index]);
        } else {
            *result = kiln_c99_from_units(units + (size_t)index, 1);
        }
        break;
    }
    case KILN_C99_METHOD_STRING_TO_STRING:
    case KILN_C99_METHOD_STRING_NORMALIZE:
    case KILN_C99_METHOD_STRING_VALUE_OF:
        /* `normalize` has no normalisation tables behind it on either side, so it is the
         * identity: honest for text that is already normalised, which is what a bundler emits,
         * and wrong for text that is not -- which is why it is said here rather than left to be
         * discovered. */
        *result = receiver;
        break;
    case KILN_C99_METHOD_STRING_LOCALE_COMPARE: {
        /* No collation tables either, so this compares code units -- which is a valid locale
         * ordering and is the one both engines use. */
        kiln_value other = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_from_units(NULL, 0);
        if (other.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        *result = kiln_c99_number((double)kiln_c99_string_compare(receiver, other));
        break;
    }
    case KILN_C99_METHOD_STRING_CODE_POINT_AT: {
        double position = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        if (position < 0.0 || position >= (double)count) {
            *result = kiln_c99_undefined();
            break;
        }
        {
            /* A surrogate pair reads as the whole code point, which is the entire difference
             * from `charCodeAt`. */
            uint16_t lead = units[(size_t)position];
            if (lead >= 0xD800 && lead <= 0xDBFF && (size_t)position + 1 < count &&
                units[(size_t)position + 1] >= 0xDC00 && units[(size_t)position + 1] <= 0xDFFF) {
                *result = kiln_c99_number(
                    (double)(0x10000u + (((uint32_t)lead - 0xD800u) << 10) +
                             ((uint32_t)units[(size_t)position + 1] - 0xDC00u)));
                break;
            }
            *result = kiln_c99_number((double)lead);
        }
        break;
    }
    case KILN_C99_METHOD_STRING_SUBSTR: {
        double relative = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        size_t start;
        size_t taken;
        /* A negative start counts from the end, which is the one thing `substr` does that
         * `substring` does not. */
        if (relative < 0.0) {
            double from = (double)count + relative;
            start = from < 0.0 ? 0 : (size_t)from;
        } else {
            start = relative > (double)count ? count : (size_t)relative;
        }
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            double wanted = kiln_c99_as_number(args[1]);
            if (isnan(wanted) || wanted < 0.0) {
                taken = 0;
            } else {
                taken = wanted > (double)(count - start) ? count - start : (size_t)wanted;
            }
        } else {
            taken = count - start;
        }
        *result = kiln_c99_from_units(units + start, taken);
        break;
    }
    case KILN_C99_METHOD_TRIM:
    case KILN_C99_METHOD_TRIM_START:
    case KILN_C99_METHOD_TRIM_END: {
        size_t start = 0;
        size_t end = count;
        if (code != KILN_C99_METHOD_TRIM_END) {
            while (start < end && kiln_c99_is_trimmed(units[start])) {
                start += 1;
            }
        }
        if (code != KILN_C99_METHOD_TRIM_START) {
            while (end > start && kiln_c99_is_trimmed(units[end - 1])) {
                end -= 1;
            }
        }
        *result = kiln_c99_from_units(units + start, end - start);
        break;
    }
    case KILN_C99_METHOD_SPLIT: {
        kiln_value built;
        status = kiln_c99_new_array(&built);
        if (status == KILN_C99_OK) {
            size_t from = 0;
            if (argc == 0 || args[0].tag == KILN_VALUE_UNDEFINED) {
                /* No separator at all yields the whole string as one part. */
                status = kiln_c99_array_push(built, kiln_c99_from_units(units, count));
            } else if (needle_count == 0) {
                /* An empty separator splits into code units, not into nothing. */
                size_t index;
                for (index = 0; index < count && status == KILN_C99_OK; ++index) {
                    status = kiln_c99_array_push(built, kiln_c99_from_units(units + index, 1));
                }
            } else {
                double at;
                while (status == KILN_C99_OK &&
                       (at = kiln_c99_units_find(units, count, needle, needle_count, from)) >=
                           0.0) {
                    status = kiln_c99_array_push(
                        built, kiln_c99_from_units(units + from, (size_t)at - from));
                    from = (size_t)at + needle_count;
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(built,
                                                 kiln_c99_from_units(units + from, count - from));
                }
            }
            *result = built;
        }
        break;
    }
    case KILN_C99_METHOD_REPEAT: {
        double times = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        kiln_text repeated = {NULL, 0, 0};
        if (!(times >= 0.0) || !isfinite(times)) {
            status = KILN_C99_CONTROL_ERROR;
        } else {
            size_t remaining = (size_t)times;
            while (remaining-- > 0 && status == KILN_C99_OK) {
                status = kiln_c99_text_append(&repeated, receiver.ptr, receiver.len);
            }
            *result = kiln_c99_value(KILN_VALUE_STRING, 0, repeated.bytes, repeated.len);
        }
        break;
    }
    case KILN_C99_METHOD_PAD_START:
    case KILN_C99_METHOD_PAD_END: {
        double target = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        static const uint16_t space[] = {0x20};
        const uint16_t *filler = space;
        size_t filler_count = 1;
        if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
            kiln_value shown = kiln_c99_text(args[1]);
            free(needle);
            needle = NULL;
            needle_count = 0;
            if (shown.tag != KILN_VALUE_STRING ||
                kiln_c99_to_units(shown, &needle, &needle_count) != KILN_C99_OK) {
                status = KILN_C99_CONTROL_ERROR;
                break;
            }
            filler = needle;
            filler_count = needle_count;
        }
        if (!(target > (double)count) || filler_count == 0) {
            *result = kiln_c99_from_units(units, count);
        } else {
            size_t wanted = (size_t)target;
            uint16_t *padded = (uint16_t *)malloc(wanted * sizeof *padded);
            size_t fill = wanted - count;
            size_t index;
            if (padded == NULL) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            for (index = 0; index < fill; ++index) {
                padded[code == KILN_C99_METHOD_PAD_START ? index : count + index] =
                    filler[index % filler_count];
            }
            for (index = 0; index < count; ++index) {
                padded[code == KILN_C99_METHOD_PAD_START ? fill + index : index] = units[index];
            }
            *result = kiln_c99_from_units(padded, wanted);
            free(padded);
        }
        break;
    }
    case KILN_C99_METHOD_TO_UPPER_CASE:
    case KILN_C99_METHOD_TO_LOWER_CASE: {
        size_t index;
        uint16_t *cased = (uint16_t *)malloc((count == 0 ? 1 : count) * sizeof *cased);
        if (cased == NULL) {
            status = KILN_C99_OUT_OF_MEMORY;
            break;
        }
        for (index = 0; index < count; ++index) {
            uint16_t unit = units[index];
            /* Case mapping outside ASCII needs Unicode tables this unit does not carry, and
             * guessing would disagree with the interpreter silently. */
            if (unit >= 0x80) {
                free(cased);
                free(units);
                free(needle);
                return KILN_C99_CONTROL_ERROR;
            }
            cased[index] = code == KILN_C99_METHOD_TO_UPPER_CASE
                               ? (uint16_t)toupper((int)unit)
                               : (uint16_t)tolower((int)unit);
        }
        *result = kiln_c99_from_units(cased, count);
        free(cased);
        break;
    }
    case KILN_C99_METHOD_REPLACE:
    case KILN_C99_METHOD_REPLACE_ALL: {
        kiln_value with = argc > 1 ? kiln_c99_text(args[1]) : kiln_c99_undefined();
        uint16_t *replacement = NULL;
        size_t replacement_count = 0;
        kiln_text built = {NULL, 0, 0};
        size_t from = 0;
        double at;
        if (with.tag == KILN_VALUE_STRING &&
            kiln_c99_to_units(with, &replacement, &replacement_count) != KILN_C99_OK) {
            status = KILN_C99_OUT_OF_MEMORY;
            break;
        }
        while ((at = kiln_c99_units_find(units, count, needle, needle_count, from)) >= 0.0) {
            kiln_value head = kiln_c99_from_units(units + from, (size_t)at - from);
            /* The occurrence is the only group, which is what lets the template expander below be
             * the same one the regexp path uses -- and a replacement *is* a template even when
             * the pattern was a plain string: `"a".replace("a", "$&$&")` is `"aa"`. */
            int64_t ranges[2];
            ranges[0] = (int64_t)at;
            ranges[1] = (int64_t)at + (int64_t)needle_count;
            if (kiln_c99_text_append(&built, head.ptr, head.len) != KILN_C99_OK ||
                kiln_c99_expand_replacement(&built, replacement, replacement_count, units, count,
                                            ranges, 1) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            from = (size_t)at + needle_count;
            /* `replace` takes the first occurrence; `replaceAll` takes every one. An empty needle
             * matches between every pair of characters, so the cursor is stepped by hand -- and
             * the character it steps over has to be carried across, or it would be dropped. */
            if (code == KILN_C99_METHOD_REPLACE) {
                break;
            }
            if (needle_count == 0) {
                kiln_value carried;
                if (from >= count) {
                    break;
                }
                carried = kiln_c99_from_units(units + from, 1);
                if (kiln_c99_text_append(&built, carried.ptr, carried.len) != KILN_C99_OK) {
                    status = KILN_C99_OUT_OF_MEMORY;
                    break;
                }
                from += 1;
            }
        }
        if (status == KILN_C99_OK) {
            kiln_value rest = kiln_c99_from_units(units + from, count - from);
            if (kiln_c99_text_append(&built, rest.ptr, rest.len) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else {
                *result = kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
            }
        }
        free(replacement);
        break;
    }
    case KILN_C99_METHOD_STRING_CONCAT: {
        kiln_text built = {NULL, 0, 0};
        size_t index;
        if (kiln_c99_text_append(&built, receiver.ptr, receiver.len) != KILN_C99_OK) {
            status = KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < argc && status == KILN_C99_OK; ++index) {
            kiln_value shown = kiln_c99_text(args[index]);
            if (shown.tag != KILN_VALUE_STRING ||
                kiln_c99_text_append(&built, shown.ptr, shown.len) != KILN_C99_OK) {
                status = KILN_C99_CONTROL_ERROR;
            }
        }
        if (status == KILN_C99_OK) {
            *result = kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
        }
        break;
    }
    default:
        status = KILN_C99_CONTROL_ERROR;
        break;
    }
    free(units);
    free(needle);
    return status;
}

/* One `exec`-shaped array: the match, then each group, with `index`, `input` and `groups`. */
static int kiln_c99_match_result(kiln_host *host, kiln_value *out, const uint16_t *units,
                                 size_t count, const int64_t *ranges, size_t groups, int depth,
                                 kiln_value *thrown) {
    kiln_value built;
    size_t group;
    int status = kiln_c99_new_array(&built);
    for (group = 0; group < groups && status == KILN_C99_OK; ++group) {
        int64_t from = ranges[group * 2];
        int64_t to = ranges[group * 2 + 1];
        status = kiln_c99_array_push(built, from < 0 ? kiln_c99_undefined()
                                                     : kiln_c99_from_units(units + (size_t)from,
                                                                           (size_t)(to - from)));
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, built, "index", 5, kiln_c99_number((double)ranges[0]), depth,
                              thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, built, "input", 5, kiln_c99_from_units(units, count), depth,
                              thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, built, "groups", 6, kiln_c99_undefined(), depth, thrown);
    }
    *out = built;
    return status;
}

/* The string methods that take a pattern.
 *
 * Intercepted before the string block because they need the interpreter: a replacement can be a
 * *function*, and the string block is a pure transformation with nothing to call. `replace` with
 * a plain string and a plain needle stays down there, where it always was.
 *
 * An empty match advances the cursor by one, or a pattern that can match nothing would never
 * finish. */
static int kiln_c99_string_pattern(kiln_host *host, kiln_value *result, uint64_t code,
                                   kiln_value receiver, const kiln_value *args, size_t argc,
                                   int depth, kiln_value *thrown) {
    kiln_value pattern = argc > 0 ? args[0] : kiln_c99_undefined();
    kiln_value with = argc > 1 ? args[1] : kiln_c99_undefined();
    kiln_regexp *regexp = pattern.tag == KILN_VALUE_REGEXP ? kiln_c99_regexp_of(pattern) : NULL;
    uint16_t *units = NULL;
    uint16_t *replacement = NULL;
    size_t count = 0;
    size_t replacement_count = 0;
    size_t from = 0;
    size_t at = 0;
    int64_t ranges[64];
    kiln_text built = {NULL, 0, 0};
    kiln_value collected = kiln_c99_undefined();
    int status = kiln_c99_to_units(receiver, &units, &count);
    int every;
    if (status != KILN_C99_OK) {
        return status;
    }
    if (regexp == NULL) {
        /* `"x".match("a")` compiles its argument, which is what makes a string pattern work
         * everywhere a regexp does. */
        kiln_value source = kiln_c99_text(pattern);
        static const char none[] = "";
        free(units);
        if (source.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        {
            kiln_value made;
            kiln_value forwarded[2];
            status = kiln_c99_new_regexp(&made, source.ptr, source.len, none, 0, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            forwarded[0] = made;
            forwarded[1] = with;
            return kiln_c99_string_pattern(host, result, code, receiver, forwarded,
                                           argc > 1 ? 2 : 1, depth, thrown);
        }
    }
    every = regexp->global || code == KILN_C99_METHOD_REPLACE_ALL ||
            code == KILN_C99_METHOD_STRING_MATCH_ALL || code == KILN_C99_METHOD_SPLIT;
    if (code == KILN_C99_METHOD_STRING_MATCH || code == KILN_C99_METHOD_STRING_MATCH_ALL ||
        code == KILN_C99_METHOD_SPLIT) {
        status = kiln_c99_new_array(&collected);
        if (status != KILN_C99_OK) {
            free(units);
            return status;
        }
    }
    if (code == KILN_C99_METHOD_SPLIT && count == 0) {
        /* An empty subject is the one case with no trailing piece: a separator that matches it
         * splits it into nothing, and one that does not leaves it whole. */
        size_t groups = 0;
        int matched = 0;
        status = kiln_c99_regexp_find(regexp, units, count, 0, ranges,
                                      sizeof ranges / sizeof *ranges, &groups, &matched);
        if (status == KILN_C99_OK && !matched) {
            status = kiln_c99_array_push(collected, kiln_c99_from_units(units, 0));
        }
        free(units);
        *result = collected;
        return status;
    }
    if ((code == KILN_C99_METHOD_REPLACE || code == KILN_C99_METHOD_REPLACE_ALL) &&
        !kiln_c99_is_callable(with)) {
        kiln_value text = kiln_c99_text(with);
        if (text.tag != KILN_VALUE_STRING ||
            kiln_c99_to_units(text, &replacement, &replacement_count) != KILN_C99_OK) {
            free(units);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    for (;;) {
        size_t groups = 0;
        int matched = 0;
        status = kiln_c99_regexp_find(regexp, units, count, at, ranges,
                                      sizeof ranges / sizeof *ranges, &groups, &matched);
        if (status != KILN_C99_OK || !matched) {
            break;
        }
        if (code == KILN_C99_METHOD_STRING_SEARCH) {
            *result = kiln_c99_number((double)ranges[0]);
            free(units);
            free(replacement);
            return KILN_C99_OK;
        }
        if (code == KILN_C99_METHOD_STRING_MATCH && !regexp->global) {
            status = kiln_c99_match_result(host, result, units, count, ranges, groups, depth,
                                           thrown);
            free(units);
            free(replacement);
            return status;
        }
        if (code == KILN_C99_METHOD_STRING_MATCH) {
            /* A global `match` collects the matched text and nothing else, which is the whole
             * difference from `matchAll`. */
            status = kiln_c99_array_push(collected,
                                         kiln_c99_from_units(units + (size_t)ranges[0],
                                                             (size_t)(ranges[1] - ranges[0])));
        } else if (code == KILN_C99_METHOD_STRING_MATCH_ALL) {
            kiln_value entry;
            status = kiln_c99_match_result(host, &entry, units, count, ranges, groups, depth,
                                           thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_array_push(collected, entry);
            }
        } else if (code == KILN_C99_METHOD_SPLIT) {
            size_t group;
            /* An empty match at the position the last piece started is not a separator, and
             * neither is a match *at* the end: the walk stops before the last position and the
             * piece after the final separator is pushed below. Without both, splitting "abc" on
             * a pattern that can match nothing grows an empty part at each end. */
            if (((size_t)ranges[0] == (size_t)ranges[1] && (size_t)ranges[0] == from) ||
                (size_t)ranges[0] == count) {
                at = ranges[1] == ranges[0] ? (size_t)ranges[1] + 1 : (size_t)ranges[1];
                if (at > count) {
                    break;
                }
                continue;
            }
            status = kiln_c99_array_push(collected,
                                         kiln_c99_from_units(units + from,
                                                             (size_t)ranges[0] - from));
            /* A captured group becomes an element of its own, which is what makes
             * `"a1b".split(/(\d)/)` three parts rather than two. */
            for (group = 1; group < groups && status == KILN_C99_OK; ++group) {
                status = kiln_c99_array_push(
                    collected, ranges[group * 2] < 0
                                   ? kiln_c99_undefined()
                                   : kiln_c99_from_units(units + (size_t)ranges[group * 2],
                                                         (size_t)(ranges[group * 2 + 1] -
                                                                  ranges[group * 2])));
            }
            from = (size_t)ranges[1];
        } else {
            kiln_value head = kiln_c99_from_units(units + from, (size_t)ranges[0] - from);
            if (kiln_c99_text_append(&built, head.ptr, head.len) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else if (kiln_c99_is_callable(with)) {
                /* The callback is handed the match, then each group, then the index and the whole
                 * subject -- which is why it cannot live in the string block. */
                kiln_value produced;
                kiln_value handed[64];
                size_t group;
                size_t handed_count = groups + 2 > 64 ? 64 : groups + 2;
                for (group = 0; group + 2 < handed_count; ++group) {
                    handed[group] = ranges[group * 2] < 0
                                        ? kiln_c99_undefined()
                                        : kiln_c99_from_units(units + (size_t)ranges[group * 2],
                                                              (size_t)(ranges[group * 2 + 1] -
                                                                       ranges[group * 2]));
                }
                handed[handed_count - 2] = kiln_c99_number((double)ranges[0]);
                handed[handed_count - 1] = kiln_c99_from_units(units, count);
                status = kiln_c99_call(host, &produced, with, kiln_c99_undefined(), handed,
                                       handed_count, depth, thrown);
                if (status == KILN_C99_OK) {
                    kiln_value text = kiln_c99_text(produced);
                    status = text.tag == KILN_VALUE_STRING &&
                                     kiln_c99_text_append(&built, text.ptr, text.len) ==
                                         KILN_C99_OK
                                 ? KILN_C99_OK
                                 : KILN_C99_OUT_OF_MEMORY;
                }
            } else {
                status = kiln_c99_expand_replacement(&built, replacement, replacement_count, units,
                                                     count, ranges, groups);
            }
            from = (size_t)ranges[1];
        }
        if (status != KILN_C99_OK) {
            break;
        }
        at = ranges[1] == ranges[0] ? (size_t)ranges[1] + 1 : (size_t)ranges[1];
        if (!every) {
            break;
        }
    }
    if (regexp->global || regexp->sticky) {
        /* Every one of these leaves the cursor where it found it, which `exec` relies on and
         * `match` deliberately resets. */
        regexp->last_index = 0.0;
    }
    if (status == KILN_C99_OK) {
        switch (code) {
        case KILN_C99_METHOD_STRING_SEARCH:
            *result = kiln_c99_number(-1.0);
            break;
        case KILN_C99_METHOD_STRING_MATCH:
            /* No match at all is null rather than an empty array: the two are different answers
             * and a program tests for the first. */
            *result = kiln_c99_array_of(collected)->count == 0 ? kiln_c99_null() : collected;
            break;
        case KILN_C99_METHOD_STRING_MATCH_ALL:
            status = kiln_c99_new_iterator(result, collected);
            break;
        case KILN_C99_METHOD_SPLIT:
            status = kiln_c99_array_push(collected,
                                         kiln_c99_from_units(units + from, count - from));
            *result = collected;
            break;
        default: {
            kiln_value rest = kiln_c99_from_units(units + from, count - from);
            if (kiln_c99_text_append(&built, rest.ptr, rest.len) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
            } else {
                *result = kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
            }
            break;
        }
        }
    }
    free(units);
    free(replacement);
    return status;
}

static int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown) {
    kiln_array *array;
    size_t index;
    int status;
    if (receiver.tag == KILN_VALUE_STRING &&
        (code == KILN_C99_METHOD_STRING_MATCH || code == KILN_C99_METHOD_STRING_MATCH_ALL ||
         code == KILN_C99_METHOD_STRING_SEARCH ||
         ((code == KILN_C99_METHOD_REPLACE || code == KILN_C99_METHOD_REPLACE_ALL ||
           code == KILN_C99_METHOD_SPLIT) &&
          argc > 0 && args[0].tag == KILN_VALUE_REGEXP) ||
         ((code == KILN_C99_METHOD_REPLACE || code == KILN_C99_METHOD_REPLACE_ALL) && argc > 1 &&
          kiln_c99_is_callable(args[1])))) {
        return kiln_c99_string_pattern(host, result, code, receiver, args, argc, depth, thrown);
    }
    if (code >= KILN_C99_METHOD_MATH_FLOOR && code < KILN_C99_METHOD_ARRAY_AT) {
        /* A namespace method takes no receiver, so `Math` contributes nothing beyond having been
         * the way the function was reached. */
        double first = 0.0 / 0.0;
        double second = 0.0 / 0.0;
        /* Only the methods that actually want a number reduce their arguments. `Object.assign`,
         * `JSON.stringify` and `Function.prototype.call` are reached through this same range, and
         * `call` in particular takes a *receiver* in `args[0]` -- running `valueOf` on it would be
         * a user-visible call the language never asks for. Coercing every argument here was safe
         * only while `kiln_c99_as_number` was pure; now that it can reach a program's `valueOf`,
         * the set has to be named. The global `isNaN`/`isFinite` are in it and the `Number` pair
         * beside them is not, for the reason given at their cases below. */
        /* `min`, `max` and `hypot` are variadic and reduce each argument themselves, so they must
         * not be reduced here as well: a `valueOf` that counts its own calls sees the difference,
         * and ToNumber is specified to happen once per argument. */
        if ((code <= KILN_C99_METHOD_MATH_ATAN && code != KILN_C99_METHOD_MATH_MIN &&
             code != KILN_C99_METHOD_MATH_MAX && code != KILN_C99_METHOD_MATH_HYPOT) ||
            code == KILN_C99_METHOD_GLOBAL_IS_NAN || code == KILN_C99_METHOD_GLOBAL_IS_FINITE) {
            if (argc > 0) {
                int reduced = kiln_c99_to_number_checked(host, args[0], &first, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
            }
            if (argc > 1) {
                int reduced = kiln_c99_to_number_checked(host, args[1], &second, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
            }
        } else {
            first = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0 / 0.0;
            second = argc > 1 ? kiln_c99_as_number(args[1]) : 0.0 / 0.0;
        }
        switch (code) {
        case KILN_C99_METHOD_FUNCTION_CALL:
            /* The receiver is the function; the first argument becomes *its* receiver. */
            return kiln_c99_call(host, result, receiver,
                                 argc > 0 ? args[0] : kiln_c99_undefined(),
                                 argc > 1 ? args + 1 : NULL, argc > 1 ? argc - 1 : 0, depth,
                                 thrown);
        case KILN_C99_METHOD_FUNCTION_APPLY: {
            kiln_array *given = NULL;
            if (argc > 1 && args[1].tag == KILN_VALUE_ARRAY) {
                given = kiln_c99_array_of(args[1]);
            }
            return kiln_c99_call(host, result, receiver,
                                 argc > 0 ? args[0] : kiln_c99_undefined(),
                                 given == NULL || given->count == 0 ? NULL : given->elements,
                                 given == NULL ? 0 : given->count, depth, thrown);
        }
        case KILN_C99_METHOD_FUNCTION_BIND:
            return kiln_c99_bind_with(result, receiver,
                                      argc > 0 ? args[0] : kiln_c99_undefined(),
                                      argc > 1 ? args + 1 : NULL, argc > 1 ? argc - 1 : 0);
        case KILN_C99_METHOD_NUMBER_VALUE_OF:
            /* The three below are the exception to the comment above: they are reached through a
             * number rather than through a namespace, so the receiver is the whole subject. */
            if (receiver.tag == KILN_VALUE_BIGINT) {
                /* `valueOf` hands back the primitive it was called on, whichever type it is. */
                *result = receiver;
                return KILN_C99_OK;
            }
            *result = kiln_c99_number(kiln_c99_as_number(receiver));
            return KILN_C99_OK;
        case KILN_C99_METHOD_NUMBER_TO_STRING: {
            double subject;
            if (receiver.tag == KILN_VALUE_BIGINT) {
                double base = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED ? first : 10.0;
                if (!(base >= 2.0 && base <= 36.0)) {
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                                "toString() radix must be between 2 and 36");
                }
                *result = kiln_c99_bigint_text(receiver, (uint32_t)base);
                return KILN_C99_OK;
            }
            subject = kiln_c99_as_number(receiver);
            double radix = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED ? first : 10.0;
            if (radix == 10.0) {
                /* Radix 10 is exactly `String(n)`, exponent rules included. */
                *result = kiln_c99_text(kiln_c99_number(subject));
                return KILN_C99_OK;
            }
            if (!(radix >= 2.0 && radix <= 36.0)) {
                /* A RangeError rather than a NaN, which would read as a result. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "toString() radix must be between 2 and 36");
            }
            return kiln_c99_number_in_radix(result, subject, (int)radix);
        }
        case KILN_C99_METHOD_NUMBER_TO_FIXED: {
            double digits = argc > 0 ? first : 0.0;
            if (!(digits >= 0.0 && digits <= 100.0)) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                            "toString() radix must be between 2 and 36");
            }
            return kiln_c99_to_fixed(result, kiln_c99_as_number(receiver), (int)digits);
        }
        case KILN_C99_METHOD_SET_TIMEOUT:
        case KILN_C99_METHOD_SET_INTERVAL: {
            kiln_timer entry;
            double delay = argc > 1 ? second : 0.0;
            /* A negative, missing or NaN delay is zero: "as soon as possible", which is still
             * after everything already queued. */
            if (!(isfinite(delay) && delay > 0.0)) {
                delay = 0.0;
            }
            status = kiln_c99_reserve((void **)&kiln_c99_timers, &kiln_c99_timer_capacity,
                                      kiln_c99_timer_count + 1, sizeof *kiln_c99_timers);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_next_sequence += 1;
            entry.id = kiln_c99_next_timer_id;
            kiln_c99_next_timer_id += 1.0;
            entry.due = kiln_c99_now + delay;
            entry.sequence = kiln_c99_next_sequence;
            entry.callback = argc > 0 ? args[0] : kiln_c99_undefined();
            entry.repeats = code == KILN_C99_METHOD_SET_INTERVAL;
            entry.repeat = delay;
            /* Everything after the delay is handed to the callback, which is how a timer carries
             * state without a closure. */
            entry.argc = argc > 2 ? argc - 2 : 0;
            entry.args = NULL;
            if (entry.argc != 0) {
                entry.args = (kiln_value *)malloc(entry.argc * sizeof *entry.args);
                if (entry.args == NULL) {
                    return KILN_C99_OUT_OF_MEMORY;
                }
                memcpy(entry.args, args + 2, entry.argc * sizeof *entry.args);
            }
            kiln_c99_timers[kiln_c99_timer_count] = entry;
            kiln_c99_timer_count += 1;
            *result = kiln_c99_number(entry.id);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_CLEAR_TIMEOUT:
        case KILN_C99_METHOD_CLEAR_INTERVAL: {
            double id = argc > 0 ? first : 0.0 / 0.0;
            size_t index = 0;
            while (index < kiln_c99_timer_count) {
                if (kiln_c99_timers[index].id == id) {
                    memmove(kiln_c99_timers + index, kiln_c99_timers + index + 1,
                            (kiln_c99_timer_count - index - 1) * sizeof *kiln_c99_timers);
                    kiln_c99_timer_count -= 1;
                    continue;
                }
                index += 1;
            }
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_QUEUE_MICROTASK: {
            /* The same queue a promise reaction goes on, which is the whole point of the
             * function: it is how a program gets in line behind the reactions already there
             * rather than in front of them. */
            if (argc == 0 || !kiln_c99_is_callable(args[0])) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "is not a function");
            }
            *result = kiln_c99_undefined();
            return kiln_c99_enqueue(args[0], kiln_c99_undefined(), kiln_c99_undefined(), 0);
        }
        case KILN_C99_METHOD_ESCAPE:
        case KILN_C99_METHOD_UNESCAPE: {
            /* Annex B's pre-Unicode pair, which real code still reaches for:
             * `unescape(encodeURIComponent(s))` is how a library gets a string's UTF-8 bytes one
             * per code unit. They work in code units rather than scalars, which is exactly why
             * they were replaced and exactly why they cannot be written in terms of the four
             * below. */
            kiln_value subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_text built = {NULL, 0, 0};
            uint16_t *units = NULL;
            size_t count = 0;
            size_t at = 0;
            int failed = 0;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_to_units(subject, &units, &count) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (code == KILN_C99_METHOD_ESCAPE) {
                for (at = 0; at < count && !failed; ++at) {
                    uint16_t unit = units[at];
                    char character = (char)unit;
                    if (unit < 128 &&
                        (isalnum((unsigned char)character) || character == '@' ||
                         character == '*' || character == '_' || character == '+' ||
                         character == '-' || character == '.' || character == '/')) {
                        failed = kiln_c99_text_append(&built, &character, 1) != KILN_C99_OK;
                        continue;
                    }
                    /* Two hex digits below 256, four with a `%u` prefix above it. That split is
                     * the whole reason `escape` was replaced. */
                    failed = kiln_c99_text_append_c(&built, unit < 256 ? "%" : "%u") !=
                                 KILN_C99_OK ||
                             kiln_c99_append_hex(&built, unit, unit < 256 ? 2 : 4) != KILN_C99_OK;
                }
                free(units);
                if (failed) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                *result = kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
                return KILN_C99_OK;
            }
            {
                uint16_t *decoded = (uint16_t *)malloc((count == 0 ? 1 : count) * sizeof *decoded);
                size_t written = 0;
                if (decoded == NULL) {
                    free(units);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                while (at < count) {
                    int high;
                    int low;
                    if (units[at] != (uint16_t)'%') {
                        decoded[written++] = units[at++];
                        continue;
                    }
                    if (at + 5 < count && units[at + 1] == (uint16_t)'u') {
                        int digit;
                        uint32_t value = 0;
                        int usable = 1;
                        for (digit = 0; digit < 4 && usable; ++digit) {
                            uint16_t unit = units[at + 2 + (size_t)digit];
                            int nibble = unit < 128 ? kiln_c99_hex_digit((char)unit) : -1;
                            usable = nibble >= 0;
                            value = (value << 4) | (uint32_t)(nibble < 0 ? 0 : nibble);
                        }
                        if (usable) {
                            decoded[written++] = (uint16_t)value;
                            at += 6;
                            continue;
                        }
                    }
                    high = at + 2 < count && units[at + 1] < 128
                               ? kiln_c99_hex_digit((char)units[at + 1])
                               : -1;
                    low = at + 2 < count && units[at + 2] < 128
                              ? kiln_c99_hex_digit((char)units[at + 2])
                              : -1;
                    if (high >= 0 && low >= 0) {
                        decoded[written++] = (uint16_t)(high * 16 + low);
                        at += 3;
                        continue;
                    }
                    /* A `%` that does not introduce an escape stands for itself, unlike
                     * `decodeURI` where the same text is an error. */
                    decoded[written++] = units[at++];
                }
                free(units);
                *result = kiln_c99_from_units(decoded, written);
                free(decoded);
                return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
            }
        }
        case KILN_C99_METHOD_ENCODE_URI:
        case KILN_C99_METHOD_ENCODE_URI_COMPONENT: {
            /* The four URI functions differ only in which characters they leave alone.
             * `encodeURI` keeps the ones that structure a URI, because it is encoding a whole
             * one; `encodeURIComponent` encodes those too, because a component containing a `/`
             * must not look like a path separator. */
            kiln_value subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_text built = {NULL, 0, 0};
            size_t at;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            for (at = 0; at < subject.len; ++at) {
                char character = subject.ptr[at];
                unsigned char byte = (unsigned char)character;
                int unreserved =
                    byte < 128 &&
                    (isalnum(byte) || character == '-' || character == '_' || character == '.' ||
                     character == '!' || character == '~' || character == '*' ||
                     character == '\'' || character == '(' || character == ')');
                int reserved = byte < 128 && (character == ';' || character == '/' ||
                                              character == '?' || character == ':' ||
                                              character == '@' || character == '&' ||
                                              character == '=' || character == '+' ||
                                              character == '$' || character == ',' ||
                                              character == '#');
                int ok;
                if (unreserved || (code == KILN_C99_METHOD_ENCODE_URI && reserved)) {
                    ok = kiln_c99_text_append(&built, &character, 1) == KILN_C99_OK;
                } else {
                    /* A non-ASCII byte is never in either set, so walking bytes rather than
                     * scalars percent-encodes each byte of a multi-byte character, which is what
                     * the specification says to do. */
                    ok = kiln_c99_text_append_c(&built, "%") == KILN_C99_OK &&
                         kiln_c99_append_hex(&built, byte, 2) == KILN_C99_OK;
                }
                if (!ok) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
            *result = kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DECODE_URI:
        case KILN_C99_METHOD_DECODE_URI_COMPONENT: {
            kiln_value subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_text built = {NULL, 0, 0};
            size_t at = 0;
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            while (at < subject.len) {
                int high;
                int low;
                char byte;
                if (subject.ptr[at] != '%') {
                    if (kiln_c99_text_append(&built, subject.ptr + at, 1) != KILN_C99_OK) {
                        free(built.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    at += 1;
                    continue;
                }
                high = at + 2 < subject.len ? kiln_c99_hex_digit(subject.ptr[at + 1]) : -1;
                low = at + 2 < subject.len ? kiln_c99_hex_digit(subject.ptr[at + 2]) : -1;
                if (high < 0 || low < 0) {
                    /* A `%` not followed by two hex digits is malformed, not a literal percent. */
                    free(built.bytes);
                    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                                "URI malformed");
                }
                byte = (char)(high * 16 + low);
                /* `decodeURI` leaves the reserved set encoded: undoing it would change what the
                 * URI means, which is the one thing decoding must not do. */
                if (code == KILN_C99_METHOD_DECODE_URI &&
                    (byte == ';' || byte == '/' || byte == '?' || byte == ':' || byte == '@' ||
                     byte == '&' || byte == '=' || byte == '+' || byte == '$' || byte == ',' ||
                     byte == '#')) {
                    if (kiln_c99_text_append(&built, subject.ptr + at, 3) != KILN_C99_OK) {
                        free(built.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                } else if (kiln_c99_text_append(&built, &byte, 1) != KILN_C99_OK) {
                    free(built.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                at += 3;
            }
            if (!kiln_c99_valid_utf8(built.bytes, built.len)) {
                /* The escapes named bytes that are not a string. */
                free(built.bytes);
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR, "URI malformed");
            }
            *result = kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_GLOBAL_IS_NAN:
            /* The global pair coerces where the `Number` pair does not: `isNaN('x')` is true and
             * `Number.isNaN('x')` is false, because the second asks whether the value *is* the
             * NaN number rather than whether it reads as one. */
            *result = kiln_c99_boolean(isnan(first));
            return KILN_C99_OK;
        case KILN_C99_METHOD_GLOBAL_IS_FINITE:
            *result = kiln_c99_boolean(isfinite(first) ? 1 : 0);
            return KILN_C99_OK;
        case KILN_C99_METHOD_PARSE_INT:
        case KILN_C99_METHOD_PARSE_FLOAT: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value text;
            const char *bytes;
            size_t len;
            size_t at = 0;
            /* These two read a *string*, so the string hint: `toString` before `valueOf`, and
             * `parseInt({toString: () => "99"})` is 99. The same split `kiln_c99_to_number_checked`
             * makes for the number hint -- `kiln_c99_text` is pure, so it cannot run a `toString`
             * the program wrote and answered NaN for every object that has one. */
            if (kiln_c99_is_reference(subject)) {
                int reduced = kiln_c99_to_primitive(host, subject, 1, &subject, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
            }
            text = kiln_c99_text(subject);
            if (text.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            bytes = text.ptr;
            len = text.len;
            /* Both read as far as they can and stop, rather than rejecting a string with a tail:
             * `parseInt('12px')` is 12. That tolerance is the whole difference between these and
             * `Number(x)`, which rejects the same input. */
            while (at < len && kiln_c99_is_trimmed((uint16_t)(unsigned char)bytes[at])) {
                at += 1;
            }
            if (code == KILN_C99_METHOD_PARSE_INT) {
                double radix = argc > 1 ? kiln_c99_as_number(args[1]) : 0.0;
                double magnitude = 0.0;
                int base;
                int negative = 0;
                size_t digits_at;
                radix = isfinite(radix) ? trunc(radix) : 0.0;
                if (!(radix == 0.0 || (radix >= 2.0 && radix <= 36.0))) {
                    *result = kiln_c99_number(0.0 / 0.0);
                    return KILN_C99_OK;
                }
                base = (int)radix;
                if (at < len && (bytes[at] == '-' || bytes[at] == '+')) {
                    negative = bytes[at] == '-';
                    at += 1;
                }
                /* `0x` selects hexadecimal when no radix was given, and is allowed when 16 was. */
                if ((base == 0 || base == 16) && at + 1 < len && bytes[at] == '0' &&
                    (bytes[at + 1] == 'x' || bytes[at + 1] == 'X')) {
                    base = 16;
                    at += 2;
                } else if (base == 0) {
                    base = 10;
                }
                digits_at = at;
                while (at < len) {
                    int digit;
                    char character = bytes[at];
                    if (character >= '0' && character <= '9') {
                        digit = character - '0';
                    } else if (character >= 'a' && character <= 'z') {
                        digit = character - 'a' + 10;
                    } else if (character >= 'A' && character <= 'Z') {
                        digit = character - 'A' + 10;
                    } else {
                        break;
                    }
                    if (digit >= base) {
                        break;
                    }
                    /* Folded as a double rather than accumulated as an integer: the result is a
                     * double either way, and a long run of digits has to saturate toward infinity
                     * rather than overflow. */
                    magnitude = magnitude * (double)base + (double)digit;
                    at += 1;
                }
                *result = kiln_c99_number(at == digits_at ? 0.0 / 0.0
                                                          : (negative ? -magnitude : magnitude));
                return KILN_C99_OK;
            }
            {
                /* The longest prefix that is a decimal literal, scanned rather than handed to
                 * `strtod` outright: C's parser also accepts `0x1f`, `inf` and `nan`, none of
                 * which this grammar has -- `parseFloat('0x1f')` is 0. */
                size_t start = at;
                size_t scan;
                size_t end;
                size_t before = 0;
                size_t after = 0;
                double parsed;
                if (at < len && (bytes[at] == '-' || bytes[at] == '+')) {
                    at += 1;
                }
                if (len - at >= 8 && memcmp(bytes + at, "Infinity", 8) == 0) {
                    *result = kiln_c99_number(bytes[start] == '-' ? -1.0 / 0.0 : 1.0 / 0.0);
                    return KILN_C99_OK;
                }
                scan = at;
                while (scan < len && bytes[scan] >= '0' && bytes[scan] <= '9') {
                    scan += 1;
                    before += 1;
                }
                if (scan < len && bytes[scan] == '.') {
                    scan += 1;
                    while (scan < len && bytes[scan] >= '0' && bytes[scan] <= '9') {
                        scan += 1;
                        after += 1;
                    }
                }
                if (before + after == 0) {
                    *result = kiln_c99_number(0.0 / 0.0);
                    return KILN_C99_OK;
                }
                end = scan;
                if (scan < len && (bytes[scan] == 'e' || bytes[scan] == 'E')) {
                    size_t probe = scan + 1;
                    if (probe < len && (bytes[probe] == '+' || bytes[probe] == '-')) {
                        probe += 1;
                    }
                    if (probe < len && bytes[probe] >= '0' && bytes[probe] <= '9') {
                        while (probe < len && bytes[probe] >= '0' && bytes[probe] <= '9') {
                            probe += 1;
                        }
                        /* An exponent with no digits is not part of the literal: `1e` is 1. */
                        end = probe;
                    }
                }
                /* The scanned prefix is a `StringNumericLiteral` by construction, so the
                 * provider's parser is exactly the right reader for it -- and it is the same one
                 * `Number(x)` uses, which is what keeps the two agreeing. `strtod` would read the
                 * process locale's decimal point, and an embedded module runs in its host's. */
                parsed = kiln_string_to_number(bytes + start, end - start);
                *result = kiln_c99_number(parsed);
                return KILN_C99_OK;
            }
        }
        case KILN_C99_METHOD_MATH_FLOOR: *result = kiln_c99_number(floor(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_CEIL: *result = kiln_c99_number(ceil(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ROUND: {
            /* Half rounds *up*, not away from zero, so `floor(x + 0.5)` is the definition. It
             * loses the sign of `Math.round(-0.5)`, which is -0 and not 0 -- a difference a
             * console shows and `Object.is` can see. */
            double rounded = isfinite(first) ? floor(first + 0.5) : first;
            if (rounded == 0.0 && kiln_c99_signbit(first)) {
                rounded = -0.0;
            }
            *result = kiln_c99_number(rounded);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MATH_TRUNC: *result = kiln_c99_number(trunc(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ABS: *result = kiln_c99_number(fabs(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SIGN:
            *result = kiln_c99_number(isnan(first) || first == 0.0 ? first
                                      : first < 0.0                ? -1.0
                                                                   : 1.0);
            return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SQRT: *result = kiln_c99_number(sqrt(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_CBRT: *result = kiln_c99_number(cbrt(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_POW: *result = kiln_c99_number(pow(first, second)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_LOG: *result = kiln_c99_number(log(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_LOG2: *result = kiln_c99_number(log2(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_LOG10: *result = kiln_c99_number(log10(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_EXP: *result = kiln_c99_number(exp(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ATAN2: *result = kiln_c99_number(atan2(first, second)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SIN: *result = kiln_c99_number(sin(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_COS: *result = kiln_c99_number(cos(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_TAN: *result = kiln_c99_number(tan(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ASIN: *result = kiln_c99_number(asin(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ACOS: *result = kiln_c99_number(acos(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ATAN: *result = kiln_c99_number(atan(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_SINH: *result = kiln_c99_number(sinh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_COSH: *result = kiln_c99_number(cosh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_TANH: *result = kiln_c99_number(tanh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ASINH: *result = kiln_c99_number(asinh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ACOSH: *result = kiln_c99_number(acosh(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_ATANH: *result = kiln_c99_number(atanh(first)); return KILN_C99_OK;
        /* `ln(1 + x)` and `e^x - 1` as one operation each, which is the whole point of both: the
         * two-step spelling loses every significant digit for small `x`. */
        case KILN_C99_METHOD_MATH_LOG1P: *result = kiln_c99_number(log1p(first)); return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_EXPM1: *result = kiln_c99_number(expm1(first)); return KILN_C99_OK;
        /* The nearest `float`, widened back. A round trip, not a truncation. */
        case KILN_C99_METHOD_MATH_FROUND:
            *result = kiln_c99_number((double)(float)first);
            return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_CLZ32: {
            uint32_t bits = kiln_c99_to_uint32(first);
            int leading = 0;
            while (leading < 32 && (bits & (UINT32_C(0x80000000) >> leading)) == 0) {
                leading += 1;
            }
            *result = kiln_c99_number((double)leading);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MATH_IMUL: {
            /* Both operands are truncated to 32 bits *first*, which is what makes this different
             * from `a * b`: the product is exact in 32 bits and wraps rather than rounding. */
            uint32_t product = kiln_c99_to_uint32(first) * kiln_c99_to_uint32(second);
            *result = kiln_c99_number((double)(int32_t)product);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_REGEXP_TO_STRING:
            if (receiver.tag != KILN_VALUE_REGEXP) {
                return KILN_C99_CONTROL_ERROR;
            }
            *result = kiln_c99_text(receiver);
            return result->tag == KILN_VALUE_STRING ? KILN_C99_OK : KILN_C99_OUT_OF_MEMORY;
        case KILN_C99_METHOD_REGEXP_TEST:
        case KILN_C99_METHOD_REGEXP_EXEC: {
            kiln_regexp *regexp;
            kiln_value subject;
            uint16_t *units = NULL;
            size_t count = 0;
            size_t start;
            size_t groups = 0;
            int matched = 0;
            int64_t ranges[64];
            if (receiver.tag != KILN_VALUE_REGEXP) {
                return KILN_C99_CONTROL_ERROR;
            }
            regexp = kiln_c99_regexp_of(receiver);
            subject = kiln_c99_text(argc > 0 ? args[0] : kiln_c99_undefined());
            if (subject.tag != KILN_VALUE_STRING) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_to_units(subject, &units, &count) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* Only a `g` or `y` pattern has a cursor; everything else searches from the start
             * however many times it is asked. */
            start = regexp->global || regexp->sticky
                        ? (regexp->last_index < 0.0 ? 0 : (size_t)regexp->last_index)
                        : 0;
            status = kiln_c99_regexp_find(regexp, units, count, start, ranges,
                                          sizeof ranges / sizeof *ranges, &groups, &matched);
            if (status != KILN_C99_OK) {
                free(units);
                return status;
            }
            if (regexp->global || regexp->sticky) {
                /* A miss resets the cursor, which is what makes a `while (re.exec(s))` loop
                 * terminate rather than repeat its last match forever. */
                regexp->last_index = matched ? (double)ranges[1] : 0.0;
            }
            if (!matched) {
                free(units);
                *result = code == KILN_C99_METHOD_REGEXP_TEST ? kiln_c99_boolean(0)
                                                              : kiln_c99_null();
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_REGEXP_TEST) {
                free(units);
                *result = kiln_c99_boolean(1);
                return KILN_C99_OK;
            }
            {
                /* The array `exec` produces: the whole match, then each group, with `index`,
                 * `input` and `groups` alongside -- properties *of the array*, which is why an
                 * array needs somewhere to keep something that is not an element. */
                kiln_value built;
                size_t group;
                status = kiln_c99_new_array(&built);
                for (group = 0; group < groups && status == KILN_C99_OK; ++group) {
                    int64_t from = ranges[group * 2];
                    int64_t to = ranges[group * 2 + 1];
                    /* A group that did not participate is undefined rather than empty. */
                    status = kiln_c99_array_push(
                        built, from < 0 ? kiln_c99_undefined()
                                        : kiln_c99_from_units(units + (size_t)from,
                                                              (size_t)(to - from)));
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, built, "index", 5,
                                          kiln_c99_number((double)ranges[0]), depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, built, "input", 5, kiln_c99_from_units(units, count),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    /* Present and undefined rather than absent: a program reads `m.groups?.name`,
                     * and the key being there is what stops that from looking like a different
                     * shape. Named groups themselves are not collected on either side yet. */
                    status = kiln_c99_put(host, built, "groups", 6, kiln_c99_undefined(), depth,
                                          thrown);
                }
                free(units);
                *result = built;
                return status;
            }
        }
        case KILN_C99_METHOD_MATH_RANDOM:
            *result = kiln_c99_number(kiln_c99_random());
            return KILN_C99_OK;
        case KILN_C99_METHOD_MATH_HYPOT: {
            /* A fold of `hypot` rather than `sqrt(x*x + y*y)`: squaring overflows for arguments
             * whose result is perfectly representable, which is the whole reason it exists. */
            double total = 0.0;
            for (index = 0; index < argc; ++index) {
                double leg;
                int reduced = kiln_c99_to_number_checked(host, args[index], &leg, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
                total = hypot(total, leg);
            }
            *result = kiln_c99_number(total);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_MATH_MIN:
        case KILN_C99_METHOD_MATH_MAX: {
            /* No arguments is the identity for the comparison, which is the infinity that loses
             * every one of them. */
            double best = code == KILN_C99_METHOD_MATH_MIN ? 1.0 / 0.0 : -1.0 / 0.0;
            for (index = 0; index < argc; ++index) {
                double value;
                int reduced = kiln_c99_to_number_checked(host, args[index], &value, depth, thrown);
                if (reduced != KILN_C99_OK) {
                    return reduced;
                }
                if (isnan(value)) {
                    best = value;
                    break;
                }
                if (code == KILN_C99_METHOD_MATH_MIN ? value < best : value > best) {
                    best = value;
                }
            }
            *result = kiln_c99_number(best);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_IS_INTEGER:
        case KILN_C99_METHOD_IS_SAFE_INTEGER:
            /* `Number.isInteger` does not coerce, unlike the global `isNaN` and friends: a
             * string is simply not an integer. Same code with coercion would be a different
             * function, so it is a different function. */
            *result = kiln_c99_boolean(
                argc > 0 && args[0].tag == KILN_VALUE_NUMBER && isfinite(first) &&
                floor(first) == first &&
                (code == KILN_C99_METHOD_IS_INTEGER || fabs(first) <= 9007199254740991.0));
            return KILN_C99_OK;
        case KILN_C99_METHOD_IS_NAN:
            *result = kiln_c99_boolean(argc > 0 && args[0].tag == KILN_VALUE_NUMBER &&
                                       isnan(first));
            return KILN_C99_OK;
        case KILN_C99_METHOD_IS_FINITE:
            *result = kiln_c99_boolean(argc > 0 && args[0].tag == KILN_VALUE_NUMBER &&
                                       isfinite(first));
            return KILN_C99_OK;
        case KILN_C99_METHOD_IS_ARRAY:
            *result = kiln_c99_boolean(argc > 0 && args[0].tag == KILN_VALUE_ARRAY);
            return KILN_C99_OK;
        case KILN_C99_METHOD_ARRAY_OF: {
            kiln_value built;
            status = kiln_c99_new_array(&built);
            for (index = 0; index < argc && status == KILN_C99_OK; ++index) {
                status = kiln_c99_array_push(built, args[index]);
            }
            *result = built;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_KEYS:
        case KILN_C99_METHOD_OBJECT_VALUES:
        case KILN_C99_METHOD_OBJECT_ENTRIES: {
            kiln_value built;
            kiln_object *object;
            if (argc == 0) {
                return KILN_C99_CONTROL_ERROR;
            }
            /* An array's own enumerable keys are its indices, as strings, and a hole is not one
             * of them -- which is the same walk `for...in` does, so it is the same code. */
            if (args[0].tag == KILN_VALUE_ARRAY) {
                kiln_array *array = kiln_c99_array_of(args[0]);
                status = kiln_c99_new_array(&built);
                for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
                    if (kiln_c99_is_hole(array->elements[index])) {
                        continue;
                    }
                    if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                        status = kiln_c99_array_push(
                            built, kiln_c99_text(kiln_c99_number((double)index)));
                    } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                        status = kiln_c99_array_push(built, array->elements[index]);
                    } else {
                        kiln_value pair;
                        status = kiln_c99_new_array(&pair);
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(
                                pair, kiln_c99_text(kiln_c99_number((double)index)));
                        }
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(pair, array->elements[index]);
                        }
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_array_push(built, pair);
                        }
                    }
                }
                *result = built;
                return status;
            }
            if (args[0].tag != KILN_VALUE_OBJECT) {
                return KILN_C99_CONTROL_ERROR;
            }
            object = kiln_c99_object_of(args[0]);
            status = kiln_c99_new_array(&built);
            for (index = 0; index < object->count && status == KILN_C99_OK; ++index) {
                kiln_value key;
                if (!object->properties[index].enumerable) {
                    continue;
                }
                key = kiln_c99_copy_text(object->properties[index].key,
                                         object->properties[index].key_len);
                if (code == KILN_C99_METHOD_OBJECT_KEYS) {
                    status = kiln_c99_array_push(built, key);
                } else if (code == KILN_C99_METHOD_OBJECT_VALUES) {
                    status = kiln_c99_array_push(built, object->properties[index].value);
                } else {
                    kiln_value pair;
                    status = kiln_c99_new_array(&pair);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(pair, key);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(pair, object->properties[index].value);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(built, pair);
                    }
                }
            }
            *result = built;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_ASSIGN: {
            kiln_value target;
            size_t argument;
            if (argc == 0 || args[0].tag != KILN_VALUE_OBJECT) {
                return KILN_C99_CONTROL_ERROR;
            }
            target = args[0];
            for (argument = 1; argument < argc; ++argument) {
                kiln_object *source;
                if (args[argument].tag == KILN_VALUE_UNDEFINED ||
                    args[argument].tag == KILN_VALUE_NULL) {
                    continue;
                }
                if (args[argument].tag != KILN_VALUE_OBJECT) {
                    return KILN_C99_CONTROL_ERROR;
                }
                source = kiln_c99_object_of(args[argument]);
                for (index = 0; index < source->count; ++index) {
                    if (!source->properties[index].enumerable) {
                        continue;
                    }
                    status = kiln_c99_put(host, target, source->properties[index].key,
                                          source->properties[index].key_len,
                                          source->properties[index].value, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            }
            *result = target;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DATE_NOW:
            /* The clock is virtual and nothing here advances it (ADR 0015). A program that asks
             * the time twice gets the same answer, which is what makes a run repeatable -- and
             * is why a probe may compare `typeof Date.now()` and nothing more. */
            *result = kiln_c99_number(0.0);
            return KILN_C99_OK;
        case KILN_C99_METHOD_COLLECTION_GET:
        case KILN_C99_METHOD_COLLECTION_SET:
        case KILN_C99_METHOD_COLLECTION_ADD:
        case KILN_C99_METHOD_COLLECTION_HAS:
        case KILN_C99_METHOD_COLLECTION_DELETE:
        case KILN_C99_METHOD_COLLECTION_CLEAR:
        case KILN_C99_METHOD_COLLECTION_FOR_EACH:
        case KILN_C99_METHOD_COLLECTION_KEYS:
        case KILN_C99_METHOD_COLLECTION_VALUES:
        case KILN_C99_METHOD_COLLECTION_ENTRIES:
        case KILN_C99_METHOD_COLLECTION_SELF: {
            kiln_collection *collection;
            kiln_value first = argc > 0 ? args[0] : kiln_c99_undefined();
            double at;
            if (receiver.tag != KILN_VALUE_COLLECTION) {
                return KILN_C99_CONTROL_ERROR;
            }
            collection = kiln_c99_collection_of(receiver);
            switch (code) {
            case KILN_C99_METHOD_COLLECTION_GET:
                at = kiln_c99_collection_find(collection, first);
                *result = at < 0.0 ? kiln_c99_undefined() : collection->entries[(size_t)at].value;
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_HAS:
                *result = kiln_c99_boolean(kiln_c99_collection_find(collection, first) >= 0.0);
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_SET:
            case KILN_C99_METHOD_COLLECTION_ADD:
                /* A Set stores each value under itself, which is what makes `has` one lookup
                 * rather than two shapes. Both answer with the collection, so they chain. */
                status = kiln_c99_collection_put(receiver, first,
                                                 code == KILN_C99_METHOD_COLLECTION_SET
                                                     ? (argc > 1 ? args[1] : kiln_c99_undefined())
                                                     : first);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = receiver;
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_DELETE:
                at = kiln_c99_collection_find(collection, first);
                if (at < 0.0) {
                    *result = kiln_c99_boolean(0);
                    return KILN_C99_OK;
                }
                /* A hole, not a removal: a walk already past this point must not have what
                 * follows shifted under it, and one not yet there must not see this entry
                 * again. `x,z` rather than `x,y,z` when the loop deletes `y` after visiting `x`. */
                collection->entries[(size_t)at].dead = 1;
                collection->live -= 1;
                *result = kiln_c99_boolean(1);
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_CLEAR:
                collection->count = 0;
                collection->live = 0;
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            case KILN_C99_METHOD_COLLECTION_FOR_EACH: {
                /* The callback takes `(value, key, collection)`. A Set passes its value twice,
                 * which is the specification's way of giving both one signature. */
                size_t length = collection->count;
                for (index = 0; index < length && index < collection->count; ++index) {
                    kiln_value visit[3];
                    kiln_value ignored;
                    if (collection->entries[index].dead) {
                        continue;
                    }
                    visit[0] = collection->entries[index].value;
                    visit[1] = collection->entries[index].key;
                    visit[2] = receiver;
                    status = kiln_c99_call(host, &ignored, first,
                                           argc > 1 ? args[1] : kiln_c99_undefined(), visit, 3,
                                           depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                *result = kiln_c99_undefined();
                return KILN_C99_OK;
            }
            default: {
                /* A *live* iterator, holding the collection rather than a copy of it: a delete
                 * during the walk has to be seen, and a snapshot is exactly what cannot see one.
                 * What each step hands back is decided here, where the collection's kind is
                 * known, so the iterator itself needs no reference back to it. */
                int pairs = code == KILN_C99_METHOD_COLLECTION_ENTRIES ||
                            (code == KILN_C99_METHOD_COLLECTION_SELF && collection->is_map);
                int wants_keys = code == KILN_C99_METHOD_COLLECTION_KEYS;
                kiln_value iterator;
                status = kiln_c99_new_object(&iterator);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@iterated", 10, receiver, depth,
                                          thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@index", 7, kiln_c99_number(0.0),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    /* 0 the value, 1 the key, 2 a `[key, value]` pair. */
                    status = kiln_c99_put(host, iterator, "@@part", 6,
                                          kiln_c99_number(pairs ? 2.0 : (wants_keys ? 1.0 : 0.0)),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "next", 4,
                                          kiln_c99_method(KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT),
                                          depth, thrown);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, iterator, "@@iterator", 10,
                                          kiln_c99_method(KILN_C99_METHOD_ITERATOR_SELF), depth,
                                          thrown);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = iterator;
                return KILN_C99_OK;
            }
            }
        }
        case KILN_C99_METHOD_THEN:
        case KILN_C99_METHOD_CATCH:
        case KILN_C99_METHOD_FINALLY: {
            kiln_value on_fulfilled = kiln_c99_undefined();
            kiln_value on_rejected = kiln_c99_undefined();
            if (receiver.tag != KILN_VALUE_PROMISE) {
                return KILN_C99_CONTROL_ERROR;
            }
            if (code == KILN_C99_METHOD_THEN) {
                on_fulfilled = argc > 0 ? args[0] : kiln_c99_undefined();
                on_rejected = argc > 1 ? args[1] : kiln_c99_undefined();
            } else if (code == KILN_C99_METHOD_CATCH) {
                on_rejected = argc > 0 ? args[0] : kiln_c99_undefined();
            } else {
                /* `finally` runs on both sides and passes the outcome through untouched, which is
                 * what separates it from `then(f, f)`. Each side is the handler wrapped in a step
                 * that restores what it was given -- bound to the handler itself, so the pair
                 * needs no state object of its own. */
                kiln_value handler = argc > 0 ? args[0] : kiln_c99_undefined();
                status = kiln_c99_bind(&on_fulfilled,
                                       kiln_c99_method(KILN_C99_METHOD_FINALLY_PASS), handler);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_bind(&on_rejected,
                                           kiln_c99_method(KILN_C99_METHOD_FINALLY_RAISE),
                                           handler);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            return kiln_c99_then(receiver, on_fulfilled, on_rejected, result);
        }
        case KILN_C99_METHOD_PROMISE_RESOLVE:
        case KILN_C99_METHOD_PROMISE_REJECT: {
            kiln_value value = argc > 0 ? args[0] : kiln_c99_undefined();
            /* `Promise.resolve(p)` is `p`, not a promise of a promise -- which is what lets a
             * function take "a value or a promise of one" without asking which it got. */
            if (code == KILN_C99_METHOD_PROMISE_RESOLVE && value.tag == KILN_VALUE_PROMISE) {
                *result = value;
                return KILN_C99_OK;
            }
            status = kiln_c99_new_promise(result);
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_settle(*result, value, code == KILN_C99_METHOD_PROMISE_REJECT);
        }
        case KILN_C99_METHOD_PROMISE_ALL:
        case KILN_C99_METHOD_PROMISE_RACE:
        case KILN_C99_METHOD_PROMISE_ALL_SETTLED:
        case KILN_C99_METHOD_PROMISE_ANY: {
            kiln_value gathered;
            kiln_value derived;
            kiln_array *given;
            /* All four are one machine differing in what each arrival does, which is why the mode
             * travels in the shared state rather than in four copies of the wiring below. */
            double mode = code == KILN_C99_METHOD_PROMISE_RACE          ? 1.0
                          : code == KILN_C99_METHOD_PROMISE_ALL_SETTLED ? 2.0
                          : code == KILN_C99_METHOD_PROMISE_ANY         ? 3.0
                                                                        : 0.0;
            int racing = mode == 1.0;
            status = kiln_c99_new_array(&gathered);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_iterate(host, gathered, argc > 0 ? args[0] : kiln_c99_undefined(),
                                      depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_new_promise(&derived);
            if (status != KILN_C99_OK) {
                return status;
            }
            given = kiln_c99_array_of(gathered);
            if (!racing && given->count == 0) {
                /* Nothing to wait for, so it is already done. `any` is the exception: with no
                 * promise to succeed, every one of them has failed. */
                if (mode == 3.0) {
                    kiln_value aggregate;
                    status = kiln_c99_aggregate_error(&aggregate, gathered);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_settle(derived, aggregate, 1);
                    }
                } else {
                    status = kiln_c99_settle(derived, gathered, 0);
                }
                *result = derived;
                return status;
            }
            {
                /* One shared state object, and one bound step per element that knows its own
                 * index. The index has to travel with the handler, and a handler is one value,
                 * so the pair is an object the step reads itself out of. */
                kiln_value state;
                kiln_value results;
                size_t index;
                status = kiln_c99_new_object(&state);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_new_array(&results);
                }
                for (index = 0; index < given->count && status == KILN_C99_OK; ++index) {
                    status = kiln_c99_array_push(results, kiln_c99_undefined());
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@results", 9, results, 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@remaining", 11,
                                          kiln_c99_number((double)given->count), 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@derived", 9, derived, 0, NULL);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(NULL, state, "@@mode", 6, kiln_c99_number(mode), 0,
                                          NULL);
                }
                for (index = 0; index < given->count && status == KILN_C99_OK; ++index) {
                    kiln_value element;
                    kiln_value slot;
                    kiln_value step;
                    kiln_value rejecter;
                    kiln_value ignored;
                    status = kiln_c99_new_object(&slot);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(NULL, slot, "@@state", 7, state, 0, NULL);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(NULL, slot, "@@index", 7,
                                              kiln_c99_number((double)index), 0, NULL);
                    }
                    if (status != KILN_C99_OK) {
                        break;
                    }
                    element = given->elements[index];
                    if (element.tag != KILN_VALUE_PROMISE) {
                        kiln_value immediate;
                        status = kiln_c99_new_promise(&immediate);
                        if (status == KILN_C99_OK) {
                            status = kiln_c99_settle(immediate, element, 0);
                        }
                        element = immediate;
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_bind(&step, kiln_c99_method(KILN_C99_METHOD_ALL_STEP),
                                               slot);
                    }
                    if (status == KILN_C99_OK) {
                        /* For `all` and `race` a rejection settles the whole thing, whichever
                         * element it came from, and a settler is exactly that. The other two have
                         * something to record instead, so they get a step of their own. */
                        status = mode == 2.0 || mode == 3.0
                                     ? kiln_c99_bind(&rejecter,
                                                     kiln_c99_method(KILN_C99_METHOD_REJECT_STEP),
                                                     slot)
                                     : kiln_c99_new_settler(&rejecter, derived, 1);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_then(element, step, rejecter, &ignored);
                    }
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = derived;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_FINALLY_PASS:
        case KILN_C99_METHOD_FINALLY_RAISE: {
            kiln_value passed = argc > 0 ? args[0] : kiln_c99_undefined();
            kiln_value ignored;
            if (kiln_c99_is_callable(receiver)) {
                /* The handler's own failure wins: a `finally` that throws replaces the outcome
                 * it was passing through, which is the one thing that can change it. */
                status = kiln_c99_call(host, &ignored, receiver, kiln_c99_undefined(), NULL, 0,
                                       depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            {
                /* `Promise.resolve(f()).then(() => v)`, which is the specification's own spelling
                 * and the reason it matters: a handler that returns a promise is *waited for*, and
                 * the extra turn that costs is visible in the order things print. Settling here
                 * would put the outcome a turn early. */
                kiln_value awaited;
                kiln_value restore;
                status = kiln_c99_new_promise(&awaited);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_settle(awaited, ignored, 0);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_bind(
                        &restore,
                        kiln_c99_method(code == KILN_C99_METHOD_FINALLY_RAISE
                                            ? KILN_C99_METHOD_FINALLY_RERAISE
                                            : KILN_C99_METHOD_FINALLY_RESTORE),
                        passed);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                return kiln_c99_then(awaited, restore, kiln_c99_undefined(), result);
            }
        }
        case KILN_C99_METHOD_ADOPT_STEP: {
            /* Two settlers bound to the outer promise: whichever the inner one reaches, the outer
             * one follows. */
            kiln_promise *inner;
            kiln_reaction adopt;
            if (receiver.tag != KILN_VALUE_PROMISE || argc == 0 ||
                args[0].tag != KILN_VALUE_PROMISE) {
                return KILN_C99_CONTROL_ERROR;
            }
            inner = kiln_c99_promise_of(args[0]);
            status = kiln_c99_new_settler(&adopt.on_fulfilled, receiver, 0);
            if (status == KILN_C99_OK) {
                status = kiln_c99_new_settler(&adopt.on_rejected, receiver, 1);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            adopt.derived = kiln_c99_undefined();
            *result = kiln_c99_undefined();
            if (inner->state != KILN_C99_PENDING) {
                return kiln_c99_schedule(inner, &adopt);
            }
            status = kiln_c99_reserve((void **)&inner->reactions, &inner->capacity,
                                      inner->count + 1, sizeof *inner->reactions);
            if (status != KILN_C99_OK) {
                return status;
            }
            inner->reactions[inner->count] = adopt;
            inner->count += 1;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_FINALLY_RESTORE:
            /* The outcome travelled here as the receiver, because a reaction is handed the value
             * it is reacting to and that is the handler's result rather than the original. */
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_FINALLY_RERAISE:
            /* A rejection travels on, which a reaction says by throwing. */
            *thrown = receiver;
            return KILN_C99_THROWN;
        case KILN_C99_METHOD_ALL_STEP:
        case KILN_C99_METHOD_REJECT_STEP: {
            kiln_value state;
            kiln_value at;
            kiln_value results;
            kiln_value remaining;
            kiln_value derived;
            kiln_value mode;
            kiln_value arrived = argc > 0 ? args[0] : kiln_c99_undefined();
            int rejected = code == KILN_C99_METHOD_REJECT_STEP;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                return KILN_C99_CONTROL_ERROR;
            }
            status = kiln_c99_get(host, receiver, "@@state", 7, &state, depth, thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@results", 9, &results, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@remaining", 11, &remaining, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@derived", 9, &derived, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, state, "@@mode", 6, &mode, depth, thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = kiln_c99_undefined();
            if (!rejected && (kiln_c99_as_number(mode) == 1.0 || kiln_c99_as_number(mode) == 3.0)) {
                /* `race` and `any` are both settled by the first arrival that counts, and the
                 * rest are ignored -- which `settle` already does by refusing a second
                 * settlement. For `any` only a fulfilment counts, which is what puts this on the
                 * fulfilment side alone. */
                return kiln_c99_settle(derived, arrived, 0);
            }
            if (results.tag == KILN_VALUE_ARRAY) {
                kiln_array *array = kiln_c99_array_of(results);
                size_t slot = (size_t)kiln_c99_as_number(at);
                kiln_value recorded = arrived;
                if (kiln_c99_as_number(mode) == 2.0) {
                    /* `allSettled` reports the outcome as well as the value, so every element
                     * gets a record rather than a bare result. */
                    status = kiln_c99_new_object(&recorded);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(host, recorded, "status", 6,
                                              kiln_c99_copy_text(rejected ? "rejected"
                                                                          : "fulfilled",
                                                                 rejected ? 8 : 9),
                                              depth, thrown);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_put(host, recorded, rejected ? "reason" : "value",
                                              rejected ? 6 : 5, arrived, depth, thrown);
                    }
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                if (slot < array->count) {
                    array->elements[slot] = recorded;
                }
            }
            {
                double left = kiln_c99_as_number(remaining) - 1.0;
                status = kiln_c99_put(host, state, "@@remaining", 11, kiln_c99_number(left), depth,
                                      thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (left <= 0.0) {
                    if (kiln_c99_as_number(mode) == 3.0) {
                        /* Every one of them failed, and the reasons are what the caller gets. */
                        kiln_value aggregate;
                        status = kiln_c99_aggregate_error(&aggregate, results);
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                        return kiln_c99_settle(derived, aggregate, 1);
                    }
                    return kiln_c99_settle(derived, results, 0);
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ASYNC_RESUME:
        case KILN_C99_METHOD_ASYNC_THROW:
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                return KILN_C99_CONTROL_ERROR;
            }
            *result = kiln_c99_undefined();
            /* The same two handlers serve both drivers: what an arrival means depends on the
             * frame it is arriving at, not on who attached them. */
            return kiln_c99_generator_of(receiver)->async_generator
                       ? kiln_c99_async_generator_step(
                             host, receiver, argc > 0 ? args[0] : kiln_c99_undefined(),
                             code == KILN_C99_METHOD_ASYNC_THROW, depth, thrown)
                       : kiln_c99_async_step(host, receiver,
                                             argc > 0 ? args[0] : kiln_c99_undefined(),
                                             code == KILN_C99_METHOD_ASYNC_THROW, depth, thrown);
        case KILN_C99_METHOD_COLLECTION_ITERATOR_NEXT: {
            kiln_value source;
            kiln_value at;
            kiln_value part;
            kiln_value step;
            kiln_value produced = kiln_c99_undefined();
            kiln_collection *collection;
            size_t position;
            int finished;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                return KILN_C99_CONTROL_ERROR;
            }
            status = kiln_c99_get(host, receiver, "@@iterated", 10, &source, depth, thrown);
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_get(host, receiver, "@@part", 6, &part, depth, thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            if (source.tag != KILN_VALUE_COLLECTION) {
                return KILN_C99_CONTROL_ERROR;
            }
            collection = kiln_c99_collection_of(source);
            position = (size_t)kiln_c99_as_number(at);
            /* Past every dead entry: one deleted ahead of the cursor is gone, and one deleted
             * behind it has already been handed over. */
            while (position < collection->count && collection->entries[position].dead) {
                position += 1;
            }
            finished = position >= collection->count;
            if (!finished) {
                double which = kiln_c99_as_number(part);
                if (which == 2.0) {
                    status = kiln_c99_new_array(&produced);
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(produced,
                                                     collection->entries[position].key);
                    }
                    if (status == KILN_C99_OK) {
                        status = kiln_c99_array_push(produced,
                                                     collection->entries[position].value);
                    }
                } else {
                    produced = which == 1.0 ? collection->entries[position].key
                                            : collection->entries[position].value;
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_put(host, receiver, "@@index", 7,
                                          kiln_c99_number((double)(position + 1)), depth, thrown);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_new_object(&step);
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(host, step, "value", 5, produced, depth, thrown);
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(finished), depth,
                                      thrown);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = step;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ITERATOR_NEXT: {
            kiln_value items;
            kiln_value at;
            kiln_value step;
            kiln_array *array;
            size_t position;
            int finished;
            if (receiver.tag != KILN_VALUE_OBJECT) {
                return KILN_C99_CONTROL_ERROR;
            }
            status = kiln_c99_get(host, receiver, "@@iterated", 10, &items, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_get(host, receiver, "@@index", 7, &at, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (items.tag != KILN_VALUE_ARRAY) {
                return KILN_C99_CONTROL_ERROR;
            }
            array = kiln_c99_array_of(items);
            position = (size_t)kiln_c99_as_number(at);
            finished = position >= array->count;
            if (!finished) {
                status = kiln_c99_put(host, receiver, "@@index", 7,
                                      kiln_c99_number((double)(position + 1)), depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_new_object(&step);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_put(host, step, "value", 5,
                                  finished ? kiln_c99_undefined() : array->elements[position],
                                  depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_put(host, step, "done", 4, kiln_c99_boolean(finished), depth,
                                  thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            *result = step;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_HAS_OWN_PROPERTY: {
            kiln_value key = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_undefined();
            kiln_object *object;
            int found = 0;
            if (receiver.tag != KILN_VALUE_OBJECT || key.tag != KILN_VALUE_STRING) {
                return KILN_C99_CONTROL_ERROR;
            }
            object = kiln_c99_object_of(receiver);
            for (index = 0; index < object->count && !found; ++index) {
                found = kiln_c99_same_key(&object->properties[index], key.ptr, key.len);
            }
            *result = kiln_c99_boolean(found);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_TO_STRING: {
            /* `Object.prototype.toString`, the oldest type test there is -- and it reports the
             * *kind* rather than converting: `[object Array]`, not `1,2,3`. A library that has to
             * tell an array from an arguments object still reaches for it. */
            kiln_text built = {NULL, 0, 0};
            const char *tag;
            switch (receiver.tag) {
            case KILN_VALUE_UNDEFINED: tag = "Undefined"; break;
            case KILN_VALUE_NULL: tag = "Null"; break;
            case KILN_VALUE_BOOLEAN: tag = "Boolean"; break;
            case KILN_VALUE_NUMBER: tag = "Number"; break;
            case KILN_VALUE_STRING: tag = "String"; break;
            case KILN_VALUE_ARRAY: tag = "Array"; break;
            case KILN_VALUE_DATE: tag = "Date"; break;
            case KILN_VALUE_REGEXP: tag = "RegExp"; break;
            case KILN_VALUE_PROMISE: tag = "Promise"; break;
            case KILN_VALUE_SYMBOL: tag = "Symbol"; break;
            case KILN_VALUE_BIGINT: tag = "BigInt"; break;
            case KILN_VALUE_FUNCTION:
            case KILN_VALUE_BUILTIN:
            case KILN_VALUE_METHOD:
            case KILN_VALUE_BOUND:
            case KILN_VALUE_SETTLER: tag = "Function"; break;
            default: tag = "Object"; break;
            }
            if (kiln_c99_text_append_c(&built, "[object ") != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, tag) != KILN_C99_OK ||
                kiln_c99_text_append_c(&built, "]") != KILN_C99_OK) {
                free(built.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            *result = kiln_c99_value(KILN_VALUE_STRING, 0, built.bytes, built.len);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_VALUE_OF:
            /* Returns the receiver unchanged, which is the identity that makes ToPrimitive fall
             * through to `toString` for a plain object -- and the right answer for a primitive,
             * which is already what it converts to. */
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_PRIMITIVE_TO_STRING:
            *result = kiln_c99_text(receiver);
            return KILN_C99_OK;
        case KILN_C99_METHOD_DATE_PARSE: {
            /* Parsing accepts the ISO forms and rejects the rest, rather than guessing: a date
             * a program did not write is worse than no date at all. */
            kiln_value source = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_undefined();
            *result = kiln_c99_number(source.tag == KILN_VALUE_STRING
                                          ? kiln_c99_parse_date(source.ptr, source.len)
                                          : 0.0 / 0.0);
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_DATE_UTC:
        case KILN_C99_METHOD_GET_TIME:
        case KILN_C99_METHOD_GET_FULL_YEAR:
        case KILN_C99_METHOD_GET_MONTH:
        case KILN_C99_METHOD_GET_DATE:
        case KILN_C99_METHOD_GET_DAY:
        case KILN_C99_METHOD_GET_HOURS:
        case KILN_C99_METHOD_GET_MINUTES:
        case KILN_C99_METHOD_GET_SECONDS:
        case KILN_C99_METHOD_GET_MILLISECONDS:
        case KILN_C99_METHOD_GET_TIMEZONE_OFFSET:
        case KILN_C99_METHOD_TO_ISO_STRING:
        case KILN_C99_METHOD_TO_JSON:
        case KILN_C99_METHOD_DATE_TO_STRING: {
            kiln_date_parts parts;
            double time;
            if (code == KILN_C99_METHOD_DATE_UTC) {
                double fields[7];
                for (index = 0; index < 7; ++index) {
                    /* Day defaults to 1 and everything else to 0, which is what makes
                     * `Date.UTC(2026, 7)` the first of August. */
                    fields[index] = index < argc  ? kiln_c99_as_number(args[index])
                                    : index == 2 ? 1.0
                                                 : 0.0;
                }
                *result = kiln_c99_number(kiln_c99_instant_from_parts(fields));
                return KILN_C99_OK;
            }
            if (receiver.tag != KILN_VALUE_DATE) {
                return KILN_C99_CONTROL_ERROR;
            }
            time = kiln_c99_date_time(receiver);
            if (code == KILN_C99_METHOD_GET_TIME) {
                *result = kiln_c99_number(time);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_GET_TIMEZONE_OFFSET) {
                /* Zero, honestly reported rather than left to be discovered: the realm is UTC. */
                *result = kiln_c99_number(0.0);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_TO_ISO_STRING || code == KILN_C99_METHOD_TO_JSON) {
                kiln_value iso;
                status = kiln_c99_iso_string(&iso, time);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (iso.tag == KILN_VALUE_STRING) {
                    *result = iso;
                    return KILN_C99_OK;
                }
                /* An instant with no ISO spelling makes `toISOString` throw where `toJSON`
                 * answers null, so the enclosing document still serialises. */
                if (code == KILN_C99_METHOD_TO_JSON) {
                    *result = kiln_c99_null();
                    return KILN_C99_OK;
                }
                return KILN_C99_CONTROL_ERROR;
            }
            if (code == KILN_C99_METHOD_DATE_TO_STRING) {
                return kiln_c99_date_to_string(result, time);
            }
            parts = kiln_c99_date_parts(time);
            if (!parts.valid) {
                *result = kiln_c99_number(0.0 / 0.0);
                return KILN_C99_OK;
            }
            switch (code) {
            case KILN_C99_METHOD_GET_FULL_YEAR:
                *result = kiln_c99_number((double)parts.year);
                break;
            /* Zero-based, alone among the readers, which is the language's oldest wart. */
            case KILN_C99_METHOD_GET_MONTH:
                *result = kiln_c99_number((double)(parts.month - 1));
                break;
            case KILN_C99_METHOD_GET_DATE: *result = kiln_c99_number((double)parts.day); break;
            case KILN_C99_METHOD_GET_DAY: *result = kiln_c99_number((double)parts.weekday); break;
            case KILN_C99_METHOD_GET_HOURS: *result = kiln_c99_number((double)parts.hour); break;
            case KILN_C99_METHOD_GET_MINUTES:
                *result = kiln_c99_number((double)parts.minute);
                break;
            case KILN_C99_METHOD_GET_SECONDS:
                *result = kiln_c99_number((double)parts.second);
                break;
            default: *result = kiln_c99_number((double)parts.millisecond); break;
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ITERATOR_SELF:
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_ARRAY_VALUES: {
            /* Over a snapshot, so a `for...of` that appends to the array it is walking
             * terminates -- which is what the length being read once means. */
            kiln_value snapshot;
            kiln_array *array;
            if (receiver.tag != KILN_VALUE_ARRAY) {
                return KILN_C99_CONTROL_ERROR;
            }
            array = kiln_c99_array_of(receiver);
            status = kiln_c99_new_array(&snapshot);
            for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
                /* A hole reads as undefined here: `for...of` visits every position, unlike
                 * `forEach`, which skips one. */
                status = kiln_c99_array_push(snapshot,
                                             kiln_c99_is_hole(array->elements[index])
                                                 ? kiln_c99_undefined()
                                                 : array->elements[index]);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_new_iterator(result, snapshot);
        }
        case KILN_C99_METHOD_GENERATOR_SELF:
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                return KILN_C99_CONTROL_ERROR;
            }
            *result = receiver;
            return KILN_C99_OK;
        case KILN_C99_METHOD_GENERATOR_NEXT:
        case KILN_C99_METHOD_GENERATOR_RETURN: {
            kiln_generator *frame;
            kiln_closure *closure;
            kiln_value produced = kiln_c99_undefined();
            int finished = 1;
            if (receiver.tag != KILN_VALUE_GENERATOR) {
                return KILN_C99_CONTROL_ERROR;
            }
            frame = kiln_c99_generator_of(receiver);
            if (frame->async_generator && code == KILN_C99_METHOD_GENERATOR_NEXT) {
                /* Each step gets a promise of its own, because the body can `await` any number of
                 * times before it reaches the `yield` that finishes the step. */
                status = kiln_c99_new_promise(&frame->promise);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = frame->promise;
                return kiln_c99_async_generator_step(
                    host, receiver, argc > 0 ? args[0] : kiln_c99_undefined(), 0, depth, thrown);
            }
            /* `.return(v)` finishes the generator without resuming it, so the body never runs
             * again and the value is the caller's rather than the body's. */
            if (code == KILN_C99_METHOD_GENERATOR_RETURN) {
                frame->done = 1;
                produced = argc > 0 ? args[0] : kiln_c99_undefined();
            } else if (!frame->done) {
                frame->sent = argc > 0 ? args[0] : kiln_c99_undefined();
                closure = kiln_c99_closure_of(frame->function);
                status = closure->body(host, &produced, closure->upvalues, frame->receiver, NULL,
                                       0, depth + 1, thrown, frame);
                if (status == KILN_C99_YIELDED) {
                    frame->started = 1;
                    finished = 0;
                } else if (status != KILN_C99_OK) {
                    frame->done = 1;
                    return status;
                } else {
                    frame->done = 1;
                }
            }
            {
                /* A step is an object, not a pair: `{ value, done }` is what the iteration
                 * protocol is written against. */
                kiln_value step;
                status = kiln_c99_new_object(&step);
                if (status != KILN_C99_OK) {
                    return status;
                }
                status = kiln_c99_put(host, step, "value", 5, produced, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                status = kiln_c99_put(host, step, "done", 4,
                                      kiln_c99_boolean(finished || frame->done), depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                *result = step;
                return KILN_C99_OK;
            }
        }
        case KILN_C99_METHOD_JSON_STRINGIFY: {
            kiln_text text = {NULL, 0, 0};
            kiln_text indent = {NULL, 0, 0};
            int wrote;
            /* A replacer is a function or an array of keys, and honouring half of it would give
             * a document missing exactly what the program asked to control. */
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED &&
                args[1].tag != KILN_VALUE_NULL) {
                return KILN_C99_CONTROL_ERROR;
            }
            if (argc > 2) {
                if (args[2].tag == KILN_VALUE_NUMBER) {
                    /* A number is that many spaces, capped at ten, which is what the
                     * specification says and what stops a silly argument from allocating. */
                    double count = kiln_c99_as_number(args[2]);
                    size_t spaces = !(count > 0.0) ? 0 : count > 10.0 ? 10 : (size_t)count;
                    while (spaces-- > 0) {
                        if (kiln_c99_text_append_c(&indent, " ") != KILN_C99_OK) {
                            free(indent.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    }
                } else if (args[2].tag == KILN_VALUE_STRING) {
                    size_t len = args[2].len > 10 ? 10 : args[2].len;
                    if (kiln_c99_text_append(&indent, args[2].ptr, len) != KILN_C99_OK) {
                        free(indent.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
            }
            status = kiln_c99_json_value(host, &text, argc > 0 ? args[0] : kiln_c99_undefined(),
                                         indent.bytes, indent.len, "", 0, &wrote, depth, thrown);
            free(indent.bytes);
            if (status != KILN_C99_OK) {
                free(text.bytes);
                return status;
            }
            /* `JSON.stringify(undefined)` is `undefined`, not the string. */
            if (wrote) {
                *result = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
            } else {
                free(text.bytes);
                *result = kiln_c99_undefined();
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_JSON_PARSE: {
            kiln_value source = argc > 0 ? kiln_c99_text(args[0]) : kiln_c99_undefined();
            kiln_json_reader reader;
            kiln_value parsed;
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
                /* A reviver rewrites the tree bottom-up; ignoring it would hand back a document
                 * the program never asked for. */
                return KILN_C99_CONTROL_ERROR;
            }
            if (source.tag != KILN_VALUE_STRING) {
                return KILN_C99_CONTROL_ERROR;
            }
            reader.bytes = source.ptr;
            reader.len = source.len;
            reader.at = 0;
            reader.depth = 0;
            status = kiln_c99_json_read(&reader, &parsed);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* Trailing text is a syntax error, not something to ignore: `JSON.parse("1 2")` has
             * no answer. */
            kiln_c99_json_space(&reader);
            if (reader.at != reader.len) {
                return KILN_C99_CONTROL_ERROR;
            }
            *result = parsed;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (subject.tag == KILN_VALUE_OBJECT) {
                kiln_value linked = kiln_c99_object_of(subject)->prototype;
                /* Undefined and null are different answers: one means nothing was recorded, and
                 * an object with nothing recorded still inherits `Object.prototype`. The other
                 * means nothing was wanted, which is what `Object.create(null)` asks for. */
                *result = linked.tag == KILN_VALUE_UNDEFINED
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE)
                              : linked;
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_ARRAY) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_STRING) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_STRING_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_FUNCTION || subject.tag == KILN_VALUE_METHOD ||
                subject.tag == KILN_VALUE_BOUND) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_NUMBER) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_REGEXP) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_DATE) {
                *result = kiln_c99_builtin(KILN_C99_BUILTIN_DATE_PROTOTYPE);
                return KILN_C99_OK;
            }
            if (subject.tag == KILN_VALUE_BUILTIN) {
                /* A prototype table's own prototype is `Object.prototype`, which is where the
                 * chain from any of them ends. */
                *result = kiln_c99_is_namespace(subject.bits) &&
                                  subject.bits != KILN_C99_BUILTIN_OBJECT_PROTOTYPE
                              ? kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE)
                          : subject.bits == KILN_C99_BUILTIN_OBJECT_PROTOTYPE
                              ? kiln_c99_null()
                              : kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION_PROTOTYPE);
                return KILN_C99_OK;
            }
            return KILN_C99_CONTROL_ERROR;
        }
        case KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (subject.tag != KILN_VALUE_OBJECT) {
                return KILN_C99_CONTROL_ERROR;
            }
            status = kiln_c99_set_prototype(subject, argc > 1 ? args[1] : kiln_c99_undefined());
            /* The object, so a `setPrototypeOf` can be inlined into the expression that built
             * it. */
            *result = subject;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_GET_OWN_PROPERTY_NAMES: {
            /* Every own string key, enumerable or not -- which is the whole difference from
             * `Object.keys`, and why a function's assigned properties show up here. */
            kiln_object *object;
            kiln_value built;
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            if (subject.tag == KILN_VALUE_FUNCTION) {
                subject = kiln_c99_closure_of(subject)->properties;
            }
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (subject.tag != KILN_VALUE_OBJECT) {
                *result = built;
                return KILN_C99_OK;
            }
            object = kiln_c99_object_of(subject);
            for (index = 0; index < object->count && status == KILN_C99_OK; ++index) {
                /* A `@@` key is the machine's own and is not a property of the program's object,
                 * however un-enumerable the rest of them are. */
                if (object->properties[index].key_len >= 2 &&
                    object->properties[index].key[0] == '@' &&
                    object->properties[index].key[1] == '@') {
                    continue;
                }
                status = kiln_c99_array_push(built,
                                             kiln_c99_copy_text(object->properties[index].key,
                                                                object->properties[index].key_len));
            }
            *result = built;
            return status;
        }
        case KILN_C99_METHOD_OBJECT_FROM_ENTRIES: {
            kiln_value built;
            kiln_array *pairs;
            if (argc == 0 || args[0].tag != KILN_VALUE_ARRAY) {
                return KILN_C99_CONTROL_ERROR;
            }
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            pairs = kiln_c99_array_of(args[0]);
            for (index = 0; index < pairs->count; ++index) {
                kiln_array *pair;
                if (pairs->elements[index].tag != KILN_VALUE_ARRAY) {
                    return KILN_C99_CONTROL_ERROR;
                }
                pair = kiln_c99_array_of(pairs->elements[index]);
                status = kiln_c99_set_computed(
                    host, built, pair->count > 0 ? pair->elements[0] : kiln_c99_undefined(),
                    pair->count > 1 ? pair->elements[1] : kiln_c99_undefined(), depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_ARRAY_FROM: {
            /* Anything iterable, then the optional mapper -- which is one walk rather than a
             * `map` afterwards, because `Array.from({length: 3}, f)` has nothing to map. */
            kiln_value built;
            kiln_value mapper = argc > 1 ? args[1] : kiln_c99_undefined();
            kiln_array *collected;
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_iterate(host, built, argc > 0 ? args[0] : kiln_c99_undefined(),
                                      depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (!kiln_c99_is_callable(mapper)) {
                *result = built;
                return KILN_C99_OK;
            }
            collected = kiln_c99_array_of(built);
            for (index = 0; index < collected->count; ++index) {
                kiln_value visit[2];
                kiln_value produced;
                visit[0] = collected->elements[index];
                visit[1] = kiln_c99_number((double)index);
                status = kiln_c99_call(host, &produced, mapper, kiln_c99_undefined(), visit, 2,
                                       depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                collected->elements[index] = produced;
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_PROPERTY_IS_ENUMERABLE: {
            /* Own *and* enumerable, which is the pair `Object.keys` reports and `hasOwnProperty`
             * only half of. */
            kiln_value name = kiln_c99_property_key(argc > 0 ? args[0] : kiln_c99_undefined());
            kiln_object *object;
            size_t slot;
            *result = kiln_c99_boolean(0);
            if (receiver.tag != KILN_VALUE_OBJECT || name.tag != KILN_VALUE_STRING) {
                return KILN_C99_OK;
            }
            object = kiln_c99_object_of(receiver);
            for (slot = 0; slot < object->count; ++slot) {
                if (kiln_c99_same_key(&object->properties[slot], name.ptr, name.len)) {
                    *result = kiln_c99_boolean(object->properties[slot].enumerable);
                    break;
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_IS_PROTOTYPE_OF: {
            /* Walks the chain rather than comparing one link, which is what makes it different
             * from `Object.getPrototypeOf(x) === this`. */
            kiln_value walked = argc > 0 ? args[0] : kiln_c99_undefined();
            size_t steps;
            *result = kiln_c99_boolean(0);
            for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
                if (walked.tag != KILN_VALUE_OBJECT) {
                    break;
                }
                walked = kiln_c99_object_of(walked)->prototype;
                if (kiln_c99_strict_equal(walked, receiver)) {
                    *result = kiln_c99_boolean(1);
                    break;
                }
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_FREEZE:
        case KILN_C99_METHOD_OBJECT_IS_FROZEN: {
            kiln_value subject = argc > 0 ? args[0] : kiln_c99_undefined();
            int *flag = NULL;
            if (subject.tag == KILN_VALUE_OBJECT) {
                flag = &kiln_c99_object_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_ARRAY) {
                /* An array is not a `kiln_object` here, so freezing one has to reach the flag on
                 * the array itself -- and its elements are not properties of a store, which is
                 * why they need their own check rather than inheriting the object one. */
                flag = &kiln_c99_array_of(subject)->frozen;
            } else if (subject.tag == KILN_VALUE_FUNCTION) {
                /* A function's own properties live in an object it makes on first use. Freezing
                 * the function is freezing that store, so it has to exist first. */
                kiln_closure *closure = kiln_c99_closure_of(subject);
                if (closure->properties.tag != KILN_VALUE_OBJECT) {
                    int status = kiln_c99_new_object(&closure->properties);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
                flag = &kiln_c99_object_of(closure->properties)->frozen;
            }
            if (flag == NULL) {
                /* A primitive has nothing to freeze and is already as frozen as it gets. */
                *result = code == KILN_C99_METHOD_OBJECT_FREEZE ? subject : kiln_c99_boolean(1);
                return KILN_C99_OK;
            }
            if (code == KILN_C99_METHOD_OBJECT_FREEZE) {
                *flag = 1;
                /* The object itself, so `Object.freeze(o) === o` and a freeze can be inlined into
                 * the expression that built it. */
                *result = subject;
            } else {
                *result = kiln_c99_boolean(*flag);
            }
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_OBJECT_CREATE: {
            kiln_value built;
            /* The descriptor argument can carry accessors, which are functions, so it is
             * refused rather than half-honoured -- silently ignoring it would give an object
             * missing every property the program asked for. */
            if (argc > 1 && args[1].tag != KILN_VALUE_UNDEFINED) {
                return KILN_C99_CONTROL_ERROR;
            }
            status = kiln_c99_new_object(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
            /* `Object.create(null)` records the absence, which is not the same as recording
             * nothing: an object with no link at all inherits `Object.prototype`. */
            if (argc > 0 && (args[0].tag == KILN_VALUE_OBJECT || args[0].tag == KILN_VALUE_NULL)) {
                kiln_c99_object_of(built)->prototype = args[0];
            } else if (argc > 0) {
                return KILN_C99_CONTROL_ERROR;
            }
            *result = built;
            return KILN_C99_OK;
        }
        case KILN_C99_METHOD_HAS_OWN: {
            kiln_value key = argc > 1 ? kiln_c99_text(args[1]) : kiln_c99_undefined();
            kiln_object *object;
            int found = 0;
            if (argc == 0 || args[0].tag != KILN_VALUE_OBJECT || key.tag != KILN_VALUE_STRING) {
                return KILN_C99_CONTROL_ERROR;
            }
            object = kiln_c99_object_of(args[0]);
            for (index = 0; index < object->count && !found; ++index) {
                found = kiln_c99_same_key(&object->properties[index], key.ptr, key.len);
            }
            *result = kiln_c99_boolean(found);
            return KILN_C99_OK;
        }
        default:
            return KILN_C99_CONTROL_ERROR;
        }
    }
    if ((code >= KILN_C99_METHOD_STRING_SLICE && code < KILN_C99_METHOD_ARRAY_AT) ||
        code >= KILN_C99_METHOD_STRING_CODE_POINT_AT) {
        if (receiver.tag != KILN_VALUE_STRING) {
            return KILN_C99_CONTROL_ERROR;
        }
        return kiln_c99_invoke_string_method(result, code, receiver, args, argc);
    }
    if (receiver.tag != KILN_VALUE_ARRAY) {
        /* A method detached from its array has no receiver to work on, which is a TypeError in
         * the language for the same reason. */
        return KILN_C99_CONTROL_ERROR;
    }
    array = kiln_c99_array_of(receiver);
    if (array->frozen && kiln_c99_changes_length(code)) {
        /* A TypeError rather than a silent no-op: an element write has an answer to give without
         * doing anything -- the value it was handed -- and `push` does not. Its answer is the new
         * length, and there is no honest one. */
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot add or remove an element of a frozen array");
    }
    switch (code) {
    case KILN_C99_METHOD_PUSH:
        for (index = 0; index < argc; ++index) {
            status = kiln_c99_array_push(receiver, args[index]);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = kiln_c99_number((double)array->count);
        return KILN_C99_OK;
    case KILN_C99_METHOD_POP:
        if (array->count == 0) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        array->count -= 1;
        *result = array->elements[array->count];
        return KILN_C99_OK;
    case KILN_C99_METHOD_JOIN: {
        kiln_text joined = {NULL, 0, 0};
        kiln_value separator = argc > 0 ? args[0] : kiln_c99_undefined();
        const char *bytes = ",";
        size_t len = 1;
        if (separator.tag != KILN_VALUE_UNDEFINED) {
            kiln_value shown = kiln_c99_text(separator);
            if (shown.tag != KILN_VALUE_STRING) {
                return KILN_C99_CONTROL_ERROR;
            }
            bytes = shown.ptr;
            len = shown.len;
        }
        for (index = 0; index < array->count; ++index) {
            kiln_value element = array->elements[index];
            if (index != 0 && kiln_c99_text_append(&joined, bytes, len) != KILN_C99_OK) {
                free(joined.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            /* A nullish element, and a hole, contribute nothing -- which is what makes
             * `[1, , 3].join()` produce "1,,3" rather than "1,undefined,3". */
            if (element.tag == KILN_VALUE_UNDEFINED || element.tag == KILN_VALUE_NULL ||
                kiln_c99_is_hole(element)) {
                continue;
            }
            {
                kiln_value shown = kiln_c99_text(element);
                if (shown.tag != KILN_VALUE_STRING ||
                    kiln_c99_text_append(&joined, shown.ptr, shown.len) != KILN_C99_OK) {
                    free(joined.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
            }
        }
        *result = kiln_c99_value(KILN_VALUE_STRING, 0, joined.bytes, joined.len);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_INDEX_OF:
    case KILN_C99_METHOD_INCLUDES: {
        kiln_value needle = argc > 0 ? args[0] : kiln_c99_undefined();
        int found = 0;
        for (index = 0; index < array->count; ++index) {
            if (kiln_c99_is_hole(array->elements[index])) {
                /* The only place `indexOf` and `includes` disagree, and the reason `includes`
                 * exists: one skips a hole and the other reads it as undefined. */
                found = code == KILN_C99_METHOD_INCLUDES && needle.tag == KILN_VALUE_UNDEFINED;
                if (found) {
                    break;
                }
                continue;
            }
            found = code == KILN_C99_METHOD_INCLUDES
                        ? kiln_c99_same_value_zero(array->elements[index], needle)
                        : kiln_c99_strict_equal(array->elements[index], needle);
            if (found) {
                break;
            }
        }
        if (code == KILN_C99_METHOD_INCLUDES) {
            *result = kiln_c99_boolean(found);
        } else {
            *result = kiln_c99_number(found ? (double)index : -1.0);
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_SLICE: {
        size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                             array->count, array->count);
        kiln_value built;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = start; index < end && index < array->count; ++index) {
            status = kiln_c99_array_push(built, array->elements[index]);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_CONCAT: {
        kiln_value built;
        size_t argument;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count; ++index) {
            status = kiln_c99_array_push(built, array->elements[index]);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        for (argument = 0; argument < argc; ++argument) {
            /* An array argument contributes its elements, one level deep; anything else
             * contributes itself. */
            if (args[argument].tag == KILN_VALUE_ARRAY) {
                kiln_array *other = kiln_c99_array_of(args[argument]);
                for (index = 0; index < other->count; ++index) {
                    status = kiln_c99_array_push(built, other->elements[index]);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            } else {
                status = kiln_c99_array_push(built, args[argument]);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_TO_STRING:
        /* `join` with the separator fixed rather than defaulted -- a program can pass one to
         * `join` and cannot to `toString`, which is why `String([1, 2])` is `"1,2"`. */
        return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_JOIN, receiver, NULL, 0, depth,
                                      thrown);
    case KILN_C99_METHOD_ARRAY_FIND_LAST:
    case KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        size_t at = array->count;
        *result = code == KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX ? kiln_c99_number(-1.0)
                                                                : kiln_c99_undefined();
        while (at > 0) {
            kiln_value visit[3];
            kiln_value produced;
            at -= 1;
            if (at >= array->count) {
                /* The callback shortened the array out from under the walk. */
                continue;
            }
            /* A hole is visited as undefined here, unlike `find`: the language added the
             * last-first pair after it stopped skipping them. */
            visit[0] = kiln_c99_is_hole(array->elements[at]) ? kiln_c99_undefined()
                                                             : array->elements[at];
            visit[1] = kiln_c99_number((double)at);
            visit[2] = receiver;
            status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (kiln_c99_truthy(produced)) {
                *result = code == KILN_C99_METHOD_ARRAY_FIND_LAST_INDEX
                              ? kiln_c99_number((double)at)
                              : visit[0];
                break;
            }
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_SPLICE: {
        kiln_value removed;
        size_t start = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t removing;
        size_t inserting = argc > 2 ? argc - 2 : 0;
        size_t tail;
        /* A missing count removes everything from `start`, where an explicit `undefined` removes
         * nothing: `splice(1)` and `splice(1, undefined)` are different calls. */
        if (argc < 2) {
            removing = array->count - start;
        } else {
            double wanted = kiln_c99_as_number(args[1]);
            removing = isnan(wanted) || wanted < 0.0        ? 0
                       : wanted > (double)(array->count - start) ? array->count - start
                                                                 : (size_t)wanted;
        }
        status = kiln_c99_new_array(&removed);
        for (index = 0; index < removing && status == KILN_C99_OK; ++index) {
            status = kiln_c99_array_push(removed, array->elements[start + index]);
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_reserve((void **)&array->elements, &array->capacity,
                                  array->count - removing + inserting, sizeof *array->elements);
        if (status != KILN_C99_OK) {
            return status;
        }
        tail = array->count - start - removing;
        memmove(array->elements + start + inserting, array->elements + start + removing,
                tail * sizeof *array->elements);
        for (index = 0; index < inserting; ++index) {
            array->elements[start + index] = args[index + 2];
        }
        array->count = array->count - removing + inserting;
        /* The removed elements, which is what makes `splice` usable as a queue read. */
        *result = removed;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_COPY_WITHIN: {
        size_t target = kiln_c99_relative_index(argc > 0 ? args[0] : kiln_c99_undefined(),
                                                array->count, 0);
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 2 ? args[2] : kiln_c99_undefined(),
                                             array->count, array->count);
        size_t moving = end > start ? end - start : 0;
        if (moving > array->count - target) {
            moving = array->count - target;
        }
        /* `memmove` rather than a loop, because the source and the destination overlap by design
         * -- that overlap is the whole reason the method exists. */
        memmove(array->elements + target, array->elements + start,
                moving * sizeof *array->elements);
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_REDUCE_RIGHT: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value accumulator;
        size_t at = array->count;
        if (argc > 1) {
            accumulator = args[1];
        } else {
            /* The seed is the last element that is *there*: an array of nothing but holes has no
             * answer to give, exactly as an empty one does not. */
            while (at > 0 && kiln_c99_is_hole(array->elements[at - 1])) {
                at -= 1;
            }
            if (at == 0) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Reduce of empty array with no initial value");
            }
            at -= 1;
            accumulator = array->elements[at];
        }
        while (at > 0) {
            kiln_value visit[4];
            at -= 1;
            if (at >= array->count || kiln_c99_is_hole(array->elements[at])) {
                continue;
            }
            visit[0] = accumulator;
            visit[1] = array->elements[at];
            visit[2] = kiln_c99_number((double)at);
            visit[3] = receiver;
            status = kiln_c99_call(host, &accumulator, callback, kiln_c99_undefined(), visit, 4,
                                   depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = accumulator;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FLAT_MAP:
    case KILN_C99_METHOD_MAP:
    case KILN_C99_METHOD_FILTER:
    case KILN_C99_METHOD_FOR_EACH: {
        kiln_value built = kiln_c99_undefined();
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        if (code != KILN_C99_METHOD_FOR_EACH) {
            status = kiln_c99_new_array(&built);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        /* The length is read once, so an element the callback appends is not visited -- which is
         * what the specification says and what stops an appending callback from looping. */
        {
            size_t length = array->count;
            for (index = 0; index < length && index < array->count; ++index) {
                kiln_value visit[3];
                kiln_value produced;
                /* Every element walk skips a hole and visits an undefined. `map` still has to
                 * *leave* one: its result has an entry per input position, and skipping would
                 * shift everything after it. */
                if (kiln_c99_is_hole(array->elements[index])) {
                    if (code == KILN_C99_METHOD_MAP) {
                        status = kiln_c99_array_push(built, kiln_c99_hole());
                        if (status != KILN_C99_OK) {
                            return status;
                        }
                    }
                    continue;
                }
                visit[0] = array->elements[index];
                visit[1] = kiln_c99_number((double)index);
                visit[2] = receiver;
                status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (code == KILN_C99_METHOD_MAP || code == KILN_C99_METHOD_ARRAY_FLAT_MAP) {
                    status = kiln_c99_array_push(built, produced);
                } else if (code == KILN_C99_METHOD_FILTER && kiln_c99_truthy(produced)) {
                    status = kiln_c99_array_push(built, visit[0]);
                } else {
                    status = KILN_C99_OK;
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
        }
        if (code == KILN_C99_METHOD_ARRAY_FLAT_MAP) {
            /* One level, and only of what the callback produced -- `flatMap` does not flatten an
             * array that was already an element. */
            kiln_value one = kiln_c99_number(1.0);
            return kiln_c99_invoke_method(host, result, KILN_C99_METHOD_ARRAY_FLAT, built, &one, 1,
                                          depth, thrown);
        }
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_AT: {
        /* A negative index counts from the end, which is the whole reason `at` exists alongside
         * indexing; out of range is undefined rather than clamped. */
        double relative = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0;
        double position = relative < 0.0 ? (double)array->count + relative : relative;
        *result = position >= 0.0 && position < (double)array->count &&
                          !kiln_c99_is_hole(array->elements[(size_t)position])
                      ? array->elements[(size_t)position]
                      : kiln_c99_undefined();
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_LAST_INDEX_OF: {
        kiln_value needle = argc > 0 ? args[0] : kiln_c99_undefined();
        double found = -1.0;
        for (index = array->count; index > 0; --index) {
            if (kiln_c99_is_hole(array->elements[index - 1])) {
                continue;
            }
            if (kiln_c99_strict_equal(array->elements[index - 1], needle)) {
                found = (double)(index - 1);
                break;
            }
        }
        *result = kiln_c99_number(found);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FIND:
    case KILN_C99_METHOD_ARRAY_FIND_INDEX:
    case KILN_C99_METHOD_ARRAY_SOME:
    case KILN_C99_METHOD_ARRAY_EVERY: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value self_value = argc > 1 ? args[1] : kiln_c99_undefined();
        size_t length = array->count;
        int matched = 0;
        /* `every` is the only one that answers true for an empty array, and the only one that
         * stops on a *falsy* result. Everything else about the four is the same walk. */
        for (index = 0; index < length && index < array->count; ++index) {
            kiln_value visit[3];
            kiln_value produced;
            /* `find` and `findIndex` visit a hole as undefined, where `some` and `every` skip
             * it -- the same split `includes` and `indexOf` make. */
            if (kiln_c99_is_hole(array->elements[index])) {
                if (code == KILN_C99_METHOD_ARRAY_SOME || code == KILN_C99_METHOD_ARRAY_EVERY) {
                    continue;
                }
                visit[0] = kiln_c99_undefined();
            } else {
                visit[0] = array->elements[index];
            }
            visit[1] = kiln_c99_number((double)index);
            visit[2] = receiver;
            status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            matched = kiln_c99_truthy(produced);
            if (code == KILN_C99_METHOD_ARRAY_EVERY ? !matched : matched) {
                break;
            }
        }
        switch (code) {
        case KILN_C99_METHOD_ARRAY_FIND:
            *result = matched && index < array->count ? array->elements[index]
                                                      : kiln_c99_undefined();
            if (result->tag == KILN_VALUE_HOLE) {
                *result = kiln_c99_undefined();
            }
            break;
        case KILN_C99_METHOD_ARRAY_FIND_INDEX:
            *result = kiln_c99_number(matched ? (double)index : -1.0);
            break;
        case KILN_C99_METHOD_ARRAY_SOME: *result = kiln_c99_boolean(matched); break;
        default: *result = kiln_c99_boolean(index >= length || matched); break;
        }
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_SORT: {
        /* An insertion sort, which is stable -- and stability is program-visible, so it is part
         * of the answer rather than an implementation detail. The comparator can call back into
         * the program, so every comparison has to be able to fail. */
        kiln_value comparator = argc > 0 ? args[0] : kiln_c99_undefined();
        size_t sorted;
        /* Holes and undefined sort to the end, ahead of the comparator ever seeing them. */
        size_t live = 0;
        for (index = 0; index < array->count; ++index) {
            if (!kiln_c99_is_hole(array->elements[index]) &&
                array->elements[index].tag != KILN_VALUE_UNDEFINED) {
                array->elements[live++] = array->elements[index];
            }
        }
        for (index = live; index < array->count; ++index) {
            array->elements[index] = kiln_c99_undefined();
        }
        for (sorted = 1; sorted < live; ++sorted) {
            kiln_value moving = array->elements[sorted];
            size_t place = sorted;
            while (place > 0) {
                double order;
                if (kiln_c99_is_callable(comparator)) {
                    kiln_value pair[2];
                    kiln_value produced;
                    pair[0] = array->elements[place - 1];
                    pair[1] = moving;
                    status = kiln_c99_call(host, &produced, comparator, kiln_c99_undefined(), pair,
                                           2, depth, thrown);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                    order = kiln_c99_as_number(produced);
                    /* A comparator that answers NaN is treated as "equal", which keeps the sort
                     * total rather than letting it loop. */
                    if (isnan(order)) {
                        order = 0.0;
                    }
                } else {
                    /* Without a comparator the elements are compared *as text*, which is the
                     * language's oldest surprise: `[10, 9].sort()` is `[10, 9]`. */
                    kiln_value left = kiln_c99_text(array->elements[place - 1]);
                    kiln_value right = kiln_c99_text(moving);
                    if (left.tag != KILN_VALUE_STRING || right.tag != KILN_VALUE_STRING) {
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                    order = (double)kiln_c99_string_compare(left, right);
                }
                if (order <= 0.0) {
                    break;
                }
                array->elements[place] = array->elements[place - 1];
                place -= 1;
            }
            array->elements[place] = moving;
        }
        /* The array itself, so `a.sort()` can be chained and `a.sort() === a` holds. */
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_REVERSE: {
        size_t half;
        for (half = 0; half * 2 + 1 < array->count; ++half) {
            kiln_value swap = array->elements[half];
            array->elements[half] = array->elements[array->count - 1 - half];
            array->elements[array->count - 1 - half] = swap;
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_SHIFT: {
        if (array->count == 0) {
            *result = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        *result = kiln_c99_is_hole(array->elements[0]) ? kiln_c99_undefined()
                                                       : array->elements[0];
        memmove(array->elements, array->elements + 1,
                (array->count - 1) * sizeof *array->elements);
        array->count -= 1;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_UNSHIFT: {
        size_t added;
        status = kiln_c99_reserve((void **)&array->elements, &array->capacity,
                                  array->count + argc, sizeof *array->elements);
        if (status != KILN_C99_OK) {
            return status;
        }
        memmove(array->elements + argc, array->elements, array->count * sizeof *array->elements);
        for (added = 0; added < argc; ++added) {
            array->elements[added] = args[added];
        }
        array->count += argc;
        *result = kiln_c99_number((double)array->count);
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FILL: {
        kiln_value filler = argc > 0 ? args[0] : kiln_c99_undefined();
        size_t start = kiln_c99_relative_index(argc > 1 ? args[1] : kiln_c99_undefined(),
                                               array->count, 0);
        size_t end = kiln_c99_relative_index(argc > 2 ? args[2] : kiln_c99_undefined(),
                                             array->count, array->count);
        for (index = start; index < end; ++index) {
            array->elements[index] = filler;
        }
        *result = receiver;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_ARRAY_FLAT: {
        /* One level unless told otherwise, and a hole disappears rather than flattening to
         * undefined. */
        double depth_wanted = argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED
                                  ? kiln_c99_as_number(args[0])
                                  : 1.0;
        kiln_value built;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
            kiln_value element = array->elements[index];
            if (kiln_c99_is_hole(element)) {
                continue;
            }
            if (element.tag == KILN_VALUE_ARRAY && depth_wanted >= 1.0) {
                kiln_value flattened;
                kiln_value inner_depth = kiln_c99_number(depth_wanted - 1.0);
                kiln_array *nested;
                size_t inner;
                status = kiln_c99_invoke_method(host, &flattened, KILN_C99_METHOD_ARRAY_FLAT,
                                                element, &inner_depth, 1, depth + 1, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                nested = kiln_c99_array_of(flattened);
                for (inner = 0; inner < nested->count && status == KILN_C99_OK; ++inner) {
                    status = kiln_c99_array_push(built, nested->elements[inner]);
                }
                continue;
            }
            status = kiln_c99_array_push(built, element);
        }
        *result = built;
        return status;
    }
    case KILN_C99_METHOD_ARRAY_ENTRIES:
    case KILN_C99_METHOD_ARRAY_KEYS: {
        kiln_value snapshot;
        status = kiln_c99_new_array(&snapshot);
        for (index = 0; index < array->count && status == KILN_C99_OK; ++index) {
            kiln_value entry = kiln_c99_number((double)index);
            if (code == KILN_C99_METHOD_ARRAY_ENTRIES) {
                kiln_value pair;
                status = kiln_c99_new_array(&pair);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(pair, entry);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(pair, kiln_c99_is_hole(array->elements[index])
                                                           ? kiln_c99_undefined()
                                                           : array->elements[index]);
                }
                entry = pair;
            }
            if (status == KILN_C99_OK) {
                status = kiln_c99_array_push(snapshot, entry);
            }
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_new_iterator(result, snapshot);
    }
    case KILN_C99_METHOD_REDUCE: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value accumulator;
        size_t length = array->count;
        index = 0;
        if (argc > 1) {
            accumulator = args[1];
        } else {
            /* Without a seed the accumulator is the first element that is *there*: folding a
             * hole in would turn every sum over a sparse array into NaN. An array of nothing but
             * holes has no answer to give, exactly as an empty one does not. */
            while (index < length && kiln_c99_is_hole(array->elements[index])) {
                index += 1;
            }
            if (index >= length) {
                /* A TypeError the program can catch, not a refusal: `[].reduce(f)` is ordinary
                 * code with an ordinary error, and a fatal status would make it unwritable. */
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Reduce of empty array with no initial value");
            }
            accumulator = array->elements[index];
            index += 1;
        }
        for (; index < length && index < array->count; ++index) {
            kiln_value visit[4];
            if (kiln_c99_is_hole(array->elements[index])) {
                continue;
            }
            visit[0] = accumulator;
            visit[1] = array->elements[index];
            visit[2] = kiln_c99_number((double)index);
            visit[3] = receiver;
            status = kiln_c99_call(host, &accumulator, callback, kiln_c99_undefined(), visit, 4,
                                   depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        *result = accumulator;
        return KILN_C99_OK;
    }
    default:
        return KILN_C99_CONTROL_ERROR;
    }
}

/* Append everything `source` yields to `target`, which is what array spread, `for...of` and
 * destructuring all reduce to.
 *
 * An array and a string are walked directly rather than through their iterators. That is a
 * shortcut the specification does not take, and it is visible to a program that replaces
 * `Array.prototype[Symbol.iterator]` -- which this backend has no way to do, so the shortcut
 * cannot be observed here. Anything else must hand back an iterator and is driven by `next`. */
static int kiln_c99_iterate(kiln_host *host, kiln_value target, kiln_value source, int depth,
                            kiln_value *thrown) {
    kiln_value iterator;
    size_t index;
    int status;
    if (source.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(source);
        size_t length = array->count;
        for (index = 0; index < length && index < array->count; ++index) {
            /* Iteration *reads* each position, so a hole arrives as the undefined it reads as.
             * That is what makes `[...[1, , 3]]` a dense three-element array where `map` over
             * the same input leaves the hole alone. */
            status = kiln_c99_array_push(target, kiln_c99_is_hole(array->elements[index])
                                                     ? kiln_c99_undefined()
                                                     : array->elements[index]);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_COLLECTION) {
        /* A Map yields `[key, value]` pairs and a Set yields values, which is what makes
         * `for (const [k, v] of map)` and `for (const v of set)` both read naturally. */
        kiln_collection *collection = kiln_c99_collection_of(source);
        size_t length = collection->count;
        for (index = 0; index < length && index < collection->count; ++index) {
            kiln_value produced;
            if (collection->entries[index].dead) {
                continue;
            }
            produced = collection->entries[index].value;
            if (collection->is_map) {
                status = kiln_c99_new_array(&produced);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(produced, collection->entries[index].key);
                }
                if (status == KILN_C99_OK) {
                    status = kiln_c99_array_push(produced, collection->entries[index].value);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            status = kiln_c99_array_push(target, produced);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_STRING) {
        /* A string iterates as its *characters*, not its code units: an astral character is one
         * step, which is the difference between iterating a string and indexing it. */
        uint16_t *units = NULL;
        size_t count = 0;
        status = kiln_c99_to_units(source, &units, &count);
        if (status != KILN_C99_OK) {
            return status;
        }
        for (index = 0; index < count; ++index) {
            size_t width = (units[index] >= 0xD800 && units[index] <= 0xDBFF &&
                            index + 1 < count && units[index + 1] >= 0xDC00 &&
                            units[index + 1] <= 0xDFFF)
                               ? 2
                               : 1;
            status = kiln_c99_array_push(target, kiln_c99_from_units(units + index, width));
            if (status != KILN_C99_OK) {
                free(units);
                return status;
            }
            index += width - 1;
        }
        free(units);
        return KILN_C99_OK;
    }
    /* Everything else goes through the protocol: ask for an iterator, then drive it. */
    if (source.tag == KILN_VALUE_GENERATOR) {
        iterator = source;
    } else {
        kiln_value method;
        status = kiln_c99_get(host, source, "@@iterator", 10, &method, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!kiln_c99_is_callable(method)) {
            return KILN_C99_CONTROL_ERROR;
        }
        status = kiln_c99_call(host, &iterator, method, source, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    for (;;) {
        kiln_value next;
        kiln_value step;
        kiln_value done;
        kiln_value value;
        status = kiln_c99_get(host, iterator, "next", 4, &next, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!kiln_c99_is_callable(next)) {
            return KILN_C99_CONTROL_ERROR;
        }
        status = kiln_c99_call(host, &step, next, iterator, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_get(host, step, "done", 4, &done, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (kiln_c99_truthy(done)) {
            /* The value that comes back with `done` is the iterator's *return* value, which
             * spread and `for...of` both discard. */
            return KILN_C99_OK;
        }
        status = kiln_c99_get(host, step, "value", 5, &value, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        status = kiln_c99_array_push(target, value);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
}

/* `key in target`: whether the name is reachable, own or inherited. Unlike a read this does not
 * call an accessor -- the question is whether the property exists, not what it produces. */
static int kiln_c99_has_property(kiln_value target, const char *key, size_t key_len, int *answer) {
    kiln_value current = target;
    size_t steps;
    *answer = 0;
    if (target.tag == KILN_VALUE_ARRAY) {
        /* An index is present when it is in range, and `length` always is. */
        double index = kiln_c99_key_index(key, key_len);
        if (kiln_c99_same_name(key, key_len, "length")) {
            *answer = 1;
            return KILN_C99_OK;
        }
        if (index >= 0.0) {
            kiln_array *array = kiln_c99_array_of(target);
            *answer = (size_t)index < array->count && !kiln_c99_is_hole(array->elements[(size_t)index]);
            return KILN_C99_OK;
        }
        *answer = kiln_c99_array_method(key, key_len) != 0;
        return KILN_C99_OK;
    }
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        kiln_object *object;
        size_t index;
        if (current.tag != KILN_VALUE_OBJECT) {
            break;
        }
        object = kiln_c99_object_of(current);
        for (index = 0; index < object->count; ++index) {
            if (kiln_c99_same_key(&object->properties[index], key, key_len)) {
                *answer = 1;
                return KILN_C99_OK;
            }
        }
        current = object->prototype;
    }
    /* `in` has to agree with the read about what the end of the chain carries. */
    if (target.tag == KILN_VALUE_OBJECT &&
        kiln_c99_object_of(target)->prototype.tag != KILN_VALUE_NULL) {
        *answer = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_HAS_OWN_PROPERTY,
                                         KILN_C99_METHOD_OBJECT_VALUE_OF) != 0 ||
                  kiln_c99_same_name(key, key_len, "constructor");
    }
    return KILN_C99_OK;
}

/* `delete target[key]`: remove an own property, answering whether the name is gone afterwards.
 *
 * A name that was never there deletes successfully, which is what makes `delete` almost always
 * true and why the answer is about the *end state* rather than about work done. */
static int kiln_c99_delete(kiln_value target, const char *key, size_t key_len, int *answer) {
    kiln_object *object;
    size_t index;
    *answer = 1;
    if (target.tag == KILN_VALUE_STRING) {
        /* `delete "text".length` is false: the property is real and cannot be configured away.
         * Anything else on a string was never there, and deleting what is not there is true. */
        *answer = !kiln_c99_same_name(key, key_len, "length");
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_ARRAY) {
        /* Deleting an element leaves a *hole*, not an undefined and not a shorter array: the
         * slot reads as undefined, `length` is unchanged, and `in` says the element is gone. */
        double slot = kiln_c99_key_index(key, key_len);
        kiln_array *array = kiln_c99_array_of(target);
        if (array->frozen) {
            *answer = 0;
            return KILN_C99_OK;
        }
        if (slot >= 0.0 && (size_t)slot < array->count) {
            array->elements[(size_t)slot] = kiln_c99_hole();
        }
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_OK;
    }
    object = kiln_c99_object_of(target);
    if (object->frozen) {
        *answer = 0;
        return KILN_C99_OK;
    }
    for (index = 0; index < object->count; ++index) {
        if (!kiln_c99_same_key(&object->properties[index], key, key_len)) {
            continue;
        }
        /* Shifted rather than swapped with the last: property order is observable, and a
         * `delete` must not reorder what remains. */
        for (; index + 1 < object->count; ++index) {
            object->properties[index] = object->properties[index + 1];
        }
        object->count -= 1;
        return KILN_C99_OK;
    }
    return KILN_C99_OK;
}

/* An array of a value's own enumerable string keys, in enumeration order: what `for...in` walks
 * and what `Object.keys` reports. A snapshot, which is what the language specifies -- a key added
 * during the loop may or may not be visited, and this chooses not to. */
static int kiln_c99_own_keys(kiln_value target, kiln_value *out) {
    kiln_value built;
    size_t index;
    int status = kiln_c99_new_array(&built);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (target.tag == KILN_VALUE_OBJECT) {
        kiln_object *object = kiln_c99_object_of(target);
        for (index = 0; index < object->count; ++index) {
            if (!object->properties[index].enumerable) {
                continue;
            }
            status = kiln_c99_array_push(built, kiln_c99_copy_text(object->properties[index].key,
                                                                   object->properties[index].key_len));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    } else if (target.tag == KILN_VALUE_ARRAY) {
        /* An array's own enumerable keys are its indices, as strings: `for (const i in [7,8])`
         * gives "0" and "1", not 0 and 1. */
        kiln_array *array = kiln_c99_array_of(target);
        for (index = 0; index < array->count; ++index) {
            if (kiln_c99_is_hole(array->elements[index])) {
                continue;
            }
            status = kiln_c99_array_push(built, kiln_c99_text(kiln_c99_number((double)index)));
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    *out = built;
    return KILN_C99_OK;
}

/* `{ ...source }`: copy every own property of the source into the target.
 *
 * A spread copies *values*, which for an accessor means calling its getter and storing what it
 * produced -- the target gets a plain data property, not the accessor. A setter-only property
 * has nothing to read, so it contributes nothing. Spreading nullish contributes nothing rather
 * than throwing, which is the one place object spread is more forgiving than array spread. */
static int kiln_c99_spread_object(kiln_host *host, kiln_value target, kiln_value source,
                                  int depth, kiln_value *thrown) {
    kiln_object *object;
    size_t index;
    if (source.tag == KILN_VALUE_UNDEFINED || source.tag == KILN_VALUE_NULL) {
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT || source.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_CONTROL_ERROR;
    }
    object = kiln_c99_object_of(source);
    /* The count is read each time round because a getter can add a property to the source; the
     * bound is the source's *original* length, so a property the getter appends is not copied,
     * which is what stops an appending getter from looping. */
    {
        size_t length = object->count;
        for (index = 0; index < length && index < object->count; ++index) {
            kiln_value copied;
            int status;
            if (!object->properties[index].enumerable) {
                continue;
            }
            if (object->properties[index].accessor) {
                if (object->properties[index].getter.tag == KILN_VALUE_UNDEFINED) {
                    continue;
                }
                status = kiln_c99_call(host, &copied, object->properties[index].getter, source,
                                       NULL, 0, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
            } else {
                copied = object->properties[index].value;
            }
            status = kiln_c99_put(host, target, object->properties[index].key,
                                  object->properties[index].key_len, copied, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    return KILN_C99_OK;
}

/* Define an accessor half, merging with a half already defined under the same key.
 *
 * Merging matters because `get x()` and `set x()` are written separately and describe one
 * property; defining the second must not discard the first.
 *
 * `enumerable` separates the two syntaxes that reach here: an object literal's accessor is an
 * ordinary property and is enumerated, a class body's is not. */
static int kiln_c99_define_accessor(kiln_value target, const char *key, size_t key_len,
                                    kiln_value getter, kiln_value setter, int enumerable) {
    kiln_object *object;
    size_t index;
    int status;
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_value store;
        status = kiln_c99_function_store(target, &store);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_define_accessor(store, key, key_len, getter, setter, enumerable);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_CONTROL_ERROR;
    }
    object = kiln_c99_object_of(target);
    for (index = 0; index < object->count; ++index) {
        if (!kiln_c99_same_key(&object->properties[index], key, key_len)) {
            continue;
        }
        if (!object->properties[index].accessor) {
            object->properties[index].accessor = 1;
            object->properties[index].getter = kiln_c99_undefined();
            object->properties[index].setter = kiln_c99_undefined();
        }
        if (getter.tag != KILN_VALUE_UNDEFINED) {
            object->properties[index].getter = getter;
        }
        if (setter.tag != KILN_VALUE_UNDEFINED) {
            object->properties[index].setter = setter;
        }
        if (!enumerable) {
            object->properties[index].enumerable = 0;
        }
        return KILN_C99_OK;
    }
    status = kiln_c99_put(NULL, target, key, key_len, kiln_c99_undefined(), 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    for (index = 0; index < object->count; ++index) {
        if (kiln_c99_same_key(&object->properties[index], key, key_len)) {
            object->properties[index].accessor = 1;
            object->properties[index].getter = getter;
            object->properties[index].setter = setter;
            if (!enumerable) {
                object->properties[index].enumerable = 0;
            }
            return KILN_C99_OK;
        }
    }
    return KILN_C99_CONTROL_ERROR;
}

/* Define a method: a data property that is reachable by name and never enumerated.
 *
 * `kiln_c99_put` is an assignment and creates an enumerated property, which is right for an
 * object literal and wrong for a class body -- `for (const k in new C())` visits the fields the
 * constructor assigned and nothing the class declared. */
static int kiln_c99_define_method(kiln_value target, const char *key, size_t key_len,
                                  kiln_value method) {
    kiln_object *object;
    size_t index;
    int status;
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_value store;
        status = kiln_c99_function_store(target, &store);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_define_method(store, key, key_len, method);
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_CONTROL_ERROR;
    }
    status = kiln_c99_put(NULL, target, key, key_len, method, 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    object = kiln_c99_object_of(target);
    for (index = 0; index < object->count; ++index) {
        if (kiln_c99_same_key(&object->properties[index], key, key_len)) {
            object->properties[index].enumerable = 0;
            return KILN_C99_OK;
        }
    }
    return KILN_C99_CONTROL_ERROR;
}

/* JSON.
 *
 * Serialising can *call*: `toJSON` runs before anything else is decided about a value, and a
 * property may be an accessor. So this takes a host and a depth like every other read does. */
static int kiln_c99_json_quote(kiln_text *text, const char *bytes, size_t len) {
    static const char digits[] = "0123456789abcdef";
    size_t index;
    if (kiln_c99_text_append(text, "\"", 1) != KILN_C99_OK) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    for (index = 0; index < len; ++index) {
        char byte = bytes[index];
        const char *escape = NULL;
        char pair[2];
        char hex[6];
        switch (byte) {
        case '"': escape = "\\\""; break;
        case '\\': escape = "\\\\"; break;
        case '\n': escape = "\\n"; break;
        case '\r': escape = "\\r"; break;
        case '\t': escape = "\\t"; break;
        case '\b': escape = "\\b"; break;
        case '\f': escape = "\\f"; break;
        default: break;
        }
        if (escape != NULL) {
            if (kiln_c99_text_append_c(text, escape) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        /* Only the C0 controls are escaped as \u; everything else is emitted as its own UTF-8,
         * which is what JSON permits and what node produces. */
        if ((unsigned char)byte < 0x20) {
            hex[0] = '\\';
            hex[1] = 'u';
            hex[2] = '0';
            hex[3] = '0';
            hex[4] = digits[((unsigned char)byte >> 4) & 0xF];
            hex[5] = digits[(unsigned char)byte & 0xF];
            if (kiln_c99_text_append(text, hex, 6) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            continue;
        }
        pair[0] = byte;
        if (kiln_c99_text_append(text, pair, 1) != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    return kiln_c99_text_append(text, "\"", 1);
}

/* `serialised` is 0 when the value has no JSON spelling at all -- undefined, a function -- which
 * drops it from an object and turns it into null in an array. */
static int kiln_c99_json_value(kiln_host *host, kiln_text *text, kiln_value value,
                               const char *indent, size_t indent_len, const char *current,
                               size_t current_len, int *serialised, int depth,
                               kiln_value *thrown) {
    kiln_text nested = {NULL, 0, 0};
    kiln_value replaced;
    size_t index;
    int status;
    *serialised = 1;

    /* The `toJSON` step, before anything else is decided about the value.
     *
     * Only an object is asked. An array's named properties are not stored here at all, so there
     * is nowhere for one to have been put -- and asking would reach the array method table,
     * which reports an unknown name as an error rather than as absence. A Date is asked because
     * carrying one is the whole reason the step exists. */
    if (value.tag == KILN_VALUE_OBJECT || value.tag == KILN_VALUE_DATE) {
        kiln_value method;
        status = kiln_c99_get(host, value, "toJSON", 6, &method, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (kiln_c99_is_callable(method)) {
            status = kiln_c99_call(host, &replaced, method, value, NULL, 0, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            value = replaced;
        }
    }

    switch (value.tag) {
    case KILN_VALUE_UNDEFINED:
    case KILN_VALUE_HOLE:
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_BUILTIN:
    case KILN_VALUE_METHOD:
        *serialised = 0;
        return KILN_C99_OK;
    case KILN_VALUE_NULL:
        return kiln_c99_text_append_c(text, "null");
    case KILN_VALUE_BOOLEAN:
        return kiln_c99_text_append_c(text, value.bits ? "true" : "false");
    case KILN_VALUE_NUMBER: {
        /* NaN and both infinities have no JSON spelling, so they serialise as null. */
        double number = kiln_c99_as_number(value);
        kiln_value shown;
        if (!isfinite(number)) {
            return kiln_c99_text_append_c(text, "null");
        }
        shown = kiln_c99_text(value);
        if (shown.tag != KILN_VALUE_STRING) {
            return KILN_C99_CONTROL_ERROR;
        }
        return kiln_c99_text_append(text, shown.ptr, shown.len);
    }
    case KILN_VALUE_STRING:
        return kiln_c99_json_quote(text, value.ptr, value.len);
    default:
        break;
    }
    if (value.tag != KILN_VALUE_OBJECT && value.tag != KILN_VALUE_ARRAY) {
        *serialised = 0;
        return KILN_C99_OK;
    }

    /* With no indent every separator is empty, which collapses the pretty form to the compact
     * one and lets both share one code path. */
    if (indent_len != 0 &&
        (kiln_c99_text_append(&nested, current, current_len) != KILN_C99_OK ||
         kiln_c99_text_append(&nested, indent, indent_len) != KILN_C99_OK)) {
        free(nested.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }

#define KILN_JSON_OPEN(bracket)                                                                   \
    do {                                                                                          \
        if (kiln_c99_text_append_c(text, bracket) != KILN_C99_OK) {                                \
            free(nested.bytes);                                                                    \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
    } while (0)
#define KILN_JSON_BREAK(first)                                                                    \
    do {                                                                                          \
        if (!(first) && kiln_c99_text_append_c(text, ",") != KILN_C99_OK) {                        \
            free(nested.bytes);                                                                    \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
        if (indent_len != 0 &&                                                                     \
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||                                  \
             kiln_c99_text_append(text, nested.bytes, nested.len) != KILN_C99_OK)) {                \
            free(nested.bytes);                                                                    \
            return KILN_C99_OUT_OF_MEMORY;                                                          \
        }                                                                                          \
    } while (0)

    if (value.tag == KILN_VALUE_ARRAY) {
        kiln_array *array = kiln_c99_array_of(value);
        KILN_JSON_OPEN("[");
        for (index = 0; index < array->count; ++index) {
            int wrote;
            KILN_JSON_BREAK(index == 0);
            status = kiln_c99_json_value(host, text, array->elements[index], indent, indent_len,
                                         nested.bytes, nested.len, &wrote, depth, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                return status;
            }
            /* An element with no JSON spelling -- undefined, a function, a hole -- becomes null
             * rather than disappearing: an array's positions are part of its meaning. */
            if (!wrote && kiln_c99_text_append_c(text, "null") != KILN_C99_OK) {
                free(nested.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        if (indent_len != 0 && array->count != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            free(nested.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(nested.bytes);
        return kiln_c99_text_append_c(text, "]");
    }
    {
        kiln_object *object = kiln_c99_object_of(value);
        size_t written = 0;
        size_t length = object->count;
        KILN_JSON_OPEN("{");
        for (index = 0; index < length && index < object->count; ++index) {
            kiln_value member;
            kiln_text member_text = {NULL, 0, 0};
            int wrote;
            if (!object->properties[index].enumerable) {
                continue;
            }
            if (object->properties[index].accessor) {
                if (object->properties[index].getter.tag == KILN_VALUE_UNDEFINED) {
                    continue;
                }
                status = kiln_c99_call(host, &member, object->properties[index].getter, value,
                                       NULL, 0, depth, thrown);
                if (status != KILN_C99_OK) {
                    free(nested.bytes);
                    return status;
                }
            } else {
                member = object->properties[index].value;
            }
            /* Serialised into a buffer of its own first: a member with no JSON spelling is
             * *dropped*, so its separator must not have been written yet. */
            status = kiln_c99_json_value(host, &member_text, member, indent, indent_len,
                                         nested.bytes, nested.len, &wrote, depth, thrown);
            if (status != KILN_C99_OK) {
                free(nested.bytes);
                free(member_text.bytes);
                return status;
            }
            if (!wrote) {
                free(member_text.bytes);
                continue;
            }
            KILN_JSON_BREAK(written == 0);
            if (kiln_c99_json_quote(text, object->properties[index].key,
                                    object->properties[index].key_len) != KILN_C99_OK ||
                kiln_c99_text_append_c(text, indent_len != 0 ? ": " : ":") != KILN_C99_OK ||
                kiln_c99_text_append(text, member_text.bytes, member_text.len) != KILN_C99_OK) {
                free(nested.bytes);
                free(member_text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            free(member_text.bytes);
            written += 1;
        }
        if (indent_len != 0 && written != 0 &&
            (kiln_c99_text_append_c(text, "\n") != KILN_C99_OK ||
             kiln_c99_text_append(text, current, current_len) != KILN_C99_OK)) {
            free(nested.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
        free(nested.bytes);
        return kiln_c99_text_append_c(text, "}");
    }
#undef KILN_JSON_OPEN
#undef KILN_JSON_BREAK
}

/* A recursive-descent JSON parser over the source bytes.
 *
 * `at` is the cursor, moved forward as text is consumed. A malformed document is a control error
 * rather than a partial value: JSON.parse either produces the whole thing or nothing. */

static int kiln_c99_json_read(kiln_json_reader *reader, kiln_value *out);

static void kiln_c99_json_space(kiln_json_reader *reader) {
    while (reader->at < reader->len) {
        char byte = reader->bytes[reader->at];
        if (byte != ' ' && byte != '\t' && byte != '\n' && byte != '\r') {
            return;
        }
        reader->at += 1;
    }
}

static int kiln_c99_json_literal(kiln_json_reader *reader, const char *word) {
    size_t len = strlen(word);
    if (reader->len - reader->at < len || memcmp(reader->bytes + reader->at, word, len) != 0) {
        return 0;
    }
    reader->at += len;
    return 1;
}

static int kiln_c99_json_string(kiln_json_reader *reader, kiln_value *out) {
    kiln_text text = {NULL, 0, 0};
    uint16_t pending_high = 0;
    reader->at += 1;
    while (reader->at < reader->len) {
        char byte = reader->bytes[reader->at];
        if (byte == '"') {
            reader->at += 1;
            /* A high surrogate left unpaired at the end is written as the replacement
             * character, for the same reason as everywhere else: this string cannot hold one. */
            if (pending_high != 0 &&
                kiln_c99_text_append_c(&text, "\xEF\xBF\xBD") != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
            return KILN_C99_OK;
        }
        if (byte != '\\') {
            if (pending_high != 0) {
                if (kiln_c99_text_append_c(&text, "\xEF\xBF\xBD") != KILN_C99_OK) {
                    free(text.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                pending_high = 0;
            }
            if (kiln_c99_text_append(&text, &byte, 1) != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
            reader->at += 1;
            continue;
        }
        reader->at += 1;
        if (reader->at >= reader->len) {
            break;
        }
        {
            char escaped = reader->bytes[reader->at];
            const char *literal = NULL;
            reader->at += 1;
            switch (escaped) {
            case '"': literal = "\""; break;
            case '\\': literal = "\\"; break;
            case '/': literal = "/"; break;
            case 'n': literal = "\n"; break;
            case 't': literal = "\t"; break;
            case 'r': literal = "\r"; break;
            case 'b': literal = "\b"; break;
            case 'f': literal = "\f"; break;
            case 'u': {
                uint16_t unit = 0;
                uint16_t pair[2];
                size_t digit;
                if (reader->len - reader->at < 4) {
                    free(text.bytes);
                    return KILN_C99_CONTROL_ERROR;
                }
                for (digit = 0; digit < 4; ++digit) {
                    char hex = reader->bytes[reader->at + digit];
                    unit = (uint16_t)(unit << 4);
                    if (hex >= '0' && hex <= '9') {
                        unit = (uint16_t)(unit | (uint16_t)(hex - '0'));
                    } else if (hex >= 'a' && hex <= 'f') {
                        unit = (uint16_t)(unit | (uint16_t)(hex - 'a' + 10));
                    } else if (hex >= 'A' && hex <= 'F') {
                        unit = (uint16_t)(unit | (uint16_t)(hex - 'A' + 10));
                    } else {
                        free(text.bytes);
                        return KILN_C99_CONTROL_ERROR;
                    }
                }
                reader->at += 4;
                /* A surrogate pair is two escapes and one character, so the high half is held
                 * until the low half arrives. */
                if (unit >= 0xD800 && unit <= 0xDBFF) {
                    pair[0] = unit;
                    if (pending_high != 0) {
                        kiln_value lone = kiln_c99_from_units(&pending_high, 1);
                        if (kiln_c99_text_append(&text, lone.ptr, lone.len) != KILN_C99_OK) {
                            free(text.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    }
                    pending_high = pair[0];
                    continue;
                }
                if (pending_high != 0) {
                    pair[0] = pending_high;
                    pair[1] = unit;
                    pending_high = 0;
                    {
                        kiln_value combined = kiln_c99_from_units(pair, 2);
                        if (kiln_c99_text_append(&text, combined.ptr, combined.len) !=
                            KILN_C99_OK) {
                            free(text.bytes);
                            return KILN_C99_OUT_OF_MEMORY;
                        }
                    }
                    continue;
                }
                {
                    kiln_value single = kiln_c99_from_units(&unit, 1);
                    if (kiln_c99_text_append(&text, single.ptr, single.len) != KILN_C99_OK) {
                        free(text.bytes);
                        return KILN_C99_OUT_OF_MEMORY;
                    }
                }
                continue;
            }
            default:
                free(text.bytes);
                return KILN_C99_CONTROL_ERROR;
            }
            if (pending_high != 0) {
                if (kiln_c99_text_append_c(&text, "\xEF\xBF\xBD") != KILN_C99_OK) {
                    free(text.bytes);
                    return KILN_C99_OUT_OF_MEMORY;
                }
                pending_high = 0;
            }
            if (kiln_c99_text_append_c(&text, literal) != KILN_C99_OK) {
                free(text.bytes);
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
    }
    free(text.bytes);
    return KILN_C99_CONTROL_ERROR;
}

static int kiln_c99_json_read(kiln_json_reader *reader, kiln_value *out) {
    int status;
    if (reader->depth >= KILN_C99_MAX_CALL_DEPTH) {
        return KILN_C99_CALL_DEPTH_ERROR;
    }
    kiln_c99_json_space(reader);
    if (reader->at >= reader->len) {
        return KILN_C99_CONTROL_ERROR;
    }
    switch (reader->bytes[reader->at]) {
    case 'n':
        if (!kiln_c99_json_literal(reader, "null")) {
            return KILN_C99_CONTROL_ERROR;
        }
        *out = kiln_c99_null();
        return KILN_C99_OK;
    case 't':
        if (!kiln_c99_json_literal(reader, "true")) {
            return KILN_C99_CONTROL_ERROR;
        }
        *out = kiln_c99_boolean(1);
        return KILN_C99_OK;
    case 'f':
        if (!kiln_c99_json_literal(reader, "false")) {
            return KILN_C99_CONTROL_ERROR;
        }
        *out = kiln_c99_boolean(0);
        return KILN_C99_OK;
    case '"':
        return kiln_c99_json_string(reader, out);
    case '[': {
        kiln_value built;
        reader->at += 1;
        status = kiln_c99_new_array(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_json_space(reader);
        if (reader->at < reader->len && reader->bytes[reader->at] == ']') {
            reader->at += 1;
            *out = built;
            return KILN_C99_OK;
        }
        for (;;) {
            kiln_value element;
            reader->depth += 1;
            status = kiln_c99_json_read(reader, &element);
            reader->depth -= 1;
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_array_push(built, element);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len) {
                return KILN_C99_CONTROL_ERROR;
            }
            if (reader->bytes[reader->at] == ',') {
                reader->at += 1;
                continue;
            }
            if (reader->bytes[reader->at] == ']') {
                reader->at += 1;
                *out = built;
                return KILN_C99_OK;
            }
            return KILN_C99_CONTROL_ERROR;
        }
    }
    case '{': {
        kiln_value built;
        reader->at += 1;
        status = kiln_c99_new_object(&built);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_json_space(reader);
        if (reader->at < reader->len && reader->bytes[reader->at] == '}') {
            reader->at += 1;
            *out = built;
            return KILN_C99_OK;
        }
        for (;;) {
            kiln_value key;
            kiln_value member;
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len || reader->bytes[reader->at] != '"') {
                return KILN_C99_CONTROL_ERROR;
            }
            status = kiln_c99_json_string(reader, &key);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len || reader->bytes[reader->at] != ':') {
                return KILN_C99_CONTROL_ERROR;
            }
            reader->at += 1;
            reader->depth += 1;
            status = kiln_c99_json_read(reader, &member);
            reader->depth -= 1;
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_put(NULL, built, key.ptr, key.len, member, 0, NULL);
            if (status != KILN_C99_OK) {
                return status;
            }
            kiln_c99_json_space(reader);
            if (reader->at >= reader->len) {
                return KILN_C99_CONTROL_ERROR;
            }
            if (reader->bytes[reader->at] == ',') {
                reader->at += 1;
                continue;
            }
            if (reader->bytes[reader->at] == '}') {
                reader->at += 1;
                *out = built;
                return KILN_C99_OK;
            }
            return KILN_C99_CONTROL_ERROR;
        }
    }
    default: {
        /* A number. JSON's grammar is narrower than the language's, so the extent is measured
         * here and the digits themselves are handed to the shared parser -- one place that knows
         * how a decimal becomes a double. */
        size_t start = reader->at;
        if (reader->bytes[reader->at] == '-') {
            reader->at += 1;
        }
        while (reader->at < reader->len) {
            char byte = reader->bytes[reader->at];
            if ((byte >= '0' && byte <= '9') || byte == '.' || byte == 'e' || byte == 'E' ||
                byte == '+' || byte == '-') {
                reader->at += 1;
                continue;
            }
            break;
        }
        if (reader->at == start) {
            return KILN_C99_CONTROL_ERROR;
        }
        {
            double number = kiln_string_to_number(reader->bytes + start, reader->at - start);
            if (isnan(number)) {
                return KILN_C99_CONTROL_ERROR;
            }
            *out = kiln_c99_number(number);
            return KILN_C99_OK;
        }
    }
    }
}

/* Days-to-civil and back: Howard Hinnant's algorithm, the same one the interpreter uses. The two
 * have to agree exactly, so this is a transcription rather than a reimplementation. */
static void kiln_c99_civil_from_days(int64_t days, int64_t *year, int *month, int *day) {
    int64_t shifted = days + 719468;
    int64_t era = (shifted >= 0 ? shifted : shifted - 146096) / 146097;
    int64_t day_of_era = shifted - era * 146097;
    int64_t year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    int64_t computed_year = year_of_era + era * 400;
    int64_t day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    int64_t shifted_month = (5 * day_of_year + 2) / 153;
    int computed_day = (int)(day_of_year - (153 * shifted_month + 2) / 5 + 1);
    int computed_month = (int)(shifted_month < 10 ? shifted_month + 3 : shifted_month - 9);
    *year = computed_year + (computed_month <= 2 ? 1 : 0);
    *month = computed_month;
    *day = computed_day;
}

static int64_t kiln_c99_days_from_civil(int64_t year, int64_t month, int64_t day) {
    int64_t era;
    int64_t year_of_era;
    int64_t shifted_month;
    int64_t day_of_year;
    int64_t day_of_era;
    year -= month <= 2 ? 1 : 0;
    era = (year >= 0 ? year : year - 399) / 400;
    year_of_era = year - era * 400;
    shifted_month = month > 2 ? month - 3 : month + 9;
    day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    return era * 146097 + day_of_era - 719468;
}

/* Floor division, not truncation: an instant before the epoch is a negative count and has to
 * round *down* to the day containing it. */
static int64_t kiln_c99_floor_div(int64_t value, int64_t divisor) {
    int64_t quotient = value / divisor;
    return (value % divisor != 0 && ((value < 0) != (divisor < 0))) ? quotient - 1 : quotient;
}

static int64_t kiln_c99_floor_mod(int64_t value, int64_t divisor) {
    int64_t remainder = value % divisor;
    return remainder != 0 && ((remainder < 0) != (divisor < 0)) ? remainder + divisor : remainder;
}

static kiln_date_parts kiln_c99_date_parts(double time) {
    kiln_date_parts parts;
    int64_t millis;
    int64_t days;
    int64_t within;
    parts.valid = 0;
    if (!isfinite(time)) {
        return parts;
    }
    millis = (int64_t)time;
    days = kiln_c99_floor_div(millis, 86400000);
    within = kiln_c99_floor_mod(millis, 86400000);
    kiln_c99_civil_from_days(days, &parts.year, &parts.month, &parts.day);
    /* 1970-01-01 was a Thursday, which is day 4. */
    parts.weekday = (int)kiln_c99_floor_mod(days + 4, 7);
    parts.hour = (int)(within / 3600000);
    parts.minute = (int)(within / 60000 % 60);
    parts.second = (int)(within / 1000 % 60);
    parts.millisecond = (int)(within % 1000);
    parts.valid = 1;
    return parts;
}

static double kiln_c99_date_time(kiln_value value) {
    return *(double *)(uintptr_t)value.bits;
}

static int kiln_c99_new_date(kiln_value *out, double time) {
    double *slot = (double *)malloc(sizeof *slot);
    if (slot == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    *slot = time;
    *out = kiln_c99_value(KILN_VALUE_DATE, (uint64_t)(uintptr_t)slot, NULL, 0);
    return KILN_C99_OK;
}

/* Fixed-width decimal, because every field of an ISO instant is written to its own width and
 * `kiln_number_to_js_string` writes the shortest form instead. */
static int kiln_c99_pad_number(kiln_text *text, int64_t value, int width) {
    char digits[24];
    int at = (int)sizeof digits;
    int negative = value < 0;
    uint64_t magnitude = negative ? (uint64_t)(-value) : (uint64_t)value;
    do {
        digits[--at] = (char)('0' + (magnitude % 10));
        magnitude /= 10;
    } while (magnitude != 0);
    while ((int)sizeof digits - at < width) {
        digits[--at] = '0';
    }
    if (negative && kiln_c99_text_append(text, "-", 1) != KILN_C99_OK) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    return kiln_c99_text_append(text, digits + at, (size_t)((int)sizeof digits - at));
}

/* `toISOString`, or a string with no contents when the instant has no ISO spelling -- the caller
 * decides whether that is a throw or a null, because `toISOString` and `toJSON` differ there. */
static int kiln_c99_iso_string(kiln_value *out, double time) {
    kiln_date_parts parts = kiln_c99_date_parts(time);
    kiln_text text = {NULL, 0, 0};
    if (!parts.valid) {
        *out = kiln_c99_undefined();
        return KILN_C99_OK;
    }
    /* A year outside four digits is written with a sign and six, which is the only shape that
     * stays unambiguous. */
    if (parts.year >= 0 && parts.year <= 9999) {
        if (kiln_c99_pad_number(&text, parts.year, 4) != KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    } else {
        if (kiln_c99_text_append_c(&text, parts.year < 0 ? "-" : "+") != KILN_C99_OK ||
            kiln_c99_pad_number(&text, parts.year < 0 ? -parts.year : parts.year, 6) !=
                KILN_C99_OK) {
            free(text.bytes);
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    if (kiln_c99_text_append_c(&text, "-") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.month, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, "-") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.day, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, "T") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.hour, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.minute, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.second, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ".") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.millisecond, 3) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, "Z") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

/* `String(date)`: the human form, which is UTC here because the realm has no other zone. */
static int kiln_c99_date_to_string(kiln_value *out, double time) {
    static const char *const WEEKDAYS[7] = {"Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"};
    static const char *const MONTHS[12] = {"Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                           "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"};
    kiln_date_parts parts = kiln_c99_date_parts(time);
    kiln_text text = {NULL, 0, 0};
    if (!parts.valid) {
        *out = kiln_c99_copy_text("Invalid Date", 12);
        return KILN_C99_OK;
    }
    if (kiln_c99_text_append_c(&text, WEEKDAYS[parts.weekday]) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, MONTHS[parts.month - 1]) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.day, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.year, 4) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " ") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.hour, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.minute, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, ":") != KILN_C99_OK ||
        kiln_c99_pad_number(&text, parts.second, 2) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text, " GMT+0000 (Coordinated Universal Time)") != KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    *out = kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len);
    return KILN_C99_OK;
}

static kiln_collection *kiln_c99_collection_of(kiln_value value) {
    return (kiln_collection *)(uintptr_t)value.bits;
}

static int kiln_c99_new_collection(kiln_value *out, int is_map) {
    kiln_collection *collection = (kiln_collection *)malloc(sizeof *collection);
    if (collection == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    collection->entries = NULL;
    collection->count = 0;
    collection->live = 0;
    collection->capacity = 0;
    collection->is_map = is_map;
    *out = kiln_c99_value(KILN_VALUE_COLLECTION, (uint64_t)(uintptr_t)collection, NULL, 0);
    return KILN_C99_OK;
}

/* The index of a key, or -1. SameValueZero, which is what a collection compares with: NaN finds
 * itself, where `===` never would, and that is why `new Set([NaN, NaN]).size` is 1. */
static double kiln_c99_collection_find(kiln_collection *collection, kiln_value key) {
    size_t index;
    for (index = 0; index < collection->count; ++index) {
        if (collection->entries[index].dead) {
            continue;
        }
        if (kiln_c99_same_value_zero(collection->entries[index].key, key)) {
            return (double)index;
        }
    }
    return -1.0;
}

static int kiln_c99_collection_put(kiln_value target, kiln_value key, kiln_value value) {
    kiln_collection *collection = kiln_c99_collection_of(target);
    double at = kiln_c99_collection_find(collection, key);
    int status;
    if (at >= 0.0) {
        /* Re-setting a key keeps its position: insertion order is the order of *first*
         * appearance, which is what iteration reports. */
        collection->entries[(size_t)at].value = value;
        return KILN_C99_OK;
    }
    status = kiln_c99_reserve((void **)&collection->entries, &collection->capacity,
                              collection->count + 1, sizeof *collection->entries);
    if (status != KILN_C99_OK) {
        return status;
    }
    collection->entries[collection->count].key = key;
    collection->entries[collection->count].value = value;
    collection->entries[collection->count].dead = 0;
    collection->count += 1;
    collection->live += 1;
    return KILN_C99_OK;
}

/* An iterator over a snapshot array.
 *
 * `for...of` asks for an iterator and then drives `next`, so a collection's `@@iterator` cannot
 * hand back the array itself -- an array has no `next`. The state is two reserved properties on
 * an ordinary object, which is what the interpreter does and for the same reason: an iterator is
 * an object with a method, and inventing a third kind of value for it would buy nothing. */
static int kiln_c99_new_iterator(kiln_value *out, kiln_value items) {
    kiln_value iterator;
    int status = kiln_c99_new_object(&iterator);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, iterator, "@@iterated", 10, items, 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, iterator, "@@index", 7, kiln_c99_number(0.0), 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, iterator, "next", 4,
                          kiln_c99_method(KILN_C99_METHOD_ITERATOR_NEXT), 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    /* An iterator is its own iterable, which is what lets `[...m.keys()]` work: spread asks for
     * an iterator, and asking one for another has to answer with itself. */
    status = kiln_c99_put(NULL, iterator, "@@iterator", 10,
                          kiln_c99_method(KILN_C99_METHOD_ITERATOR_SELF), 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    *out = iterator;
    return KILN_C99_OK;
}

static const char *kiln_c99_error_name(uint64_t builtin) {
    switch (builtin) {
    case KILN_C99_BUILTIN_TYPE_ERROR: return "TypeError";
    case KILN_C99_BUILTIN_RANGE_ERROR: return "RangeError";
    case KILN_C99_BUILTIN_REFERENCE_ERROR: return "ReferenceError";
    case KILN_C99_BUILTIN_SYNTAX_ERROR: return "SyntaxError";
    case KILN_C99_BUILTIN_ERROR: return "Error";
    default: return NULL;
    }
}

/* An error is an ordinary object carrying `message`, `name` and `stack`, none of them
 * enumerated: `err.message` reads and `Object.keys(err)` is empty, which is why
 * `JSON.stringify(err)` is `{}`. The kind is remembered in a reserved property rather than
 * inferred from `name`, so a program that overwrites `name` does not change what it *is*. */
static int kiln_c99_new_error(kiln_value *out, uint64_t kind, kiln_value message) {
    const char *name = kiln_c99_error_name(kind);
    kiln_value error;
    kiln_text stack = {NULL, 0, 0};
    kiln_value text = message.tag == KILN_VALUE_UNDEFINED ? kiln_c99_from_units(NULL, 0)
                                                          : kiln_c99_text(message);
    int status;
    if (name == NULL || text.tag != KILN_VALUE_STRING) {
        return KILN_C99_CONTROL_ERROR;
    }
    status = kiln_c99_new_object(&error);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (kiln_c99_text_append_c(&stack, name) != KILN_C99_OK ||
        (text.len != 0 && (kiln_c99_text_append_c(&stack, ": ") != KILN_C99_OK ||
                           kiln_c99_text_append(&stack, text.ptr, text.len) != KILN_C99_OK))) {
        free(stack.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_put(NULL, error, "message", 7, text, 0, NULL);
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, error, "name", 4, kiln_c99_copy_text(name, strlen(name)), 0,
                              NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, error, "stack", 5,
                              kiln_c99_value(KILN_VALUE_STRING, 0, stack.bytes, stack.len), 0,
                              NULL);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(NULL, error, "@@error", 7, kiln_c99_number((double)kind), 0, NULL);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        /* None of the four is enumerated. The reserved one is hidden by its prefix anyway; the
         * other three have to be marked. */
        kiln_object *object = kiln_c99_object_of(error);
        size_t index;
        for (index = 0; index < object->count; ++index) {
            object->properties[index].enumerable = 0;
        }
    }
    *out = error;
    return KILN_C99_OK;
}

/* What `Promise.any` rejects with when nothing succeeded: the reasons, and a sentence saying so.
 *
 * A plain `Error` rather than an `AggregateError`, which is the kind the interpreter builds and
 * therefore the kind this has to build -- node names it `AggregateError` and both engines are
 * behind on that together. `errors` is not enumerated, for the same reason an error's `message`
 * is not: it is the error saying what happened, not a property of an object. */
static int kiln_c99_aggregate_error(kiln_value *out, kiln_value reasons) {
    kiln_value aggregate;
    int status = kiln_c99_new_error(&aggregate, KILN_C99_BUILTIN_ERROR,
                                    kiln_c99_copy_text("All promises were rejected", 26));
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_put(NULL, aggregate, "errors", 6, reasons, 0, NULL);
    if (status != KILN_C99_OK) {
        return status;
    }
    {
        kiln_object *object = kiln_c99_object_of(aggregate);
        if (object->count != 0) {
            object->properties[object->count - 1].enumerable = 0;
        }
    }
    *out = aggregate;
    return KILN_C99_OK;
}

/* The own slot a private name occupies, or NULL.
 *
 * Own-only, and that own-only rule *is* the brand check: an object of another class does not carry
 * the name, however much of the rest of the shape it shares. Nothing walks the prototype chain
 * here for the same reason -- inheriting a private name would be inheriting somebody else's
 * identity. A static private member lives on the constructor, whose properties are in its own
 * store rather than in an object. */
static kiln_property *kiln_c99_own_private(kiln_value target, const char *key, size_t key_len) {
    kiln_object *object;
    size_t index;
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_value store = kiln_c99_closure_of(target)->properties;
        if (store.tag != KILN_VALUE_OBJECT) {
            return NULL;
        }
        object = kiln_c99_object_of(store);
    } else if (target.tag == KILN_VALUE_OBJECT) {
        object = kiln_c99_object_of(target);
    } else {
        return NULL;
    }
    for (index = 0; index < object->count; ++index) {
        if (kiln_c99_same_key(&object->properties[index], key, key_len)) {
            return &object->properties[index];
        }
    }
    return NULL;
}

/* A private name on an object that does not carry it: a TypeError naming the member.
 *
 * The stored key is `@@private:<scope>:<name>` -- reserved so nothing that enumerates own string
 * properties can see it and no computed access can spell it -- and the message wants the `<name>`
 * half, spelled the way the program spells it. */
static int kiln_c99_private_error(kiln_value *thrown, const char *key, size_t key_len,
                                  int writing) {
    kiln_text text = {NULL, 0, 0};
    kiln_value error;
    size_t at = 0;
    size_t colons = 0;
    int status;
    while (at < key_len && colons < 2) {
        if (key[at] == ':') {
            colons += 1;
        }
        at += 1;
    }
    if (colons < 2) {
        at = 0;
    }
    if (kiln_c99_text_append_c(&text, writing ? "Cannot write private member #"
                                              : "Cannot read private member #") != KILN_C99_OK ||
        kiln_c99_text_append(&text, key + at, key_len - at) != KILN_C99_OK ||
        kiln_c99_text_append_c(&text,
                               writing ? " to an object whose class did not declare it"
                                       : " from an object whose class did not declare it") !=
            KILN_C99_OK) {
        free(text.bytes);
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_new_error(&error, KILN_C99_BUILTIN_TYPE_ERROR,
                                kiln_c99_value(KILN_VALUE_STRING, 0, text.bytes, text.len));
    if (status != KILN_C99_OK) {
        return status;
    }
    *thrown = error;
    return KILN_C99_THROWN;
}

/* Raise a language-level error: build it, hand it to the caller as the thrown value, and report
 * that something was thrown rather than that something could not be done.
 *
 * The difference matters because a `catch` takes the first and must not take the second: a
 * TypeError is the program's business and an unimplemented instruction is not. */
static int kiln_c99_throw_error(kiln_value *thrown, uint64_t kind, const char *message) {
    kiln_value built;
    int status;
    if (thrown == NULL) {
        return KILN_C99_CONTROL_ERROR;
    }
    status = kiln_c99_new_error(&built, kind, kiln_c99_copy_text(message, strlen(message)));
    if (status != KILN_C99_OK) {
        return status;
    }
    *thrown = built;
    return KILN_C99_THROWN;
}

/* The microtask queue.
 *
 * One per generated unit rather than one per program, drained when the unit's entry point
 * finishes. For a single module -- which is what `kiln run` compiles and what every probe here
 * is -- that is exactly node's ordering: the body runs to completion and then the queue drains.
 * For a *graph* it is an approximation, because node drains after every module body rather than
 * after each one; see NOTES.md, where the shared-queue design that fixes it is written down. */
static kiln_microtask *kiln_c99_queue;
static size_t kiln_c99_queue_head;
static size_t kiln_c99_queue_count;
static size_t kiln_c99_queue_capacity;


static int kiln_c99_enqueue(kiln_value handler, kiln_value argument, kiln_value derived,
                            int rejected) {
    int status;
    if (kiln_c99_queue_head > 0 && kiln_c99_queue_count == kiln_c99_queue_capacity) {
        /* Reclaim what has already run before growing: a long chain of `then`s would otherwise
         * grow the queue by one for every link even though only one is ever live. */
        size_t index;
        for (index = 0; index < kiln_c99_queue_count - kiln_c99_queue_head; ++index) {
            kiln_c99_queue[index] = kiln_c99_queue[kiln_c99_queue_head + index];
        }
        kiln_c99_queue_count -= kiln_c99_queue_head;
        kiln_c99_queue_head = 0;
    }
    status = kiln_c99_reserve((void **)&kiln_c99_queue, &kiln_c99_queue_capacity,
                              kiln_c99_queue_count + 1, sizeof *kiln_c99_queue);
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_c99_queue[kiln_c99_queue_count].handler = handler;
    kiln_c99_queue[kiln_c99_queue_count].argument = argument;
    kiln_c99_queue[kiln_c99_queue_count].derived = derived;
    kiln_c99_queue[kiln_c99_queue_count].rejected = rejected;
    kiln_c99_queue_count += 1;
    return KILN_C99_OK;
}

static int kiln_c99_new_promise(kiln_value *out) {
    kiln_promise *promise = (kiln_promise *)malloc(sizeof *promise);
    if (promise == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    promise->state = KILN_C99_PENDING;
    promise->value = kiln_c99_undefined();
    promise->reactions = NULL;
    promise->count = 0;
    promise->capacity = 0;
    *out = kiln_c99_value(KILN_VALUE_PROMISE, (uint64_t)(uintptr_t)promise, NULL, 0);
    return KILN_C99_OK;
}

static kiln_promise *kiln_c99_promise_of(kiln_value value) {
    return (kiln_promise *)(uintptr_t)value.bits;
}

static int kiln_c99_bind_with(kiln_value *out, kiln_value target, kiln_value receiver,
                              const kiln_value *arguments, size_t count) {
    kiln_bound *bound = (kiln_bound *)malloc(sizeof *bound);
    if (bound == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    bound->target = target;
    bound->receiver = receiver;
    bound->arguments = NULL;
    bound->count = count;
    if (count != 0) {
        bound->arguments = (kiln_value *)malloc(count * sizeof *bound->arguments);
        if (bound->arguments == NULL) {
            free(bound);
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(bound->arguments, arguments, count * sizeof *bound->arguments);
    }
    *out = kiln_c99_value(KILN_VALUE_BOUND, (uint64_t)(uintptr_t)bound, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_bind(kiln_value *out, kiln_value target, kiln_value receiver) {
    return kiln_c99_bind_with(out, target, receiver, NULL, 0);
}

static int kiln_c99_settle(kiln_value promise, kiln_value value, int rejected);

/* Queue one reaction against a promise that has already settled. */
static int kiln_c99_schedule(kiln_promise *promise, const kiln_reaction *reaction) {
    int rejected = promise->state == KILN_C99_REJECTED;
    return kiln_c99_enqueue(rejected ? reaction->on_rejected : reaction->on_fulfilled,
                            promise->value, reaction->derived, rejected);
}

/* Settle a promise, queueing everything that was waiting for it.
 *
 * Fulfilling with another promise *adopts* it rather than nesting: the value a promise arrives
 * at is never itself a promise, which is what makes a `then` chain flat. */
static int kiln_c99_settle(kiln_value promise, kiln_value value, int rejected) {
    kiln_promise *state = kiln_c99_promise_of(promise);
    size_t index;
    int status;
    if (state->state != KILN_C99_PENDING) {
        return KILN_C99_OK;
    }
    if (!rejected && value.tag == KILN_VALUE_PROMISE) {
        /* Resolving with a promise does not settle this one: it *follows* the other. Attaching to
         * it is a job in its own right, so a chain that returns a promise from a handler takes one
         * more turn than one that returns a plain value -- which is a difference a program sees in
         * what order things print, and therefore one both engines have to make. */
        kiln_value step;
        status = kiln_c99_bind(&step, kiln_c99_method(KILN_C99_METHOD_ADOPT_STEP), promise);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_enqueue(step, value, kiln_c99_undefined(), 0);
    }
    state->state = rejected ? KILN_C99_REJECTED : KILN_C99_FULFILLED;
    state->value = value;
    for (index = 0; index < state->count; ++index) {
        status = kiln_c99_schedule(state, &state->reactions[index]);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    state->count = 0;
    return KILN_C99_OK;
}

static int kiln_c99_new_settler(kiln_value *out, kiln_value promise, int rejects) {
    kiln_settler *settler = (kiln_settler *)malloc(sizeof *settler);
    if (settler == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    settler->promise = promise;
    settler->rejects = rejects;
    *out = kiln_c99_value(KILN_VALUE_SETTLER, (uint64_t)(uintptr_t)settler, NULL, 0);
    return KILN_C99_OK;
}

/* `p.then(f, r)`: the derived promise settles with what the handler produces, or with what `p`
 * produced when there is no handler for that side -- which is what makes a rejection travel down
 * a chain of `then`s until something catches it. */
static int kiln_c99_then(kiln_value promise, kiln_value on_fulfilled, kiln_value on_rejected,
                         kiln_value *out) {
    kiln_promise *state = kiln_c99_promise_of(promise);
    kiln_reaction reaction;
    int status = kiln_c99_new_promise(&reaction.derived);
    if (status != KILN_C99_OK) {
        return status;
    }
    reaction.on_fulfilled = on_fulfilled;
    reaction.on_rejected = on_rejected;
    *out = reaction.derived;
    if (state->state != KILN_C99_PENDING) {
        return kiln_c99_schedule(state, &reaction);
    }
    status = kiln_c99_reserve((void **)&state->reactions, &state->capacity, state->count + 1,
                              sizeof *state->reactions);
    if (status != KILN_C99_OK) {
        return status;
    }
    state->reactions[state->count] = reaction;
    state->count += 1;
    return KILN_C99_OK;
}

/* Run the queue until it is empty, which is what "after the synchronous part" means. */
/* The event loop: microtasks until there are none, then the earliest due timer, then round
 * again.
 *
 * The order is the observable one. Everything already queued runs before any timer, which is why
 * `Promise.resolve().then(f)` beats `setTimeout(g, 0)` however early `g` was scheduled -- and it
 * is checked between turns rather than at each settlement, so a `.catch` attached on the next
 * line still counts as attached. */
static int kiln_c99_run_loop(kiln_host *host, int depth, kiln_value *thrown) {
    for (;;) {
        size_t index;
        size_t chosen;
        kiln_timer timer;
        kiln_value produced;
        int status = kiln_c99_drain(host, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        chosen = kiln_c99_timer_count;
        for (index = 0; index < kiln_c99_timer_count; ++index) {
            if (chosen == kiln_c99_timer_count ||
                kiln_c99_timers[index].due < kiln_c99_timers[chosen].due ||
                (kiln_c99_timers[index].due == kiln_c99_timers[chosen].due &&
                 kiln_c99_timers[index].sequence < kiln_c99_timers[chosen].sequence)) {
                chosen = index;
            }
        }
        if (chosen == kiln_c99_timer_count) {
            return KILN_C99_OK;
        }
        timer = kiln_c99_timers[chosen];
        if (kiln_c99_now < timer.due) {
            kiln_c99_now = timer.due;
        }
        if (timer.repeats) {
            /* Re-armed *before* running, and under the same id, so a `clearInterval` inside the
             * callback cancels the next run rather than one that no longer exists. A zero delay
             * becomes one, or an interval would starve every timer behind it. */
            kiln_c99_next_sequence += 1;
            kiln_c99_timers[chosen].due =
                kiln_c99_now + (timer.repeat > 1.0 ? timer.repeat : 1.0);
            kiln_c99_timers[chosen].sequence = kiln_c99_next_sequence;
        } else {
            memmove(kiln_c99_timers + chosen, kiln_c99_timers + chosen + 1,
                    (kiln_c99_timer_count - chosen - 1) * sizeof *kiln_c99_timers);
            kiln_c99_timer_count -= 1;
        }
        status = kiln_c99_call(host, &produced, timer.callback, kiln_c99_undefined(),
                               timer.argc == 0 ? NULL : timer.args, timer.argc, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
}

static int kiln_c99_drain(kiln_host *host, int depth, kiln_value *thrown) {
    while (kiln_c99_queue_head < kiln_c99_queue_count) {
        kiln_microtask task = kiln_c99_queue[kiln_c99_queue_head];
        kiln_value produced;
        int status;
        kiln_c99_queue_head += 1;
        if (!kiln_c99_is_callable(task.handler)) {
            /* No handler for this side, so the value travels on unchanged. */
            if (task.derived.tag == KILN_VALUE_PROMISE) {
                status = kiln_c99_settle(task.derived, task.argument, task.rejected);
                if (status != KILN_C99_OK) {
                    return status;
                }
            }
            continue;
        }
        status = kiln_c99_call(host, &produced, task.handler, kiln_c99_undefined(), &task.argument,
                               1, depth, thrown);
        if (status == KILN_C99_THROWN) {
            /* A handler that throws rejects the promise it was producing, rather than failing
             * the program: that is the whole reason `catch` exists further down the chain. */
            if (task.derived.tag == KILN_VALUE_PROMISE) {
                status = kiln_c99_settle(task.derived, *thrown, 1);
                if (status != KILN_C99_OK) {
                    return status;
                }
                continue;
            }
            return KILN_C99_THROWN;
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        if (task.derived.tag == KILN_VALUE_PROMISE) {
            status = kiln_c99_settle(task.derived, produced, 0);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    return KILN_C99_OK;
}

/* One step of an async function: resume the body, and decide what to do with where it stopped.
 *
 * It yields when it awaits, so the value it hands back is what it is waiting on. Everything that
 * has to happen after arrival is attached to that as two handlers -- resume with the value, or
 * throw into the body -- and the machine goes back to the queue. That is the whole of `await`:
 * the suspension is the generator's, and the scheduling is the promise's. */
/* Settle one step's promise with `{ value, done }`, which is the shape the iteration protocol is
 * written against -- an async generator's `next` promises that object rather than the value. */
static int kiln_c99_async_generator_settle(kiln_host *host, kiln_value step, kiln_value value,
                                           int finished, int depth, kiln_value *thrown) {
    kiln_value shaped;
    int status = kiln_c99_new_object(&shaped);
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, shaped, "value", 5, value, depth, thrown);
    }
    if (status == KILN_C99_OK) {
        status = kiln_c99_put(host, shaped, "done", 4, kiln_c99_boolean(finished), depth, thrown);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    return kiln_c99_settle(step, shaped, 0);
}

/* One step of an async generator: resume the body until it either awaits or finishes a step.
 *
 * The difference from an async *function* is the whole reason this exists. A function has one
 * promise and runs to completion; a generator has one promise per `next`, and an `await` between
 * two `yield`s must leave that promise pending -- the step is not finished, and the consumer has
 * not been told anything yet. `frame->awaiting` is what says which suspension happened. */
static int kiln_c99_async_generator_step(kiln_host *host, kiln_value generator, kiln_value sent,
                                         int rejected, int depth, kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    kiln_value step = frame->promise;
    kiln_closure *closure;
    kiln_value produced;
    int status;
    if (frame->done) {
        return kiln_c99_async_generator_settle(host, step, kiln_c99_undefined(), 1, depth, thrown);
    }
    if (rejected && !frame->started) {
        frame->done = 1;
        return kiln_c99_settle(step, sent, 1);
    }
    frame->sent = sent;
    frame->raising = rejected;
    closure = kiln_c99_closure_of(frame->function);
    status = closure->body(host, &produced, closure->upvalues, frame->receiver, NULL, 0, depth + 1,
                           thrown, frame);
    if (status == KILN_C99_YIELDED) {
        frame->started = 1;
        if (frame->awaiting) {
            /* Still inside the step: wait for what was awaited and come back here. */
            kiln_value awaited = produced;
            kiln_value resume;
            kiln_value raise;
            kiln_value ignored;
            if (awaited.tag != KILN_VALUE_PROMISE) {
                kiln_value wrapped;
                status = kiln_c99_new_promise(&wrapped);
                if (status == KILN_C99_OK) {
                    status = kiln_c99_settle(wrapped, awaited, 0);
                }
                if (status != KILN_C99_OK) {
                    return status;
                }
                awaited = wrapped;
            }
            status = kiln_c99_bind(&resume, kiln_c99_method(KILN_C99_METHOD_ASYNC_RESUME),
                                   generator);
            if (status == KILN_C99_OK) {
                status = kiln_c99_bind(&raise, kiln_c99_method(KILN_C99_METHOD_ASYNC_THROW),
                                       generator);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            return kiln_c99_then(awaited, resume, raise, &ignored);
        }
        return kiln_c99_async_generator_settle(host, step, produced, 0, depth, thrown);
    }
    if (status == KILN_C99_THROWN) {
        frame->done = 1;
        return kiln_c99_settle(step, *thrown, 1);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    frame->done = 1;
    return kiln_c99_async_generator_settle(host, step, produced, 1, depth, thrown);
}

static int kiln_c99_async_step(kiln_host *host, kiln_value generator, kiln_value sent,
                               int rejected, int depth, kiln_value *thrown) {
    kiln_generator *frame = kiln_c99_generator_of(generator);
    kiln_closure *closure;
    kiln_value produced;
    int status;
    if (frame->done) {
        return KILN_C99_OK;
    }
    if (rejected && !frame->started) {
        /* Nothing has run yet, so there is no suspension point to raise at and the only place the
         * rejection can go is the function's own promise. */
        frame->done = 1;
        return kiln_c99_settle(frame->promise, sent, 1);
    }
    frame->sent = sent;
    frame->raising = rejected;
    closure = kiln_c99_closure_of(frame->function);
    status = closure->body(host, &produced, closure->upvalues, frame->receiver, NULL, 0, depth + 1,
                           thrown, frame);
    if (status == KILN_C99_YIELDED) {
        kiln_value awaited = produced;
        kiln_value resume;
        kiln_value raise;
        kiln_value ignored;
        frame->started = 1;
        if (awaited.tag != KILN_VALUE_PROMISE) {
            /* Awaiting a plain value still yields to the queue: `await 1` is a suspension, and a
             * program that relies on it running later is relying on that. */
            kiln_value immediate;
            status = kiln_c99_new_promise(&immediate);
            if (status == KILN_C99_OK) {
                status = kiln_c99_settle(immediate, awaited, 0);
            }
            if (status != KILN_C99_OK) {
                return status;
            }
            awaited = immediate;
        }
        status = kiln_c99_bind(&resume, kiln_c99_method(KILN_C99_METHOD_ASYNC_RESUME), generator);
        if (status == KILN_C99_OK) {
            status = kiln_c99_bind(&raise, kiln_c99_method(KILN_C99_METHOD_ASYNC_THROW),
                                   generator);
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_then(awaited, resume, raise, &ignored);
    }
    frame->done = 1;
    if (status == KILN_C99_THROWN) {
        /* The body threw where nothing caught it, which rejects the promise rather than failing
         * the program: `async function f() { throw e }` is how a rejection is often written. */
        return kiln_c99_settle(frame->promise, *thrown, 1);
    }
    if (status != KILN_C99_OK) {
        return status;
    }
    return kiln_c99_settle(frame->promise, produced, 0);
}

/* `new f(...)`: build the instance, run the body with it as `this`, and yield the instance --
 * unless the body returned an object, which wins. That override is what makes a factory
 * constructor work. */
/* An ISO-8601 instant, or NaN. Deliberately narrow: the shapes a program writes down and the
 * shapes `toISOString` produces, which is what a round trip needs. Anything looser would accept
 * text the interpreter rejects, and the two have to agree. */
static double kiln_c99_parse_date(const char *bytes, size_t len) {
    int64_t fields[7] = {0, 1, 1, 0, 0, 0, 0};
    size_t at = 0;
    size_t index;
    static const size_t WIDTHS[7] = {4, 2, 2, 2, 2, 2, 3};
    while (at < len && (bytes[at] == ' ' || bytes[at] == '\t')) {
        at += 1;
    }
    while (len > at && (bytes[len - 1] == ' ' || bytes[len - 1] == '\t')) {
        len -= 1;
    }
    for (index = 0; index < 7; ++index) {
        size_t width = WIDTHS[index];
        size_t digit;
        if (index > 0) {
            /* The separator each field is introduced by, and the point past which the rest is
             * optional: a date alone is a valid instant. */
            char expected = index == 1 || index == 2 ? '-' : index == 3 ? 'T' : index == 6 ? '.' : ':';
            if (at >= len || bytes[at] != expected) {
                break;
            }
            at += 1;
        }
        if (len - at < width) {
            return 0.0 / 0.0;
        }
        fields[index] = 0;
        for (digit = 0; digit < width; ++digit) {
            char byte = bytes[at + digit];
            if (byte < '0' || byte > '9') {
                return 0.0 / 0.0;
            }
            fields[index] = fields[index] * 10 + (byte - '0');
        }
        at += width;
    }
    /* A trailing `Z` is the only zone accepted, because the realm has no other. */
    if (at < len && bytes[at] == 'Z') {
        at += 1;
    }
    if (at != len || fields[1] < 1 || fields[1] > 12 || fields[2] < 1 || fields[2] > 31) {
        return 0.0 / 0.0;
    }
    return (double)kiln_c99_days_from_civil(fields[0], fields[1], fields[2]) * 86400000.0 +
           (double)fields[3] * 3600000.0 + (double)fields[4] * 60000.0 +
           (double)fields[5] * 1000.0 + (double)fields[6];
}

static int kiln_c99_construct(kiln_host *host, kiln_value *result, kiln_value callee,
                              const kiln_value *args, size_t argc, int depth,
                              kiln_value *thrown) {
    kiln_value prototype;
    kiln_value instance;
    kiln_value produced;
    kiln_value saved_new_target;
    int status;
    if (callee.tag == KILN_VALUE_BUILTIN && (callee.bits == KILN_C99_BUILTIN_REGEXP ||
                                             callee.bits == KILN_C99_BUILTIN_STRING ||
                                             callee.bits == KILN_C99_BUILTIN_NUMBER ||
                                             callee.bits == KILN_C99_BUILTIN_BOOLEAN)) {
        /* These four build the same thing whether or not `new` was written, so they go through
         * the call path rather than repeating it. (For the three primitives that is not what the
         * language says -- `new Number(1)` is an object wrapper -- but neither engine has wrapper
         * objects, and agreeing about that is better than one of them inventing them.) */
        return kiln_c99_call(host, result, callee, kiln_c99_undefined(), args, argc, depth,
                             thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_PROMISE) {
        kiln_value promise;
        kiln_value settlers[2];
        kiln_value ignored;
        status = kiln_c99_new_promise(&promise);
        if (status == KILN_C99_OK) {
            status = kiln_c99_new_settler(&settlers[0], promise, 0);
        }
        if (status == KILN_C99_OK) {
            status = kiln_c99_new_settler(&settlers[1], promise, 1);
        }
        if (status != KILN_C99_OK) {
            return status;
        }
        /* The executor runs *now*, synchronously, which is what lets it capture something and
         * settle the promise later. One that throws rejects rather than failing the program:
         * `new Promise(() => { throw e })` is how a rejection is often written. */
        status = kiln_c99_call(host, &ignored, argc > 0 ? args[0] : kiln_c99_undefined(),
                               kiln_c99_undefined(), settlers, 2, depth, thrown);
        if (status == KILN_C99_THROWN) {
            status = kiln_c99_settle(promise, *thrown, 1);
            if (status != KILN_C99_OK) {
                return status;
            }
        } else if (status != KILN_C99_OK) {
            return status;
        }
        *result = promise;
        return KILN_C99_OK;
    }
    if (callee.tag == KILN_VALUE_BUILTIN && kiln_c99_error_name(callee.bits) != NULL) {
        return kiln_c99_new_error(result, callee.bits,
                                  argc > 0 ? args[0] : kiln_c99_undefined());
    }
    if (callee.tag == KILN_VALUE_BUILTIN && (callee.bits == KILN_C99_BUILTIN_MAP ||
                                             callee.bits == KILN_C99_BUILTIN_SET)) {
        int is_map = callee.bits == KILN_C99_BUILTIN_MAP;
        kiln_value built;
        status = kiln_c99_new_collection(&built, is_map);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (argc > 0 && args[0].tag != KILN_VALUE_UNDEFINED && args[0].tag != KILN_VALUE_NULL) {
            /* The argument is anything iterable: a Set takes its values and a Map takes
             * `[key, value]` pairs, which is why `new Map(map)` copies one. */
            kiln_value gathered;
            size_t index;
            status = kiln_c99_new_array(&gathered);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_iterate(host, gathered, args[0], depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            {
                kiln_array *given = kiln_c99_array_of(gathered);
                for (index = 0; index < given->count; ++index) {
                    kiln_value key = given->elements[index];
                    kiln_value member = key;
                    if (is_map) {
                        kiln_array *pair;
                        if (key.tag != KILN_VALUE_ARRAY) {
                            return KILN_C99_CONTROL_ERROR;
                        }
                        pair = kiln_c99_array_of(key);
                        key = pair->count > 0 ? pair->elements[0] : kiln_c99_undefined();
                        member = pair->count > 1 ? pair->elements[1] : kiln_c99_undefined();
                    }
                    status = kiln_c99_collection_put(built, key, member);
                    if (status != KILN_C99_OK) {
                        return status;
                    }
                }
            }
        }
        *result = built;
        return KILN_C99_OK;
    }
    if (callee.tag == KILN_VALUE_BUILTIN && callee.bits == KILN_C99_BUILTIN_DATE) {
        /* `new Date()` is the current instant, which is virtual here and never advances; one
         * argument is either milliseconds or an instant written down; more than one is the field
         * form, where the month is zero-based and the day defaults to 1. */
        double time;
        if (argc == 0) {
            time = 0.0;
        } else if (argc == 1) {
            time = args[0].tag == KILN_VALUE_STRING
                       ? kiln_c99_parse_date(args[0].ptr, args[0].len)
                       : kiln_c99_as_number(args[0]);
        } else {
            double fields[7];
            size_t index;
            for (index = 0; index < 7; ++index) {
                fields[index] = index < argc  ? kiln_c99_as_number(args[index])
                                : index == 2 ? 1.0
                                             : 0.0;
            }
            time = kiln_c99_instant_from_parts(fields);
        }
        return kiln_c99_new_date(result, time);
    }
    if (callee.tag != KILN_VALUE_FUNCTION) {
        return KILN_C99_CONTROL_ERROR;
    }
    status = kiln_c99_prototype_object(callee, &prototype);
    if (status != KILN_C99_OK) {
        return status;
    }
    status = kiln_c99_new_object(&instance);
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_c99_object_of(instance)->prototype = prototype;
    /* Inside the body being constructed, `new.target` is the constructor the `new` named -- and
     * the call below is the one that installs it, so the flag that makes `super()` keep it has to
     * be off here whatever the caller was in the middle of. */
    saved_new_target = kiln_c99_new_target;
    kiln_c99_new_target = callee;
    kiln_c99_inheriting_new_target = 1;
    status = kiln_c99_call(host, &produced, callee, instance, args, argc, depth, thrown);
    kiln_c99_inheriting_new_target = 0;
    kiln_c99_new_target = saved_new_target;
    if (status != KILN_C99_OK) {
        return status;
    }
    *result = produced.tag == KILN_VALUE_OBJECT || produced.tag == KILN_VALUE_ARRAY ? produced
                                                                                   : instance;
    return KILN_C99_OK;
}

/* `class D extends C` links two pairs of things: the prototype objects, so an instance of `D`
 * finds a method of `C`, and the *constructors*, so `D` finds a static of `C`. The second is
 * what makes this take a function as well as an object. */
static int kiln_c99_set_prototype(kiln_value target, kiln_value prototype) {
    if (target.tag == KILN_VALUE_OBJECT) {
        kiln_c99_object_of(target)->prototype = prototype;
        return KILN_C99_OK;
    }
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_value store;
        int status = kiln_c99_function_store(target, &store);
        if (status != KILN_C99_OK) {
            return status;
        }
        kiln_c99_object_of(store)->prototype = prototype;
        return KILN_C99_OK;
    }
    return KILN_C99_CONTROL_ERROR;
}

/* `x instanceof C` walks the same chain a property read walks, looking for `C.prototype` rather
 * than for a name -- which is why the two must agree about the shape of the chain. */
static int kiln_c99_instance_of(kiln_value value, kiln_value constructor, int *answer) {
    kiln_value prototype;
    kiln_value current;
    size_t steps;
    int status;
    /* A builtin constructor has no prototype object here; the question it answers is about the
     * tag instead, which is the same question and a shorter walk. */
    if (constructor.tag == KILN_VALUE_BUILTIN) {
        switch (constructor.bits) {
        case KILN_C99_BUILTIN_DATE: *answer = value.tag == KILN_VALUE_DATE; return KILN_C99_OK;
        case KILN_C99_BUILTIN_REGEXP: *answer = value.tag == KILN_VALUE_REGEXP; return KILN_C99_OK;
        case KILN_C99_BUILTIN_ARRAY: *answer = value.tag == KILN_VALUE_ARRAY; return KILN_C99_OK;
        case KILN_C99_BUILTIN_ERROR:
        case KILN_C99_BUILTIN_TYPE_ERROR:
        case KILN_C99_BUILTIN_RANGE_ERROR:
        case KILN_C99_BUILTIN_REFERENCE_ERROR:
        case KILN_C99_BUILTIN_SYNTAX_ERROR: {
            /* `Error` is the base of all of them, so it accepts any kind; the others accept only
             * their own. There is no prototype chain here to walk, and this is the same answer
             * one would give. */
            kiln_value kind;
            *answer = 0;
            if (value.tag == KILN_VALUE_OBJECT &&
                kiln_c99_get(NULL, value, "@@error", 7, &kind, 0, NULL) == KILN_C99_OK &&
                kind.tag == KILN_VALUE_NUMBER) {
                *answer = constructor.bits == KILN_C99_BUILTIN_ERROR ||
                          (uint64_t)kiln_c99_as_number(kind) == constructor.bits;
            }
            return KILN_C99_OK;
        }
        case KILN_C99_BUILTIN_PROMISE:
            *answer = value.tag == KILN_VALUE_PROMISE;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_MAP:
            *answer = value.tag == KILN_VALUE_COLLECTION &&
                      kiln_c99_collection_of(value)->is_map;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_SET:
            *answer = value.tag == KILN_VALUE_COLLECTION &&
                      !kiln_c99_collection_of(value)->is_map;
            return KILN_C99_OK;
        case KILN_C99_BUILTIN_OBJECT:
            *answer = value.tag == KILN_VALUE_OBJECT || value.tag == KILN_VALUE_ARRAY ||
                      value.tag == KILN_VALUE_DATE || value.tag == KILN_VALUE_COLLECTION;
            return KILN_C99_OK;
        default: return KILN_C99_CONTROL_ERROR;
        }
    }
    if (constructor.tag != KILN_VALUE_FUNCTION) {
        return KILN_C99_CONTROL_ERROR;
    }
    status = kiln_c99_prototype_object(constructor, &prototype);
    if (status != KILN_C99_OK) {
        return status;
    }
    *answer = 0;
    current = value;
    for (steps = 0; steps < KILN_C99_MAX_PROTOTYPE_DEPTH; ++steps) {
        if (current.tag != KILN_VALUE_OBJECT) {
            return KILN_C99_OK;
        }
        current = kiln_c99_object_of(current)->prototype;
        if (current.tag == prototype.tag && current.bits == prototype.bits) {
            *answer = 1;
            return KILN_C99_OK;
        }
    }
    return KILN_C99_OK;
}

static kiln_value kiln_c99_typeof(kiln_value value) {
    static const char undefined_text[] = "undefined";
    static const char object_text[] = "object";
    static const char boolean_text[] = "boolean";
    static const char number_text[] = "number";
    static const char string_text[] = "string";
    static const char function_text[] = "function";
    static const char symbol_text[] = "symbol";
    static const char bigint_text[] = "bigint";
    switch (value.tag) {
    case KILN_VALUE_BIGINT:
        return kiln_c99_value(KILN_VALUE_STRING, 0, bigint_text, sizeof(bigint_text) - 1);
    case KILN_VALUE_SYMBOL:
        return kiln_c99_value(KILN_VALUE_STRING, 0, symbol_text, sizeof(symbol_text) - 1);
    case KILN_VALUE_NULL:
        return kiln_c99_value(KILN_VALUE_STRING, 0, object_text, sizeof(object_text) - 1);
    case KILN_VALUE_BOOLEAN:
        return kiln_c99_value(KILN_VALUE_STRING, 0, boolean_text, sizeof(boolean_text) - 1);
    case KILN_VALUE_NUMBER:
        return kiln_c99_value(KILN_VALUE_STRING, 0, number_text, sizeof(number_text) - 1);
    case KILN_VALUE_STRING:
        return kiln_c99_value(KILN_VALUE_STRING, 0, string_text, sizeof(string_text) - 1);
    case KILN_VALUE_BUILTIN:
        /* A namespace is an object holding functions and cannot be called; a constructor is a
         * function. `typeof Math === "object"` is what a library feature-detects on. */
        if (kiln_c99_is_namespace(value.bits)) {
            return kiln_c99_value(KILN_VALUE_STRING, 0, object_text, sizeof(object_text) - 1);
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, function_text, sizeof(function_text) - 1);
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_METHOD:
    /* A bound function is a function, and so is a settler: `typeof resolve` is what an executor
     * checks before calling it. */
    case KILN_VALUE_BOUND:
    case KILN_VALUE_SETTLER:
        return kiln_c99_value(KILN_VALUE_STRING, 0, function_text, sizeof(function_text) - 1);
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_DATE:
    case KILN_VALUE_COLLECTION:
    case KILN_VALUE_REGEXP:
    case KILN_VALUE_PROMISE:
        return kiln_c99_value(KILN_VALUE_STRING, 0, object_text, sizeof(object_text) - 1);
    case KILN_VALUE_UNDEFINED:
    default:
        return kiln_c99_value(KILN_VALUE_STRING, 0, undefined_text, sizeof(undefined_text) - 1);
    }
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
    /* A BigInt stays one under negation and bitwise-not; `+x` is the exception, and is a
     * TypeError in the specification precisely so that `+` cannot silently narrow one. */
    if (value.tag == KILN_VALUE_BIGINT) {
        void *answer = NULL;
        if (operation == 2 && kiln_bigint_negate((const void *)(uintptr_t)value.bits, &answer) == 0) {
            return kiln_c99_bigint_value(answer);
        }
        if (operation == 5) {
            /* `~x` is `-(x + 1)`, which the provider's own arithmetic gets right for a width
             * it does not have to guess at. */
            const void *one = kiln_c99_bigint_one();
            void *sum = NULL;
            if (one != NULL &&
                kiln_bigint_binary(1, (const void *)(uintptr_t)value.bits, one, &sum) == 0 &&
                kiln_bigint_negate(sum, &answer) == 0) {
                return kiln_c99_bigint_value(answer);
            }
        }
    }
    /* Only the numeric operators coerce.  `typeof` reports a type, `void` discards, and `!` asks
     * whether the value is truthy -- a function always is. */
    if (kiln_c99_is_reference(value) &&
        (operation == 1 || operation == 2 || operation == 5)) {
        value = kiln_c99_text(value);
    }
    switch (operation) {
    case 1: return kiln_c99_number(kiln_c99_as_number(value));
    case 2: return kiln_c99_number(-kiln_c99_as_number(value));
    case 3: return kiln_c99_boolean(!kiln_c99_truthy(value));
    case 4: return kiln_c99_typeof(value);
    case 5: return kiln_c99_number(kiln_c99_signed32(~kiln_c99_to_uint32(kiln_c99_as_number(value))));
    case 6: return kiln_c99_undefined();
    default: return kiln_c99_undefined();
    }
}

static kiln_value kiln_c99_binary(uint16_t operation, kiln_value left, kiln_value right) {
    /* Reference operands have already been reduced by `kiln_c99_binary_checked`, which is where
     * the reduction has to live: it is a *call* into the program, and this function has neither
     * a host to make one with nor anywhere to put what it throws. A reference reaching here is a
     * scalar program's, and a scalar program has no reference in it. */
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
    case 15: return kiln_c99_boolean(kiln_c99_loose_equal(left, right));
    case 16: return kiln_c99_boolean(!kiln_c99_loose_equal(left, right));
    case 17: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) &
        kiln_c99_to_uint32(kiln_c99_as_number(right))));
    case 18: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) |
        kiln_c99_to_uint32(kiln_c99_as_number(right))));
    case 19: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) ^
        kiln_c99_to_uint32(kiln_c99_as_number(right))));
    case 20: return kiln_c99_number(kiln_c99_signed32(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) <<
        (kiln_c99_to_uint32(kiln_c99_as_number(right)) & UINT32_C(31))));
    case 21: return kiln_c99_number(kiln_c99_signed32(kiln_c99_shift_right(
        kiln_c99_to_uint32(kiln_c99_as_number(left)),
        kiln_c99_to_uint32(kiln_c99_as_number(right)))));
    case 22: return kiln_c99_number((double)(
        kiln_c99_to_uint32(kiln_c99_as_number(left)) >>
        (kiln_c99_to_uint32(kiln_c99_as_number(right)) & UINT32_C(31))));
    default: return kiln_c99_undefined();
    }
}


/* Reduce a value to a primitive, which is a *call* rather than a formatting rule.
 *
 * Which method is tried first depends on what the operator wanted: arithmetic wants a number and
 * asks `valueOf` first, string conversion asks `toString` first. Every date and decimal library
 * is used through the first of those, and `${x}` differs from `"" + x` only in this.
 *
 * A method that hands back another object is skipped rather than accepted: the point is to reach
 * a primitive, and a `valueOf` returning `this` -- which is what `Object.prototype.valueOf` does
 * -- would otherwise stop the search at its own receiver.
 *
 * A value with no method to call falls back to `kiln_c99_text`, which is where an array's join
 * and a function's source text already live. */
static int kiln_c99_to_primitive(kiln_host *host, kiln_value value, int string_hint,
                                 kiln_value *out, int depth, kiln_value *thrown) {
    static const char *const string_first[2] = {"toString", "valueOf"};
    static const char *const value_first[2] = {"valueOf", "toString"};
    const char *const *order = string_hint ? string_first : value_first;
    int step;
    if (!kiln_c99_is_reference(value)) {
        *out = value;
        return KILN_C99_OK;
    }
    {
        /* `@@toPrimitive` overrides both, and is told which hint it is answering. */
        kiln_value exotic;
        int status = kiln_c99_get(host, value, "@@toPrimitive", 13, &exotic, depth, thrown);
        if (status == KILN_C99_THROWN) {
            return status;
        }
        if (status == KILN_C99_OK && kiln_c99_is_callable(exotic)) {
            kiln_value hint = kiln_c99_copy_text(string_hint ? "string" : "default",
                                                 string_hint ? 6 : 7);
            kiln_value produced;
            status = kiln_c99_call(host, &produced, exotic, value, &hint, 1, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            if (kiln_c99_is_reference(produced)) {
                return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                            "Cannot convert object to primitive value");
            }
            *out = produced;
            return KILN_C99_OK;
        }
    }
    for (step = 0; step < 2; ++step) {
        kiln_value method;
        kiln_value produced;
        int status = kiln_c99_get(host, value, order[step], strlen(order[step]), &method, depth,
                                  thrown);
        if (status == KILN_C99_THROWN) {
            return status;
        }
        if (status != KILN_C99_OK || !kiln_c99_is_callable(method)) {
            continue;
        }
        status = kiln_c99_call(host, &produced, method, value, NULL, 0, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
        if (!kiln_c99_is_reference(produced)) {
            *out = produced;
            return KILN_C99_OK;
        }
    }
    /* Neither was reachable, or neither answered with a primitive. The specification stops the
     * program here, and this used to answer `kiln_c99_text(value)` instead -- which is not merely
     * a wrong value but a wrong *program*: everything after the fault ran, and the process exited
     * 0 where the language says 1. `Object.create(null) + 1` and an object whose `valueOf` and
     * `toString` both return objects are the two ways in. */
    return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                "Cannot convert object to primitive value");
}

/* ToNumber, for a caller that has a host and somewhere to throw.
 *
 * `kiln_c99_as_number` is pure, so on a reference it can only answer NaN -- it cannot run the
 * `valueOf` that ToNumber is specified to call. Every builtin that wants a number from a value the
 * program supplied goes through here instead, which is the same split `kiln_c99_unary_checked`
 * makes below for the operators. */
static int kiln_c99_to_number_checked(kiln_host *host, kiln_value value, double *out, int depth,
                                      kiln_value *thrown) {
    kiln_value reduced;
    int status;
    if (!kiln_c99_is_reference(value)) {
        *out = kiln_c99_as_number(value);
        return KILN_C99_OK;
    }
    status = kiln_c99_to_primitive(host, value, 0, &reduced, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    *out = kiln_c99_as_number(reduced);
    return KILN_C99_OK;
}

/* `+x`, `-x` and `~x` on something that is not a primitive yet.
 *
 * The same reduction the binary operators do, for the same reason: all three want a number, and
 * a number is what `valueOf` is for. `!`, `typeof` and `void` do not coerce -- they answer about
 * the value they were given. */
static int kiln_c99_unary_checked(uint16_t operation, kiln_host *host, kiln_value value,
                                  kiln_value *out, int depth, kiln_value *thrown) {
    if ((operation == 1 || operation == 2 || operation == 5) && kiln_c99_is_reference(value)) {
        int status = kiln_c99_to_primitive(host, value, 0, &value, depth, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    *out = kiln_c99_unary(operation, value);
    return KILN_C99_OK;
}

/* The operators that can throw.
 *
 * Only BigInt makes an operator throw before anything is called, so this wraps
 * `kiln_c99_binary` rather than replacing it: everything without a BigInt operand takes the
 * same path it always did, and only the three throwing cases are spelled out here.
 *
 * The provider owns the arithmetic (ADR 0025) -- writing arbitrary-precision division twice is
 * exactly the duplication that pattern exists to prevent -- so this is the type dispatch and the
 * error surface around it, nothing more. */
static int kiln_c99_binary_checked(uint16_t operation, kiln_host *host, kiln_value left,
                                   kiln_value right, kiln_value *out, int depth,
                                   kiln_value *thrown) {
    int left_big;
    int right_big;
    if (kiln_c99_is_reference(left) || kiln_c99_is_reference(right)) {
        /* Strict equality is identity and never coerces, `in` and `instanceof` take their
         * operands as they are, and a nullish `==` is answered without any conversion at all --
         * `value != null` is the commonest guard in JavaScript, and running a wrapper's `valueOf`
         * there is both wrong and, for a wrapper that assumes it is being used properly, fatal. */
        int coerces = !(operation == 7 || operation == 8 || operation == 13 || operation == 14);
        if (operation == 15 || operation == 16) {
            /* `==` reduces only across a type boundary. Two objects compare as themselves --
             * `{} == {}` is false -- and reducing both to text would make every pair of empty
             * objects equal. A nullish side is answered without any conversion at all: `value
             * != null` is the commonest guard in JavaScript, and running a wrapper's `valueOf`
             * there is both wrong and, for a wrapper that assumes it is being used properly,
             * fatal. */
            if ((kiln_c99_is_reference(left) && kiln_c99_is_reference(right)) ||
                left.tag == KILN_VALUE_UNDEFINED || left.tag == KILN_VALUE_NULL ||
                right.tag == KILN_VALUE_UNDEFINED || right.tag == KILN_VALUE_NULL) {
                coerces = 0;
            }
        }
        if (coerces) {
            /* Every operator here wants a number or has no preference, and both of those ask
             * `valueOf` first. Only `${}` and `String(x)` use the other order. */
            int status = kiln_c99_to_primitive(host, left, 0, &left, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
            status = kiln_c99_to_primitive(host, right, 0, &right, depth, thrown);
            if (status != KILN_C99_OK) {
                return status;
            }
        }
    }
    left_big = left.tag == KILN_VALUE_BIGINT;
    right_big = right.tag == KILN_VALUE_BIGINT;
    const void *a;
    const void *b;
    uint32_t provider_op = 0;
    void *answer = NULL;
    int32_t status;
    if (!left_big && !right_big) {
        *out = kiln_c99_binary(operation, left, right);
        return KILN_C99_OK;
    }
    if (!left_big || !right_big) {
        /* Comparison across the two *is* allowed -- only arithmetic is not -- and so is `==`.
         * Both are answered by the ordinary numeric path, which reads a BigInt as a double. */
        if (operation >= 1 && operation <= 6) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Cannot mix BigInt and other types, use explicit conversions");
        }
        *out = kiln_c99_binary(operation, left, right);
        return KILN_C99_OK;
    }
    a = (const void *)(uintptr_t)left.bits;
    b = (const void *)(uintptr_t)right.bits;
    switch (operation) {
    case 7: case 15: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) == 0); return KILN_C99_OK;
    case 8: case 16: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) != 0); return KILN_C99_OK;
    case 9:  *out = kiln_c99_boolean(kiln_bigint_compare(a, b) < 0); return KILN_C99_OK;
    case 10: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) <= 0); return KILN_C99_OK;
    case 11: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) > 0); return KILN_C99_OK;
    case 12: *out = kiln_c99_boolean(kiln_bigint_compare(a, b) >= 0); return KILN_C99_OK;
    /* The provider numbers its arithmetic 1..6 the same way; only the bitwise three differ. */
    case 1: case 2: case 3: case 4: case 5: case 6: provider_op = operation; break;
    case 17: provider_op = 7; break;
    case 18: provider_op = 8; break;
    case 19: provider_op = 9; break;
    default:
        /* `in`, `instanceof` and the shifts are not BigInt operators here; the numeric path
         * answers them the way it answers any other value. */
        *out = kiln_c99_binary(operation, left, right);
        return KILN_C99_OK;
    }
    status = kiln_bigint_binary(provider_op, a, b, &answer);
    if (status != 0) {
        /* Division truncates toward zero and by zero is a RangeError, neither of which matches
         * what a number would do. A negative exponent is the other one. */
        if (provider_op == 6) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "Exponent must be non-negative");
        }
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR, "Division by zero");
    }
    *out = kiln_c99_bigint_value(answer);
    return KILN_C99_OK;
}


"#;

/// The stack each emitted C function needs: the module body, then one entry per compiled
/// function body in [`Program::functions`], in the same order.
struct StackPlan {
    body: usize,
    functions: Vec<usize>,
}

fn validate(program: &Program) -> Result<usize, EmitError> {
    Ok(validate_program(program)?.body)
}

fn validate_program(program: &Program) -> Result<StackPlan, EmitError> {
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
    for proto in &program.functions {
        // A generator does not run when it is called: it builds an object that runs in pieces,
        // each resumption picking up where the last left off. That is a suspended frame, which
        // an automatic C frame cannot be. An `async` function is compiled as one for the same
        // reason -- `await` suspends exactly as `yield` does.
        // An async *generator* suspends for two different reasons -- a value for its consumer
        // and a promise it is waiting on -- and the driver has to tell them apart to know whether
        // the consumer's step is finished. `frame->awaiting`, written at each suspension, is what
        // says which; without it the two emit identically and nothing downstream could tell.
        if proto.parameter_count > proto.local_count {
            return Err(EmitError::InvalidProgram {
                message: format!(
                    "function takes {} parameter(s) but has only {} local slot(s)",
                    proto.parameter_count, proto.local_count
                ),
            });
        }
        if proto.code.is_empty() {
            return Err(EmitError::InvalidProgram {
                message: "function body has no instructions".into(),
            });
        }
    }
    let body = validate_code(&program.code, program, program.local_count, 0)?;
    let functions = program
        .functions
        .iter()
        .map(|proto| {
            validate_code(
                &proto.code,
                program,
                proto.local_count,
                proto.upvalues.len(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(StackPlan { body, functions })
}

/// Validate one body -- the module's or a function's -- and report the stack depth it reaches.
///
/// `local_count` and `upvalue_count` describe the frame this body runs in, which is why this
/// cannot simply read them off the program: every function has its own.
fn validate_code(
    code: &[Instruction],
    program: &Program,
    local_count: u32,
    upvalue_count: usize,
) -> Result<usize, EmitError> {
    for (index, instruction) in code.iter().enumerate() {
        validate_supported_instruction(instruction)?;
        match instruction {
            Instruction::Const(constant) if (*constant as usize) >= program.constants.len() => {
                return Err(invalid(
                    index,
                    format!("constant {constant} is out of bounds"),
                ));
            }
            // A property name is emitted by naming the constant's own C symbol, and only a
            // string constant has one.
            Instruction::GetProperty(name)
            | Instruction::SetProperty(name)
            | Instruction::DeleteProperty(name)
            | Instruction::CallMethod { name, .. } => match program.constants.get(*name as usize) {
                Some(Constant::String(_)) => {}
                Some(_) => {
                    return Err(invalid(
                        index,
                        format!("property name constant {name} is not a string"),
                    ));
                }
                None => {
                    return Err(invalid(
                        index,
                        format!("property name constant {name} is out of bounds"),
                    ));
                }
            },
            Instruction::LoadLocal(local)
            | Instruction::StoreLocal(local)
            | Instruction::NewCell(local)
            | Instruction::LoadCell(local)
            | Instruction::StoreCell(local)
            | Instruction::MarkUninitialized(local)
                if (*local as u64) >= u64::from(local_count) =>
            {
                return Err(invalid(index, format!("local {local} is out of bounds")));
            }
            Instruction::LoadGlobal(slot) | Instruction::StoreGlobal(slot)
                if u64::from(*slot) >= u64::from(program.local_count) =>
            {
                return Err(invalid(
                    index,
                    format!("module slot {slot} is out of bounds"),
                ));
            }
            Instruction::LoadUpvalue(upvalue) | Instruction::StoreUpvalue(upvalue)
                if (*upvalue as usize) >= upvalue_count =>
            {
                return Err(invalid(
                    index,
                    format!("upvalue {upvalue} is out of bounds"),
                ));
            }
            Instruction::Jump(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::PushHandler(target)
                if (*target as usize) >= code.len() =>
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
            // Where a closure's captures are checked: the descriptors name slots in *this*
            // frame, so this is the only place that knows whether they exist.
            Instruction::MakeFunction(proto) => {
                let Some(target) = program.functions.get(*proto as usize) else {
                    return Err(invalid(index, format!("function {proto} is out of bounds")));
                };
                for upvalue in &target.upvalues {
                    let ok = match upvalue {
                        Upvalue::ParentLocal(slot) => u64::from(*slot) < u64::from(local_count),
                        Upvalue::ParentUpvalue(slot) => (*slot as usize) < upvalue_count,
                    };
                    if !ok {
                        return Err(invalid(
                            index,
                            format!(
                                "function {proto} captures {upvalue:?}, which this frame does not have"
                            ),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    // Dataflow validation keeps generated stack indexing defined on every reachable branch and
    // calculates a compact allocation for the emitted function.
    let mut heights: Vec<Option<usize>> = vec![None; code.len()];
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
        let instruction = &code[index];
        let (next_height, terminal) = stack_effect(instruction, height, index)?;
        max_height = max_height.max(next_height);
        if terminal {
            continue;
        }
        match instruction {
            // A handler is entered with the thrown value pushed onto the stack as the enclosing
            // `try` left it, which is why the edge carries one more than the fall-through.
            Instruction::PushHandler(target) => {
                work.push((*target as usize, next_height + 1));
                if index + 1 < code.len() {
                    work.push((index + 1, next_height));
                } else {
                    return Err(invalid(index, "control falls off the end of the program"));
                }
            }
            Instruction::Jump(target) => work.push((*target as usize, next_height)),
            Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target) => {
                work.push((*target as usize, next_height));
                if index + 1 < code.len() {
                    work.push((index + 1, next_height));
                }
            }
            _ if index + 1 < code.len() => work.push((index + 1, next_height)),
            _ => {
                return Err(invalid(index, "control falls off the end of the program"));
            }
        }
    }
    Ok(max_height.max(1))
}

/// Whether the backend can emit this instruction.
///
/// There is no catch-all arm any more: **every instruction in the set is emitted**, and the match
/// is exhaustive so that adding one to `kiln-ir` stops this compiling until somebody decides what
/// the backend does about it. A refusal that has to be written down beats one that happens by
/// default -- and the default was the whole reason the tally in NOTES.md kept finding things.
///
/// A *builtin* can still be refused: `LoadBuiltin` names one value out of a large enum and the
/// arm below lists the ones that exist.
fn validate_supported_instruction(instruction: &Instruction) -> Result<(), EmitError> {
    match instruction {
        // A template's `${x}` lowers to a call to `String`, so this is not an optional extra:
        // without it the backend loses every program containing a template literal.
        Instruction::LoadBuiltin(
            kiln_ir::Builtin::String
            | kiln_ir::Builtin::Math
            | kiln_ir::Builtin::Number
            | kiln_ir::Builtin::Object
            | kiln_ir::Builtin::Array
            | kiln_ir::Builtin::Json
            | kiln_ir::Builtin::Date
            | kiln_ir::Builtin::Map
            | kiln_ir::Builtin::Set
            | kiln_ir::Builtin::Promise
            | kiln_ir::Builtin::Error
            | kiln_ir::Builtin::TypeError
            | kiln_ir::Builtin::RangeError
            | kiln_ir::Builtin::ReferenceError
            | kiln_ir::Builtin::SyntaxError
            | kiln_ir::Builtin::NotANumber
            | kiln_ir::Builtin::Infinity
            | kiln_ir::Builtin::Boolean
            | kiln_ir::Builtin::ParseInt
            | kiln_ir::Builtin::ParseFloat
            | kiln_ir::Builtin::IsNaN
            | kiln_ir::Builtin::IsFinite
            | kiln_ir::Builtin::Escape
            | kiln_ir::Builtin::Unescape
            | kiln_ir::Builtin::EncodeUri
            | kiln_ir::Builtin::EncodeUriComponent
            | kiln_ir::Builtin::DecodeUri
            | kiln_ir::Builtin::DecodeUriComponent
            | kiln_ir::Builtin::Reflect
            | kiln_ir::Builtin::QueueMicrotask
            | kiln_ir::Builtin::SetTimeout
            | kiln_ir::Builtin::SetInterval
            | kiln_ir::Builtin::ClearTimeout
            | kiln_ir::Builtin::ClearInterval
            | kiln_ir::Builtin::Symbol
            | kiln_ir::Builtin::RegExp
            | kiln_ir::Builtin::GlobalThis
            | kiln_ir::Builtin::BigInt
            | kiln_ir::Builtin::Function,
        ) => Ok(()),
        Instruction::LoadBuiltin(builtin) => Err(EmitError::Unsupported {
            feature: format!("the `{builtin:?}` builtin in the native backend"),
        }),
        Instruction::Const(_)
        | Instruction::LoadLocal(_)
        | Instruction::StoreLocal(_)
        | Instruction::NewCell(_)
        | Instruction::LoadCell(_)
        | Instruction::StoreCell(_)
        | Instruction::MakeFunction(_)
        | Instruction::Call { .. }
        | Instruction::LoadUpvalue(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::MarkUninitialized(_)
        | Instruction::NewObject { .. }
        | Instruction::GetProperty(_)
        | Instruction::SetProperty(_)
        | Instruction::NewArray { .. }
        | Instruction::GetComputed
        | Instruction::SetComputed
        | Instruction::CallMethod { .. }
        | Instruction::LoadThis
        | Instruction::PushHandler(_)
        | Instruction::PopHandler
        | Instruction::Throw
        | Instruction::ThrowReference(_)
        | Instruction::GetPrivate(_)
        | Instruction::SetPrivate(_)
        | Instruction::Dup2
        | Instruction::CallComputed { .. }
        | Instruction::NewRegExp { .. }
        | Instruction::New { .. }
        | Instruction::SetPrototype
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::DefineAccessor { .. }
        | Instruction::DefineMethod
        | Instruction::SpreadObject
        | Instruction::SpreadInto
        | Instruction::ConstructSpread
        | Instruction::IterableToArray
        | Instruction::EnumerateKeys
        | Instruction::DeleteProperty(_)
        | Instruction::DeleteComputed
        | Instruction::Apply
        | Instruction::RestArguments(_)
        | Instruction::PushHole
        | Instruction::Yield
        | Instruction::SuperCall { .. }
        | Instruction::SuperApply
        | Instruction::SuperForward
        | Instruction::LoadNewTarget
        | Instruction::LoadArguments
        | Instruction::Await
        | Instruction::Pop
        | Instruction::Dup
        | Instruction::Unary(_)
        | Instruction::Binary(_)
        | Instruction::Jump(_)
        | Instruction::JumpIfFalse(_)
        | Instruction::JumpIfTruthy(_)
        | Instruction::JumpIfNotNullish(_)
        | Instruction::HostCall { .. }
        | Instruction::Return => Ok(()),
    }
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
        Instruction::Const(_)
        | Instruction::LoadLocal(_)
        | Instruction::LoadCell(_)
        | Instruction::LoadUpvalue(_)
        | Instruction::LoadBuiltin(_)
        | Instruction::LoadGlobal(_)
        | Instruction::RestArguments(_)
        | Instruction::PushHole
        | Instruction::LoadArguments
        | Instruction::MakeFunction(_) => Ok((height + 1, false)),
        // A literal consumes what it is built from and leaves the container.
        Instruction::NewObject { count } => {
            let needed = usize::try_from(*count)
                .unwrap_or(usize::MAX)
                .saturating_mul(2);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        Instruction::NewArray { count } => {
            let needed = usize::try_from(*count).unwrap_or(usize::MAX);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The object is replaced by what was read; the key and value are consumed with it.
        Instruction::GetProperty(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        Instruction::GetComputed => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // A write leaves the value it wrote, which is what makes `a = b.c = d` work.
        Instruction::SetProperty(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        Instruction::SetComputed => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        Instruction::LoadThis => Ok((height + 1, false)),
        // The parent initialises the *existing* instance rather than allocating one, but it is
        // still a call and still leaves what it returned -- which the lowering pops, because
        // `super()` is a statement.
        Instruction::SuperCall { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The arguments come from the frame rather than the stack, so only the parent is here
        // and the result replaces it.
        Instruction::SuperForward => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // `new` consumes the callee and its arguments and leaves the instance, exactly as a
        // call does.
        Instruction::New { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The half and the key are consumed; the target stays, so a class body can define
        // several accessors on one object without reloading it.
        Instruction::DefineAccessor { .. } | Instruction::DefineMethod => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        // Replaces the value with an array of its own enumerable keys.
        Instruction::EnumerateKeys | Instruction::DeleteProperty(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        Instruction::DeleteComputed => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // `[this, callee, arguments] -> [result]`: the arity is the length of an array the
        // program computed, which is exactly what a spread means.
        Instruction::Apply => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
            }
        }
        // `super(...xs)`: the parent and the computed argument list come off, its result goes on.
        Instruction::SuperApply => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // Replaces the value with an array of what it yields.
        Instruction::IterableToArray => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The source is consumed and the target stays, the same shape `SetPrototype` uses.
        Instruction::SpreadObject | Instruction::SpreadInto => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The prototype is written into the target, which stays.
        Instruction::SetPrototype => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The receiver sits under the arguments, exactly where a callee would, and the result
        // replaces both -- so a method call and a plain call have the same shape here.
        Instruction::CallMethod { argc, .. } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The callee sits under its arguments, and the result replaces both.
        Instruction::Call { argc } => {
            let needed = usize::from(*argc) + 1;
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // Both write a slot in place and touch no stack.
        Instruction::NewCell(_) | Instruction::MarkUninitialized(_) => Ok((height, false)),
        // Registering and retiring a handler leave the stack alone; the *target* is reached with
        // one more value on it, which the dataflow walk below accounts for.
        Instruction::PushHandler(_) | Instruction::PopHandler => Ok((height, false)),
        // A yield consumes the value and leaves one behind: what the caller sends back becomes
        // the result of the `yield` expression. `await` is identical in the machine -- the two
        // differ only in what the driver does with the value, which is not the body's business.
        Instruction::Yield | Instruction::Await => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // The name is a constant rather than a stack value, and control leaves for a handler
        // exactly as a `throw` does.
        Instruction::ThrowReference(_) => Ok((height, true)),
        // A literal produces a fresh object, so it only ever grows the stack.
        Instruction::NewRegExp { .. } => Ok((height + 1, false)),
        Instruction::LoadNewTarget => Ok((height + 1, false)),
        Instruction::Dup2 => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height + 2, false))
            }
        }
        // The receiver, the key and the arguments are all consumed and the result replaces them.
        Instruction::CallComputed { argc } => {
            let needed = usize::from(*argc).saturating_add(2);
            if height < needed {
                Err(underflow(needed))
            } else {
                Ok((height - needed + 1, false))
            }
        }
        // The callee and the argument list are both consumed and the instance replaces them.
        Instruction::ConstructSpread => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // The object is replaced by what was read, exactly as `GetProperty` is.
        Instruction::GetPrivate(_) => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
            }
        }
        // ...and the value replaces both, exactly as `SetProperty` does: an assignment is an
        // expression and its value is what was assigned.
        Instruction::SetPrivate(_) => {
            if height < 2 {
                Err(underflow(2))
            } else {
                Ok((height - 1, false))
            }
        }
        // A throw consumes the value and does not fall through: control leaves for a handler.
        Instruction::Throw => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height - 1, true))
            }
        }
        Instruction::StoreLocal(_)
        | Instruction::StoreCell(_)
        | Instruction::StoreUpvalue(_)
        | Instruction::StoreGlobal(_)
        | Instruction::Pop => {
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
        // A return takes the top of the stack and abandons the frame, so anything under it is
        // simply gone. Residue is not a malformed program: `try { return 1 } finally { return 2 }`
        // reaches the second return with the first one's value still pending underneath, and the
        // interpreter has always truncated rather than complained. Requiring an empty stack here
        // rejected that shape outright, which is a refusal the language does not licence.
        Instruction::Return => Ok((height, true)),
    }
}

/// Whether any body in this program can produce a value the embedding boundary cannot carry.
///
/// The boundary conversions depend on it, and they are not free to emit unconditionally:
/// reaching `kiln_c99_text` from every entry point makes every unit reference the number
/// provider, which the graph link tests deliberately link without. It would also change the
/// bytes of a scalar unit, and those bytes are a cache key.
///
/// So the question is asked the other way round: a program whose every instruction is in the
/// original scalar set can only ever hold a scalar, and everything else is assumed to be able to
/// produce a reference. Listing what *cannot* rather than what can is what makes this safe to
/// extend -- a new instruction is covered the day it is added, where an allowlist would have
/// silently let its values cross unconverted. That is exactly what happened: the list named
/// `MakeFunction` and `LoadBuiltin`, so `console.log([1])` -- a program with neither -- handed
/// the host a raw array and failed with no diagnostic.
fn produces_references(program: &Program) -> bool {
    let scalar_only = |code: &[Instruction]| {
        code.iter().all(|instruction| {
            matches!(
                instruction,
                Instruction::Const(_)
                    | Instruction::LoadLocal(_)
                    | Instruction::StoreLocal(_)
                    | Instruction::LoadGlobal(_)
                    | Instruction::StoreGlobal(_)
                    | Instruction::NewCell(_)
                    | Instruction::LoadCell(_)
                    | Instruction::StoreCell(_)
                    | Instruction::MarkUninitialized(_)
                    | Instruction::Pop
                    | Instruction::Dup
                    | Instruction::Unary(_)
                    | Instruction::Binary(_)
                    | Instruction::Jump(_)
                    | Instruction::JumpIfFalse(_)
                    | Instruction::JumpIfTruthy(_)
                    | Instruction::JumpIfNotNullish(_)
                    | Instruction::HostCall { .. }
                    | Instruction::Return
            )
        })
    };
    // A BigInt is the exception to the list above: it makes an *operator* throw -- `1n + 1` is
    // a TypeError and `1n / 0n` a RangeError -- so a body holding one needs somewhere to put the
    // thrown value even when every instruction in it is scalar.
    program
        .constants
        .iter()
        .any(|constant| matches!(constant, Constant::BigInt(_)))
        || !scalar_only(&program.code)
        || !program
            .functions
            .iter()
            .all(|proto| scalar_only(&proto.code))
}

/// Whether anything in this program reaches module scope from inside a call.
///
/// A module binding outlives every call and is shared by all of them, so when one is read from a
/// function body it cannot live in the entry point's automatic frame -- that frame is gone by
/// the time an exported closure runs. It becomes a file-scope array instead, which is what a
/// per-module singleton is.
fn uses_module_scope(program: &Program) -> bool {
    let reaches = |code: &[Instruction]| {
        code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::LoadGlobal(_) | Instruction::StoreGlobal(_)
            )
        })
    };
    reaches(&program.code) || program.functions.iter().any(|proto| reaches(&proto.code))
}

/// Whether a body reads `this`.
///
/// The module entry point has no receiver parameter, so it declares one only when the body asks
/// for it -- and a module's `this` is undefined, because a module is strict and strict mode does
/// not substitute the global object.
fn uses_this(code: &[Instruction]) -> bool {
    code.iter()
        .any(|instruction| matches!(instruction, Instruction::LoadThis))
}

/// Local slots this body treats as cells without ever creating one.
///
/// For an import, whose cell belongs to the exporter and arrives through the linker. The
/// compiler always lowers an import to `LoadCell`, because an import is a live view of the
/// exporter's binding -- but hand-written bytecode need not, and a body that treats the slot as
/// a plain local has to keep getting a plain local. So this asks the body rather than assuming.
fn slots_read_as_cells(code: &[Instruction], functions: &[FunctionProto]) -> HashSet<u32> {
    let mut needed = HashSet::new();
    let mut created = HashSet::new();
    for instruction in code {
        match instruction {
            Instruction::LoadCell(slot) | Instruction::StoreCell(slot) => {
                needed.insert(*slot);
            }
            Instruction::NewCell(slot) => {
                created.insert(*slot);
            }
            // Capturing a slot is only possible if it holds a cell.
            Instruction::MakeFunction(proto) => {
                if let Some(target) = functions.get(*proto as usize) {
                    for upvalue in &target.upvalues {
                        if let Upvalue::ParentLocal(slot) = upvalue {
                            needed.insert(*slot);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    needed.retain(|slot| !created.contains(slot));
    needed
}

/// What a call site does when the callee threw: jump to this body's handler, or hand it on.
/// Where a thrown status goes from the site that produced it.
///
/// A body with a handler jumps to its own unwind. A body without one propagates, except at the
/// entry point, where there is nothing left to propagate to: the throw is uncaught, and the only
/// thing that can still be done with it is say so. That is what `kiln_uncaught` is for, and why
/// the entry is the one place a bare `return status;` would lose the program's own error.
fn unwind_or_return(handlers: &[u32], site: &ReturnSite<'_>) -> &'static str {
    if !handlers.is_empty() {
        "goto kiln_unwind;"
    } else if matches!(site, ReturnSite::Entry { .. }) {
        "goto kiln_uncaught;"
    } else {
        "return status;"
    }
}

/// The entry point's last word on a throw nothing caught.
///
/// The thrown value is converted here rather than handed over, because the embedding signature
/// carries one `kiln_value` and a reference must not cross it. Text is what survives, and text is
/// what a launcher can print.
fn render_uncaught(output: &mut String) {
    if !output.contains("goto kiln_uncaught;") {
        // No site can reach it, and an unused label is a warning in every compiler worth using.
        return;
    }
    output.push_str("kiln_uncaught:;\n");
    output.push_str("    *result = kiln_c99_text(*thrown);\n");
    output.push_str("    return KILN_C99_THROWN;\n");
}

/// The instruction indices a body registers as exception handlers.
///
/// The set, not the count: an unwind has to jump to one of them, and C has no computed goto, so
/// the emitted dispatch is a switch over exactly these labels.
fn handler_targets(code: &[Instruction]) -> Vec<u32> {
    let mut targets: Vec<u32> = code
        .iter()
        .filter_map(|instruction| match instruction {
            Instruction::PushHandler(target) => Some(*target),
            _ => None,
        })
        .collect();
    targets.sort_unstable();
    targets.dedup();
    targets
}

/// The local slots a body puts into the temporal dead zone.
fn dead_zone_slots(code: &[Instruction]) -> HashSet<u32> {
    code.iter()
        .filter_map(|instruction| match instruction {
            Instruction::MarkUninitialized(local) => Some(*local),
            _ => None,
        })
        .collect()
}

/// Emit one C function per compiled body, forward-declared so a closure made before a body is
/// rendered can still point at it.
fn render_functions(output: &mut String, program: &Program, plan: &StackPlan, symbol: &str) {
    for index in 0..program.functions.len() {
        writeln!(
            output,
            "static int {symbol}_fn_{index}(kiln_host *host, kiln_value *result, kiln_value *upvalues, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame);"
        )
        .unwrap();
    }
    for (index, proto) in program.functions.iter().enumerate() {
        if let Some(name) = &proto.name {
            writeln!(
                output,
                "static const char {symbol}_name_{index}[] = {};",
                c_string(name)
            )
            .unwrap();
        }
    }
    for (index, proto) in program.functions.iter().enumerate() {
        render_function(
            output,
            program,
            proto,
            &plan.functions,
            plan.functions[index],
            index,
            symbol,
        );
    }
}

/// Declare the handler stack a body needs, or nothing when it catches nothing.
fn render_handler_prologue(output: &mut String, handlers: &[u32], depth: usize, suspends: bool) {
    if handlers.is_empty() {
        return;
    }
    if suspends {
        // A generator's handler stack is in its frame rather than automatic, for exactly the
        // reason its operand stack is: a `try` has to still be catching after the `yield` inside
        // it returns. The macro is how the shared emission below reaches it without every site
        // learning which kind of body it is in.
        output.push_str("    size_t *kiln_handlers = frame->handlers;\n");
        output.push_str("    size_t *kiln_handler_stack = frame->handler_stack;\n");
        output.push_str("#define kiln_handler_top frame->handler_top\n");
        return;
    }
    writeln!(output, "    size_t kiln_handlers[{depth}];").unwrap();
    writeln!(output, "    size_t kiln_handler_stack[{depth}];").unwrap();
    output.push_str("    size_t kiln_handler_top = 0;\n");
}

/// The label a throw jumps to: take the innermost handler, restore the stack as the `try` left
/// it, push what was thrown, and enter the catch.
///
/// A body whose handler stack is empty has nothing to catch with, so the throw keeps travelling
/// -- which is the same path a body with no handlers at all takes.
fn render_unwind(output: &mut String, handlers: &[u32], suspends: bool, uncaught: bool) {
    if handlers.is_empty() {
        return;
    }
    output.push_str("kiln_unwind:;\n");
    output.push_str(if uncaught {
        "    if (kiln_handler_top == 0) goto kiln_uncaught;\n"
    } else {
        "    if (kiln_handler_top == 0) return KILN_C99_THROWN;\n"
    });
    output.push_str("    --kiln_handler_top;\n");
    output.push_str("    sp = kiln_handler_stack[kiln_handler_top];\n");
    output.push_str("    stack[sp++] = *thrown;\n");
    output.push_str("    switch (kiln_handlers[kiln_handler_top]) {\n");
    for target in handlers {
        writeln!(output, "    case {target}: goto kiln_label_{target};").unwrap();
    }
    output.push_str("    default: return KILN_C99_CONTROL_ERROR;\n    }\n");
    if suspends {
        // The macro stood in for a local that a suspending body does not have; it ends with the
        // body rather than leaking into the next one.
        output.push_str("#undef kiln_handler_top\n");
    }
}

fn render_function(
    output: &mut String,
    program: &Program,
    proto: &FunctionProto,
    frame_stacks: &[usize],
    max_stack: usize,
    index: usize,
    symbol: &str,
) {
    let locals = usize::try_from(proto.local_count).unwrap().max(1);
    // Only when the name is already a C identifier: it goes into a comment, and a name is
    // program text that could otherwise close one.
    if let Some(name) = proto.name.as_ref().filter(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
    }) {
        writeln!(output, "/* function {index}: {name} */").unwrap();
    }
    writeln!(
        output,
        "static int {symbol}_fn_{index}(kiln_host *host, kiln_value *result, kiln_value *upvalues, kiln_value receiver, const kiln_value *args, size_t argc, int depth, kiln_value *thrown, kiln_generator *frame) {{"
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n    size_t slot;\n");
    let handlers = handler_targets(&proto.code);
    // A generator and an async function both suspend, and every part of their state has to
    // outlive the call that stopped in the middle of them.
    let suspends = proto.generator || proto.asynchronous;
    let yields: Vec<usize> = proto
        .code
        .iter()
        .enumerate()
        .filter(|(_, instruction)| matches!(instruction, Instruction::Yield | Instruction::Await))
        .map(|(position, _)| position)
        .collect();
    if proto.generator || proto.asynchronous {
        // The frame outlives this call, so neither array can be automatic: both belong to the
        // generator, and every `locals[i]` and `stack[sp]` below reads through them unchanged.
        output.push_str("    kiln_value *stack = frame->stack;\n");
        output.push_str("    kiln_value *locals = frame->locals;\n");
        render_handler_prologue(output, &handlers, proto.code.len(), suspends);
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
        output.push_str("    (void)thrown;\n");
        output.push_str("    (void)host; (void)upvalues; (void)depth; (void)receiver;\n");
        output.push_str("    (void)args; (void)argc; (void)slot;\n");
        // Re-entry. A generator that has not started begins at its first instruction; one that
        // has jumps back to just after the `Yield` that suspended it, with the operand stack
        // restored to the height it had there.
        if !yields.is_empty() {
            output.push_str("    if (frame->started) {\n        sp = frame->sp;\n");
            // A rejected `await` arrives as a throw at the suspension point rather than as a
            // value, so it goes to the handler stack instead of to the resume label.
            if handlers.is_empty() {
                output.push_str(
                    "        if (frame->raising) { *thrown = frame->sent; return KILN_C99_THROWN; }\n",
                );
            } else {
                output.push_str(
                    "        if (frame->raising) { *thrown = frame->sent; goto kiln_unwind; }\n",
                );
            }
            output.push_str("        switch (frame->resume) {\n");
            for position in &yields {
                writeln!(
                    output,
                    "        case {position}: goto kiln_resume_{position};"
                )
                .unwrap();
            }
            output.push_str("        default: return KILN_C99_CONTROL_ERROR;\n        }\n    }\n");
        }
    } else {
        writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
        writeln!(output, "    kiln_value locals[{locals}];").unwrap();
        render_handler_prologue(output, &handlers, proto.code.len(), suspends);
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
        output.push_str("    (void)thrown; (void)frame;\n");
        output.push_str("    (void)host; (void)upvalues; (void)depth; (void)receiver;\n");
        writeln!(
            output,
            "    for (slot = 0; slot < (size_t){locals}; ++slot) locals[slot] = kiln_c99_undefined();"
        )
        .unwrap();
        if proto.parameter_count == 0 {
            output.push_str("    (void)args; (void)argc;\n");
        } else {
            // Parameters occupy the first slots in declaration order. An argument that was not
            // passed leaves its slot undefined, which is what a missing argument is.
            writeln!(
                output,
                "    for (slot = 0; slot < (size_t){} && slot < argc; ++slot) locals[slot] = args[slot];",
                proto.parameter_count
            )
            .unwrap();
        }
    }
    let emitter = Emitter {
        constants: &program.constants,
        frame_stacks,
        functions: &program.functions,
        symbol,
        callables: produces_references(program),
        handlers,
        site: ReturnSite::Body,
        guarded: dead_zone_slots(&proto.code),
    };
    for position in 0..proto.code.len() {
        writeln!(output, "kiln_label_{position}:;").unwrap();
        emit_instruction(output, &proto.code[position], position, &emitter);
    }
    render_unwind(output, &emitter.handlers, suspends, false);
    // A body whose control reaches the end without a `Return` is a compiler bug, not a program
    // error: the lowering always ends a body with one.
    output.push_str("    *result = kiln_c99_undefined();\n    return KILN_C99_CONTROL_ERROR;\n}\n");
}

/// The constant table.
///
/// A string is its bytes. A BigInt is its digits plus a slot for the handle the provider builds
/// from them on first use: parsing is not free and a literal's value never changes, so the unit
/// pays for it once however often the literal is loaded.
fn render_constant_declarations(output: &mut String, program: &Program, symbol: &str) {
    for (index, constant) in program.constants.iter().enumerate() {
        match constant {
            Constant::String(value) => writeln!(
                output,
                "static const char {symbol}_constant_{index}[] = {};",
                c_string(value)
            )
            .unwrap(),
            Constant::BigInt(digits) => {
                writeln!(
                    output,
                    "static const char {symbol}_constant_{index}[] = {};",
                    c_string(digits)
                )
                .unwrap();
                writeln!(output, "static void *{symbol}_bigint_{index} = NULL;").unwrap();
            }
            Constant::Undefined | Constant::Null | Constant::Boolean(_) | Constant::Number(_) => {}
        }
    }
}

fn render(program: &Program, program_hash: u64, symbol: &str) -> String {
    let plan = validate_program(program).expect("render called after validation");
    let max_stack = plan.body;
    let mut output = String::with_capacity(HEADER.len() + program.code.len() * 64);
    output.push_str(HEADER);
    writeln!(
        output,
        "/* Program hash: {program_hash:016x}; bytecode version: {BYTECODE_VERSION}. */"
    )
    .unwrap();
    render_constant_declarations(&mut output, program, symbol);
    if uses_module_scope(program) {
        writeln!(
            output,
            "static kiln_value {symbol}_globals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_functions(&mut output, program, &plan, symbol);
    writeln!(
        output,
        "int {symbol}(kiln_host *host, kiln_value *result) {{"
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n");
    if uses_this(&program.code) {
        output.push_str("    kiln_value receiver = kiln_c99_undefined();\n");
    }
    // The entry point owns the depth counter and the thrown value every body writes into. Both
    // are locals rather than parameters because the embedding signature is fixed and a reference
    // must not cross it: an uncaught throw leaves as text in `*result`.
    if produces_references(program) {
        output.push_str("    int depth = 0;\n");
        output.push_str("    kiln_value thrown_value = kiln_c99_undefined();\n");
        output.push_str("    kiln_value *thrown = &thrown_value;\n");
    }
    writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
    if uses_module_scope(program) {
        // Named rather than automatic: an exported closure reads module scope after the entry
        // point has returned, and a frame that has returned is not somewhere to read from.
        writeln!(output, "    kiln_value *locals = {symbol}_globals;").unwrap();
    } else {
        writeln!(
            output,
            "    kiln_value locals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_handler_prologue(
        &mut output,
        &handler_targets(&program.code),
        program.code.len(),
        false,
    );
    output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
    for local in 0..program.local_count {
        writeln!(output, "    locals[{local}] = kiln_c99_undefined();").unwrap();
    }
    let emitter = Emitter {
        constants: &program.constants,
        frame_stacks: &plan.functions,
        functions: &program.functions,
        symbol,
        callables: produces_references(program),
        handlers: handler_targets(&program.code),
        site: ReturnSite::Entry { exports: None },
        guarded: dead_zone_slots(&program.code),
    };
    for index in 0..program.code.len() {
        writeln!(output, "kiln_label_{index}:;").unwrap();
        emit_instruction(&mut output, &program.code[index], index, &emitter);
    }
    render_unwind(&mut output, &emitter.handlers, false, true);
    render_uncaught(&mut output);
    output.push_str("    *result = kiln_c99_undefined();\n    return KILN_C99_CONTROL_ERROR;\n}\n");
    output
}

fn render_module(module: &ModuleProgram, program_hash: u64, symbol: &str) -> String {
    let program = &module.program;
    let plan = validate_program(program).expect("render called after validation");
    let max_stack = plan.body;
    let mut output = String::with_capacity(HEADER.len() + program.code.len() * 72);
    output.push_str(HEADER);
    writeln!(
        output,
        "/* Module hash: {program_hash:016x}; bytecode version: {BYTECODE_VERSION}; imports: {}; exports: {}. */",
        module.imports.len(),
        module.exports.len()
    )
    .unwrap();
    render_constant_declarations(&mut output, program, symbol);
    if uses_module_scope(program) {
        writeln!(
            output,
            "static kiln_value {symbol}_globals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_functions(&mut output, program, &plan, symbol);
    writeln!(
        output,
        "int {symbol}(kiln_host *host, kiln_value *result, const kiln_value *imports, size_t import_count, kiln_value *exports, size_t export_count) {{"
    )
    .unwrap();
    output.push_str("    size_t sp = 0;\n");
    if uses_this(&program.code) {
        output.push_str("    kiln_value receiver = kiln_c99_undefined();\n");
    }
    // The entry point owns the depth counter and the thrown value every body writes into. Both
    // are locals rather than parameters because the embedding signature is fixed and a reference
    // must not cross it: an uncaught throw leaves as text in `*result`.
    if produces_references(program) {
        output.push_str("    int depth = 0;\n");
        output.push_str("    kiln_value thrown_value = kiln_c99_undefined();\n");
        output.push_str("    kiln_value *thrown = &thrown_value;\n");
    }
    writeln!(output, "    kiln_value stack[{}];", max_stack.max(1)).unwrap();
    if uses_module_scope(program) {
        // Named rather than automatic: an exported closure reads module scope after the entry
        // point has returned, and a frame that has returned is not somewhere to read from.
        writeln!(output, "    kiln_value *locals = {symbol}_globals;").unwrap();
    } else {
        writeln!(
            output,
            "    kiln_value locals[{}];",
            usize::try_from(program.local_count).unwrap().max(1)
        )
        .unwrap();
    }
    render_handler_prologue(
        &mut output,
        &handler_targets(&program.code),
        program.code.len(),
        false,
    );
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
    let boxed_imports = slots_read_as_cells(&program.code, &program.functions);
    for (index, binding) in module.imports.iter().enumerate() {
        let slot = binding.local_slot;
        writeln!(output, "    locals[{slot}] = imports[{index}];").unwrap();
        // The exporter boxes only what can change, so an immutable export arrives as a plain
        // value where the body expects a cell. Wrapping it here is the same thing
        // `execute_module` does on the interpreter side, and for the same reason: the body has
        // one way to read an import and the linker has two ways to deliver one.
        if boxed_imports.contains(&slot) {
            writeln!(
                output,
                "    if (locals[{slot}].tag != KILN_VALUE_CELL && kiln_c99_make_cell(&locals[{slot}]) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;"
            )
            .unwrap();
        }
    }
    let emitter = Emitter {
        constants: &program.constants,
        frame_stacks: &plan.functions,
        functions: &program.functions,
        symbol,
        callables: produces_references(program),
        handlers: handler_targets(&program.code),
        site: ReturnSite::Entry {
            exports: Some(&module.exports),
        },
        guarded: dead_zone_slots(&program.code),
    };
    for index in 0..program.code.len() {
        writeln!(output, "kiln_label_{index}:;").unwrap();
        emit_instruction(&mut output, &program.code[index], index, &emitter);
    }
    render_unwind(&mut output, &emitter.handlers, false, true);
    render_uncaught(&mut output);
    output.push_str("    *result = kiln_c99_undefined();\n    return KILN_C99_CONTROL_ERROR;\n}\n");
    output
}

/// Where a `Return` lands.
///
/// The entry point's value crosses the embedding boundary and a function body's does not, which
/// is the difference between converting a returned function to text and handing back the closure
/// so the caller can call it.
struct Emitter<'a> {
    constants: &'a [Constant],
    /// How deep each compiled body's operand stack can get, so a generator over it can allocate
    /// one that never overflows.
    frame_stacks: &'a [usize],
    /// The handler labels this body can unwind to, empty when it catches nothing. A body that
    /// catches nothing propagates a throw by returning it, which needs no machinery at all.
    handlers: Vec<u32>,
    functions: &'a [FunctionProto],
    symbol: &'a str,
    /// Whether a value reaching the embedding boundary could be a callable and so has to be
    /// converted first. False for a scalar program, whose emitted bytes must not change.
    callables: bool,
    site: ReturnSite<'a>,
    /// The local slots this body puts into the temporal dead zone, so a read of one is checked
    /// and every other read is not.
    guarded: HashSet<u32>,
}

enum ReturnSite<'a> {
    Entry {
        exports: Option<&'a [kiln_ir::ExportBinding]>,
    },
    Body,
}

fn emit_instruction(
    output: &mut String,
    instruction: &Instruction,
    index: usize,
    emitter: &Emitter<'_>,
) {
    let Emitter {
        constants,
        frame_stacks,
        functions,
        symbol,
        callables,
        handlers,
        site,
        guarded,
    } = emitter;
    match instruction {
        // Rejected by `verify` before emission begins, so reaching here means the two passes
        // disagree about the instruction set -- a compiler bug, not a program error.
        Instruction::LoadNewTarget => {
            output.push_str("    stack[sp++] = kiln_c99_new_target;\n");
        }
        Instruction::NewRegExp { pattern, flags } => {
            let (Constant::String(source), Constant::String(letters)) =
                (&constants[*pattern as usize], &constants[*flags as usize])
            else {
                unreachable!("verification rejects a non-string pattern before emission")
            };
            // A fresh object every time the literal is evaluated, because `lastIndex` is mutable
            // and program-visible: two evaluations of one literal must not share a cursor.
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ int status = kiln_c99_new_regexp(&stack[sp], {symbol}_constant_{pattern}, {}, {symbol}_constant_{flags}, {}, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; ++sp; }}",
                source.len(),
                letters.len()
            )
            .unwrap();
        }
        // The object and the key are read once and used twice, which two `Dup`s cannot do
        // because they would reverse them.
        Instruction::Dup2 => output
            .push_str("    stack[sp] = stack[sp - 2]; stack[sp + 1] = stack[sp - 1]; sp += 2;\n"),
        Instruction::CallComputed { argc } => {
            // Not a `GetComputed` followed by a `Call`: that would discard the receiver, and
            // `obj[name]()` has to run the method against `obj`. The same reason `CallMethod`
            // is one instruction rather than two.
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value method; kiln_value call_result; int status; kiln_value key = stack[sp - {argc} - 1]; kiln_value self_value = stack[sp - {argc} - 2]; const kiln_value *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; status = kiln_c99_get_computed(host, self_value, key, &method, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_call(host, &call_result, method, self_value, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
                argc + 2
            )
            .unwrap();
        }
        Instruction::ConstructSpread => {
            // `new f(...args)` is the same construction the fixed-arity `New` performs, so it
            // goes through the same path rather than repeating the allocate-run-override
            // sequence.
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value list = stack[--sp]; kiln_value callee = stack[--sp]; kiln_value built; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) return KILN_C99_CONTROL_ERROR; given = kiln_c99_array_of(list); status = kiln_c99_construct(host, &built, callee, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = built; }}"
            )
            .unwrap();
        }
        Instruction::GetPrivate(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_property *slot = kiln_c99_own_private(stack[sp - 1], {symbol}_constant_{name}, {}); if (slot == NULL) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {}, 0); {unwind} }} stack[sp - 1] = slot->value; }}",
                text.len(),
                text.len()
            )
            .unwrap();
        }
        Instruction::SetPrivate(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string private name before emission")
            };
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; kiln_property *slot = kiln_c99_own_private(stack[sp - 1], {symbol}_constant_{name}, {}); if (slot == NULL) {{ int status = kiln_c99_private_error(thrown, {symbol}_constant_{name}, {}, 1); {unwind} }} slot->value = member; stack[sp - 1] = member; }}",
                text.len(),
                text.len()
            )
            .unwrap();
        }
        Instruction::ThrowReference(name) => {
            // A name that is not there. Emitted rather than refused, because a reference in a
            // branch that never runs is not an error in JavaScript and real bundles are full of
            // them -- feature detection mentions globals the host may not have.
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string reference name before emission")
            };
            writeln!(
                output,
                "    {{ static const char kiln_missing_{index}[] = {}; int status = kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_REFERENCE_ERROR, kiln_missing_{index}); {} }}",
                c_string(&format!("{text} is not defined")),
                unwind_or_return(handlers, site)
            )
            .unwrap();
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
            Constant::BigInt(digits) => writeln!(
                output,
                "    {{ int status = kiln_c99_bigint_constant(&{symbol}_bigint_{constant}, {symbol}_constant_{constant}, {}, &stack[sp]); if (status != KILN_C99_OK) return status; ++sp; }}",
                digits.len()
            )
            .unwrap(),
        },
        // A cell is a shared box: the slot stops holding the value and starts holding a
        // pointer to it, so that a closure over the variable and the frame that declared it are
        // writing to the same place. Until closures existed here this was emitted as plain slot
        // access, which was sound only because nothing could capture.
        Instruction::NewCell(local) => {
            writeln!(
                output,
                "    {{ int status = kiln_c99_make_cell(&locals[{local}]); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        Instruction::LoadCell(local) => {
            if guarded.contains(local) {
                writeln!(
                    output,
                    "    if (kiln_c99_cell_of(locals[{local}])->tag == KILN_VALUE_UNINITIALIZED) return KILN_C99_UNINITIALIZED_ERROR;"
                )
                .unwrap();
            }
            writeln!(output, "    stack[sp++] = *kiln_c99_cell_of(locals[{local}]);").unwrap();
        }
        Instruction::StoreCell(local) => {
            writeln!(output, "    *kiln_c99_cell_of(locals[{local}]) = stack[--sp];").unwrap();
        }
        // A `let` or `const` slot is in the temporal dead zone until its declaration runs.
        // Reading one is an error rather than `undefined`, and only the slots a program marks
        // are ever tested -- the check would otherwise be a branch on every local read for a
        // state almost no program reaches.
        Instruction::MarkUninitialized(local) => {
            writeln!(output, "    kiln_c99_mark_uninitialized(&locals[{local}]);").unwrap();
        }
        // An upvalue slot always holds a cell -- that is what capturing means -- so unlike a
        // local it needs no test to know whether to read through it.
        Instruction::LoadUpvalue(upvalue) => {
            writeln!(output, "    stack[sp++] = *kiln_c99_cell_of(upvalues[{upvalue}]);").unwrap();
        }
        Instruction::StoreUpvalue(upvalue) => {
            writeln!(output, "    *kiln_c99_cell_of(upvalues[{upvalue}]) = stack[--sp];").unwrap();
        }
        Instruction::MakeFunction(proto) => {
            let target = &functions[*proto as usize];
            let captures = &target.upvalues;
            // A console prints `[Function: name]`, and an anonymous function is a distinct
            // answer rather than a missing one.
            let (name, name_len) = match &target.name {
                Some(text) => (format!("{symbol}_name_{proto}"), text.len()),
                None => ("NULL".to_owned(), 0),
            };
            // A generator's frame is sized here rather than at the call, because this is where
            // the body's own counts are in scope.
            let generator = i32::from(target.generator || target.asynchronous);
            let asynchronous = i32::from(target.asynchronous);
            let arrow = i32::from(target.arrow);
            let async_generator = i32::from(target.async_generator);
            let frame_locals = target.local_count.max(1);
            let frame_stack = frame_stacks[*proto as usize].max(1);
            // One handler slot per instruction: a body cannot register more handlers than it has
            // instructions, which is the same bound an automatic frame uses.
            let frame_handlers = target.code.len().max(1);
            output.push_str("    {\n        int status;\n");
            if captures.is_empty() {
                writeln!(
                    output,
                    "        status = kiln_c99_make_closure(&stack[sp], {symbol}_fn_{proto}, {name}, {name_len}, UINT32_C({proto}), {generator}, {asynchronous}, {async_generator}, {arrow}, UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), NULL, 0);"
                )
                .unwrap();
            } else {
                writeln!(output, "        kiln_value captured[{}];", captures.len()).unwrap();
                for (position, upvalue) in captures.iter().enumerate() {
                    // The box is copied by pointer, which is the point: two closures over one
                    // variable have to observe each other's writes.
                    match upvalue {
                        Upvalue::ParentLocal(slot) => {
                            writeln!(output, "        captured[{position}] = locals[{slot}];")
                        }
                        Upvalue::ParentUpvalue(slot) => {
                            writeln!(output, "        captured[{position}] = upvalues[{slot}];")
                        }
                    }
                    .unwrap();
                }
                writeln!(
                    output,
                    "        status = kiln_c99_make_closure(&stack[sp], {symbol}_fn_{proto}, {name}, {name_len}, UINT32_C({proto}), {generator}, {asynchronous}, {async_generator}, {arrow}, UINT32_C({frame_locals}), UINT32_C({frame_stack}), UINT32_C({frame_handlers}), captured, UINT32_C({}));",
                    captures.len()
                )
                .unwrap();
            }
            output.push_str("        if (status != KILN_C99_OK) return status;\n        ++sp;\n    }\n");
        }
        Instruction::Call { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value call_result; int status; const kiln_value *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; status = kiln_c99_call(host, &call_result, stack[sp - {argc} - 1], kiln_c99_undefined(), call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
                argc + 1
            )
            .unwrap();
        }
        Instruction::LoadLocal(local) => {
            if guarded.contains(local) {
                writeln!(
                    output,
                    "    if (locals[{local}].tag == KILN_VALUE_UNINITIALIZED) return KILN_C99_UNINITIALIZED_ERROR;"
                )
                .unwrap();
            }
            writeln!(output, "    stack[sp++] = locals[{local}];").unwrap();
        }
        Instruction::StoreLocal(local) => writeln!(output, "    locals[{local}] = stack[--sp];").unwrap(),
        // The keys are on the stack with their values, so the object is filled from the stack
        // and only then replaces it -- a key that failed to convert must not leave a half-built
        // object where the program can see it.
        Instruction::NewObject { count } => {
            let unwind = unwind_or_return(handlers, site);
            let count = usize::try_from(*count).unwrap();
            output.push_str("    {\n        kiln_value built; size_t pair; int status;\n");
            output.push_str("        status = kiln_c99_new_object(&built);\n");
            output.push_str("        if (status != KILN_C99_OK) return status;\n");
            if count > 0 {
                writeln!(
                    output,
                    "        for (pair = 0; pair < (size_t){count}; ++pair) {{ kiln_value key = stack[sp - (size_t){count} * 2 + pair * 2]; kiln_value member = stack[sp - (size_t){count} * 2 + pair * 2 + 1]; status = kiln_c99_set_computed(host, built, key, member, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
                )
                .unwrap();
                writeln!(output, "        sp -= (size_t){count} * 2;").unwrap();
            } else {
                output.push_str("        (void)pair;\n");
            }
            output.push_str("        stack[sp++] = built;\n    }\n");
        }
        Instruction::NewArray { count } => {
            let count = usize::try_from(*count).unwrap();
            output.push_str("    {\n        kiln_value built; size_t element; int status;\n");
            output.push_str("        status = kiln_c99_new_array(&built);\n");
            output.push_str("        if (status != KILN_C99_OK) return status;\n");
            if count > 0 {
                writeln!(
                    output,
                    "        for (element = 0; element < (size_t){count}; ++element) {{ status = kiln_c99_array_push(built, stack[sp - (size_t){count} + element]); if (status != KILN_C99_OK) return status; }}"
                )
                .unwrap();
                writeln!(output, "        sp -= (size_t){count};").unwrap();
            } else {
                output.push_str("        (void)element;\n");
            }
            output.push_str("        stack[sp++] = built;\n    }\n");
        }
        Instruction::GetProperty(name) => {
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ kiln_value read; int status = kiln_c99_get(host, stack[sp - 1], {symbol}_constant_{name}, {}, &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::SetProperty(name) => {
            let unwind = unwind_or_return(handlers, site);
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            // The written value stays on the stack, which is what makes `a = b.c = d` work.
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; int status = kiln_c99_put(host, stack[sp - 1], {symbol}_constant_{name}, {}, member, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = member; }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::GetComputed => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value key = stack[--sp]; kiln_value read; int status = kiln_c99_get_computed(host, stack[sp - 1], key, &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}"
            )
            .unwrap();
        }
        Instruction::SetComputed => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value member = stack[--sp]; kiln_value key = stack[--sp]; int status = kiln_c99_set_computed(host, stack[sp - 1], key, member, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = member; }}"
            )
            .unwrap();
        }
        Instruction::LoadThis => output.push_str("    stack[sp++] = receiver;\n"),
        // Module scope is one array for the whole module, so a read from a function body names
        // it directly rather than reaching for a frame that may no longer exist.
        Instruction::LoadGlobal(slot) => {
            if guarded.contains(slot) {
                writeln!(
                    output,
                    "    if ({symbol}_globals[{slot}].tag == KILN_VALUE_UNINITIALIZED) return KILN_C99_UNINITIALIZED_ERROR;"
                )
                .unwrap();
            }
            writeln!(output, "    stack[sp++] = {symbol}_globals[{slot}];").unwrap();
        }
        Instruction::StoreGlobal(slot) => {
            writeln!(output, "    {symbol}_globals[{slot}] = stack[--sp];").unwrap();
        }
        Instruction::Binary(BinaryOp::InstanceOf) => {
            output.push_str(
                "    { kiln_value constructor = stack[--sp]; int answer; int status = kiln_c99_instance_of(stack[sp - 1], constructor, &answer); if (status != KILN_C99_OK) return status; stack[sp - 1] = kiln_c99_boolean(answer); }\n",
            );
        }
        // Both halves of `super` are an ordinary call with `this` threaded through: the parent
        // initialises the instance that already exists rather than building one, which is the
        // whole difference between `super()` and `new Parent()`.
        Instruction::SuperCall { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value produced; int status; const kiln_value *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; kiln_c99_inheriting_new_target = 1; status = kiln_c99_call(host, &produced, stack[sp - {argc} - 1], receiver, call_args, (size_t){argc}, depth, thrown); kiln_c99_inheriting_new_target = 0; if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = produced; }}",
                argc + 1
            )
            .unwrap();
        }
        // A derived class with no constructor behaves as `constructor(...args) { super(...args) }`.
        // Its arity is not known where it is synthesized, so the forwarding happens here from
        // whatever the frame was actually given.
        Instruction::SuperForward => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value produced; int status; kiln_c99_inheriting_new_target = 1; status = kiln_c99_call(host, &produced, stack[sp - 1], receiver, argc == 0 ? NULL : args, argc, depth, thrown); kiln_c99_inheriting_new_target = 0; if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = produced; }}"
            )
            .unwrap();
        }
        Instruction::New { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value built; int status; const kiln_value *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; status = kiln_c99_construct(host, &built, stack[sp - {argc} - 1], call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = built; }}",
                argc + 1
            )
            .unwrap();
        }
        Instruction::DefineAccessor {
            setter,
            enumerable,
        } => {
            let half = if *setter { "setter" } else { "getter" };
            let other = if *setter { "getter" } else { "setter" };
            let enumerable = i32::from(*enumerable);
            writeln!(
                output,
                "    {{ kiln_value {half} = stack[--sp]; kiln_value key = stack[--sp]; kiln_value {other} = kiln_c99_undefined(); kiln_value name = kiln_c99_text(key); int status; if (name.tag != KILN_VALUE_STRING) return KILN_C99_CONTROL_ERROR; status = kiln_c99_define_accessor(stack[sp - 1], name.ptr, name.len, getter, setter, {enumerable}); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // A class's methods are not enumerated, which is the whole difference from the
        // `SetProperty` an object literal's methods lower to.
        Instruction::DefineMethod => {
            output.push_str(
                "    { kiln_value method = stack[--sp]; kiln_value key = stack[--sp]; kiln_value name = kiln_c99_text(key); if (name.tag != KILN_VALUE_STRING) return KILN_C99_CONTROL_ERROR; if (kiln_c99_define_method(stack[sp - 1], name.ptr, name.len, method) != KILN_C99_OK) return KILN_C99_CONTROL_ERROR; }\n",
            );
        }
        // `in` asks whether the property exists, not what it produces, so unlike a read it does
        // not call an accessor.
        Instruction::Binary(BinaryOp::In) => {
            output.push_str(
                "    { kiln_value container = stack[--sp]; kiln_value key = kiln_c99_text(stack[sp - 1]); int answer; if (key.tag != KILN_VALUE_STRING) return KILN_C99_CONTROL_ERROR; if (kiln_c99_has_property(container, key.ptr, key.len, &answer) != KILN_C99_OK) return KILN_C99_CONTROL_ERROR; stack[sp - 1] = kiln_c99_boolean(answer); }\n",
            );
        }
        Instruction::DeleteProperty(name) => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string property name before emission")
            };
            writeln!(
                output,
                "    {{ int answer; if (kiln_c99_delete(stack[sp - 1], {symbol}_constant_{name}, {}, &answer) != KILN_C99_OK) return KILN_C99_CONTROL_ERROR; stack[sp - 1] = kiln_c99_boolean(answer); }}",
                text.len()
            )
            .unwrap();
        }
        Instruction::DeleteComputed => {
            output.push_str(
                "    { kiln_value key = kiln_c99_text(stack[--sp]); int answer; if (key.tag != KILN_VALUE_STRING) return KILN_C99_CONTROL_ERROR; if (kiln_c99_delete(stack[sp - 1], key.ptr, key.len, &answer) != KILN_C99_OK) return KILN_C99_CONTROL_ERROR; stack[sp - 1] = kiln_c99_boolean(answer); }\n",
            );
        }
        // `arguments` is every argument this frame was given, as an array. A real one is an
        // exotic object that aliases the parameters; a copy is what almost every use of it wants,
        // and the aliasing was removed from the language in strict mode anyway.
        // Not `args` and `argc`: inside an arrow those are the arrow's own, and `arguments`
        // there is the enclosing function's -- a different list entirely.
        Instruction::LoadArguments => {
            output.push_str(
                "    { kiln_value given; int status = kiln_c99_visible_arguments(&given); if (status != KILN_C99_OK) return status; stack[sp++] = given; }\n",
            );
        }
        // `...rest` is every argument past the named parameters, as an array. A generator's
        // arguments were copied into its frame when it was built, so there are none here.
        Instruction::RestArguments(from) => {
            writeln!(
                output,
                "    {{ kiln_value rest; size_t extra; int status = kiln_c99_new_array(&rest); if (status != KILN_C99_OK) return status; for (extra = (size_t){from}; extra < argc; ++extra) {{ status = kiln_c99_array_push(rest, args[extra]); if (status != KILN_C99_OK) return status; }} stack[sp++] = rest; }}"
            )
            .unwrap();
        }
        // The marker for an element a literal left out. It never escapes into an expression:
        // `NewArray` is the only thing that consumes it.
        Instruction::PushHole => output.push_str("    stack[sp++] = kiln_c99_hole();\n"),
        Instruction::EnumerateKeys => {
            output.push_str(
                "    { kiln_value keys; if (kiln_c99_own_keys(stack[sp - 1], &keys) != KILN_C99_OK) return KILN_C99_CONTROL_ERROR; stack[sp - 1] = keys; }\n",
            );
        }
        // The arity comes from an array the program built, so the arguments are read out of it
        // rather than off the stack.
        Instruction::Apply => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value list = stack[--sp]; kiln_value callee = stack[--sp]; kiln_value self_value = stack[sp - 1]; kiln_value call_result; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) return KILN_C99_CONTROL_ERROR; given = kiln_c99_array_of(list); status = kiln_c99_call(host, &call_result, callee, self_value, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = call_result; }}"
            )
            .unwrap();
        }
        // `Apply`'s runtime argument list, but against the instance being built and keeping the
        // `new.target` of the call that is building it -- the two things `super()` adds.
        Instruction::SuperApply => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value list = stack[--sp]; kiln_value parent = stack[--sp]; kiln_value call_result; kiln_array *given; int status; if (list.tag != KILN_VALUE_ARRAY) return KILN_C99_CONTROL_ERROR; given = kiln_c99_array_of(list); kiln_c99_inheriting_new_target = 1; status = kiln_c99_call(host, &call_result, parent, receiver, given->count == 0 ? NULL : given->elements, given->count, depth, thrown); kiln_c99_inheriting_new_target = 0; if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = call_result; }}"
            )
            .unwrap();
        }
        Instruction::SpreadInto => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value source = stack[--sp]; int status = kiln_c99_iterate(host, stack[sp - 1], source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        Instruction::IterableToArray => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value built; int status = kiln_c99_new_array(&built); if (status != KILN_C99_OK) return status; status = kiln_c99_iterate(host, built, stack[sp - 1], depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = built; }}"
            )
            .unwrap();
        }
        Instruction::SpreadObject => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value source = stack[--sp]; int status = kiln_c99_spread_object(host, stack[sp - 1], source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        // `class D extends C` links the two prototype objects, which is what makes an instance
        // of `D` find a method of `C`.
        Instruction::SetPrototype => {
            output.push_str(
                "    { kiln_value prototype = stack[--sp]; int status = kiln_c99_set_prototype(stack[sp - 1], prototype); if (status != KILN_C99_OK) return status; }\n",
            );
        }
        // The stack height is recorded with the handler, because a `catch` starts from the
        // height the `try` began at however deep the expression that threw had got.
        Instruction::PushHandler(target) => {
            writeln!(
                output,
                "    kiln_handlers[kiln_handler_top] = {target}; kiln_handler_stack[kiln_handler_top] = sp; ++kiln_handler_top;"
            )
            .unwrap();
        }
        Instruction::PopHandler => output.push_str("    --kiln_handler_top;\n"),
        Instruction::Yield | Instruction::Await => {
            writeln!(
                output,
                "    frame->sp = sp - 1; frame->resume = {index}; frame->awaiting = {}; *result = stack[sp - 1]; return KILN_C99_YIELDED;",
                i32::from(matches!(instruction, Instruction::Await))
            )
            .unwrap();
            // Where `next` comes back to. What the caller sent becomes the value of the `yield`
            // expression, which is why the height here is one more than it was at the suspension.
            writeln!(output, "kiln_resume_{index}:;").unwrap();
            output.push_str("    stack[sp++] = frame->sent;\n");
        }
        Instruction::Throw => {
            output.push_str("    *thrown = stack[--sp];\n");
            // Nothing here can catch it, so it leaves immediately -- as a status from a body,
            // and as text through the uncaught epilogue from the entry point. Spelled out rather
            // than shared with `unwind_or_return`, whose propagating arm returns the `status` a
            // failed call left behind and there is no call here to have left one.
            output.push_str(if !handlers.is_empty() {
                "    goto kiln_unwind;\n"
            } else if matches!(site, ReturnSite::Entry { .. }) {
                "    goto kiln_uncaught;\n"
            } else {
                "    return KILN_C99_THROWN;\n"
            });
        }
        // Not a `GetProperty` followed by a `Call`: that would discard the receiver, and the
        // read has to yield the *unbound* function so `a.f === a.f` holds. The receiver stays on
        // the stack until the call, which is what lets both be true at once.
        Instruction::CallMethod { name, argc } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string method name before emission")
            };
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers, site);
            writeln!(
                output,
                "    {{ kiln_value method; kiln_value call_result; int status; kiln_value self_value = stack[sp - {argc} - 1]; const kiln_value *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; status = kiln_c99_get(host, self_value, {symbol}_constant_{name}, {}, &method, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_call(host, &call_result, method, self_value, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
                text.len(),
                argc + 1
            )
            .unwrap();
        }
        Instruction::LoadBuiltin(kiln_ir::Builtin::GlobalThis) => {
            // The one builtin that is built rather than named: it is a real object, because a
            // program writes to it.
            output.push_str(
                "    { int status = kiln_c99_global_object(&stack[sp]); if (status != KILN_C99_OK) return status; ++sp; }\n",
            );
        }
        Instruction::LoadBuiltin(builtin) => {
            // `NaN` and `Infinity` are values rather than callables, but they reach the program
            // the same way -- as a name the compiler resolves to something the runtime supplies.
            let pushed = match builtin {
                kiln_ir::Builtin::String => "kiln_c99_builtin(KILN_C99_BUILTIN_STRING)",
                kiln_ir::Builtin::Math => "kiln_c99_builtin(KILN_C99_BUILTIN_MATH)",
                kiln_ir::Builtin::Number => "kiln_c99_builtin(KILN_C99_BUILTIN_NUMBER)",
                kiln_ir::Builtin::Object => "kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT)",
                kiln_ir::Builtin::Array => "kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY)",
                kiln_ir::Builtin::Json => "kiln_c99_builtin(KILN_C99_BUILTIN_JSON)",
                kiln_ir::Builtin::Date => "kiln_c99_builtin(KILN_C99_BUILTIN_DATE)",
                kiln_ir::Builtin::Map => "kiln_c99_builtin(KILN_C99_BUILTIN_MAP)",
                kiln_ir::Builtin::Set => "kiln_c99_builtin(KILN_C99_BUILTIN_SET)",
                kiln_ir::Builtin::Promise => "kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE)",
                kiln_ir::Builtin::Error => "kiln_c99_builtin(KILN_C99_BUILTIN_ERROR)",
                kiln_ir::Builtin::TypeError => "kiln_c99_builtin(KILN_C99_BUILTIN_TYPE_ERROR)",
                kiln_ir::Builtin::RangeError => "kiln_c99_builtin(KILN_C99_BUILTIN_RANGE_ERROR)",
                kiln_ir::Builtin::ReferenceError => {
                    "kiln_c99_builtin(KILN_C99_BUILTIN_REFERENCE_ERROR)"
                }
                kiln_ir::Builtin::SyntaxError => "kiln_c99_builtin(KILN_C99_BUILTIN_SYNTAX_ERROR)",
                kiln_ir::Builtin::Boolean => "kiln_c99_builtin(KILN_C99_BUILTIN_BOOLEAN)",
                // A global function is a namespace method with no namespace: nothing is reached
                // through to get at it, so the method value is what the name resolves to.
                kiln_ir::Builtin::ParseInt => "kiln_c99_method(KILN_C99_METHOD_PARSE_INT)",
                kiln_ir::Builtin::ParseFloat => "kiln_c99_method(KILN_C99_METHOD_PARSE_FLOAT)",
                kiln_ir::Builtin::IsNaN => "kiln_c99_method(KILN_C99_METHOD_GLOBAL_IS_NAN)",
                kiln_ir::Builtin::IsFinite => "kiln_c99_method(KILN_C99_METHOD_GLOBAL_IS_FINITE)",
                kiln_ir::Builtin::Reflect => "kiln_c99_builtin(KILN_C99_BUILTIN_REFLECT)",
                kiln_ir::Builtin::Symbol => "kiln_c99_builtin(KILN_C99_BUILTIN_SYMBOL)",
                kiln_ir::Builtin::RegExp => "kiln_c99_builtin(KILN_C99_BUILTIN_REGEXP)",
                kiln_ir::Builtin::Function => "kiln_c99_builtin(KILN_C99_BUILTIN_FUNCTION)",
                kiln_ir::Builtin::BigInt => "kiln_c99_builtin(KILN_C99_BUILTIN_BIGINT)",
                kiln_ir::Builtin::SetTimeout => "kiln_c99_method(KILN_C99_METHOD_SET_TIMEOUT)",
                kiln_ir::Builtin::SetInterval => "kiln_c99_method(KILN_C99_METHOD_SET_INTERVAL)",
                kiln_ir::Builtin::ClearTimeout => {
                    "kiln_c99_method(KILN_C99_METHOD_CLEAR_TIMEOUT)"
                }
                kiln_ir::Builtin::ClearInterval => {
                    "kiln_c99_method(KILN_C99_METHOD_CLEAR_INTERVAL)"
                }
                kiln_ir::Builtin::QueueMicrotask => {
                    "kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK)"
                }
                kiln_ir::Builtin::Escape => "kiln_c99_method(KILN_C99_METHOD_ESCAPE)",
                kiln_ir::Builtin::Unescape => "kiln_c99_method(KILN_C99_METHOD_UNESCAPE)",
                kiln_ir::Builtin::EncodeUri => "kiln_c99_method(KILN_C99_METHOD_ENCODE_URI)",
                kiln_ir::Builtin::EncodeUriComponent => {
                    "kiln_c99_method(KILN_C99_METHOD_ENCODE_URI_COMPONENT)"
                }
                kiln_ir::Builtin::DecodeUri => "kiln_c99_method(KILN_C99_METHOD_DECODE_URI)",
                kiln_ir::Builtin::DecodeUriComponent => {
                    "kiln_c99_method(KILN_C99_METHOD_DECODE_URI_COMPONENT)"
                }
                kiln_ir::Builtin::NotANumber => "kiln_c99_number(0.0 / 0.0)",
                kiln_ir::Builtin::Infinity => "kiln_c99_number(1.0 / 0.0)",
                _ => unreachable!("verification rejects every other builtin before emission"),
            };
            writeln!(output, "    stack[sp++] = {pushed};").unwrap();
        }
        Instruction::Pop => output.push_str("    --sp;\n"),
        Instruction::Dup => output.push_str("    stack[sp] = stack[sp - 1]; ++sp;\n"),
        // A scalar program holds no reference to reduce, and has nowhere to put a thrown
        // value either, so its emitted bytes stay what they were.
        Instruction::Unary(operator) if !*callables => {
            writeln!(output, "    stack[sp - 1] = kiln_c99_unary({}, stack[sp - 1]);", unary_code(*operator)).unwrap()
        }
        Instruction::Unary(operator) => {
            let unwind = unwind_or_return(handlers, site);
            writeln!(output, "    {{ kiln_value answer; int status = kiln_c99_unary_checked({}, host, stack[sp - 1], &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = answer; }}", unary_code(*operator)).unwrap()
        }
        Instruction::Binary(operator) if !*callables => {
            // A scalar program has no BigInt in it and so no operator in it can throw. It also
            // has nowhere to put a thrown value -- the entry point declares that slot only when
            // something can reach it -- so this is the form that program has to keep.
            writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; stack[sp++] = kiln_c99_binary({}, left, right); }}", binary_code(*operator)).unwrap()
        }
        Instruction::Binary(operator) => {
            // An operator throws only when a BigInt is involved, and `kiln_c99_binary_checked`
            // hands every other case straight back to `kiln_c99_binary`.
            let unwind = unwind_or_return(handlers, site);
            writeln!(output, "    {{ kiln_value right = stack[--sp]; kiln_value left = stack[--sp]; kiln_value answer; int status = kiln_c99_binary_checked({}, host, left, right, &answer, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp++] = answer; }}", binary_code(*operator)).unwrap()
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
            // The embedding boundary carries only the five scalar tags, so a function argument
            // is reduced to its text before the host sees it -- which is also what the
            // interpreter prints for one.
            if *argc > 0 && *callables {
                writeln!(output, "    {{ size_t arg; for (arg = sp - {argc}; arg < sp; ++arg) if (kiln_c99_crosses_boundary(stack[arg])) stack[arg] = kiln_c99_inspect(stack[arg]); }}").unwrap();
            }
            writeln!(output, "    {{ kiln_value host_result; const kiln_value *args = {argc} == 0 ? NULL : &stack[sp - {argc}]; if (host == NULL || host->call == NULL || host->call(host->context, UINT16_C({}), UINT16_C({argc}), args, &host_result) != 0) return KILN_C99_HOST_ERROR; sp -= {argc}; stack[sp++] = host_result; }}", host_op_code(*op)).unwrap()
        }
        Instruction::Return => match site {
            ReturnSite::Body => output.push_str(
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp]; return KILN_C99_OK;\n",
            ),
            ReturnSite::Entry { exports: None } if !callables => output.push_str(
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp]; return KILN_C99_OK;\n",
            ),
            ReturnSite::Entry { exports: None } => output.push_str(
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp];\n    { int drained = kiln_c99_run_loop(host, depth, thrown); if (drained != KILN_C99_OK) return drained; }\n    if (kiln_c99_crosses_boundary(*result)) *result = kiln_c99_text(*result);\n    return KILN_C99_OK;\n",
            ),
            ReturnSite::Entry {
                exports: Some(exports),
            } => {
                output.push_str(
                    "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp];\n",
                );
                if *callables {
                    // Everything the body queued runs before the module is finished, which is
                    // what "after the synchronous part" means.
                    output.push_str(
                        "    { int drained = kiln_c99_run_loop(host, depth, thrown); if (drained != KILN_C99_OK) return drained; }\n",
                    );
                }
                if *callables {
                    output.push_str(
                        "    if (kiln_c99_crosses_boundary(*result)) *result = kiln_c99_text(*result);\n",
                    );
                }
                for (index, binding) in exports.iter().enumerate() {
                    // The slot is handed over exactly as it stands, box included. A mutable
                    // export *is* its cell: that is what makes an import a live view of the
                    // exporter's binding rather than a copy of the value it happened to hold
                    // when the exporter finished. Only an export that can change is boxed, so
                    // an immutable one arrives here as a plain value and stays one.
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
        UnaryOp::BitwiseNot => 5,
        UnaryOp::Void => 6,
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
        // Stable codes: a new operator takes the next free value.
        BinaryOp::In => 13,
        BinaryOp::InstanceOf => 14,
        BinaryOp::LooseEqual => 15,
        BinaryOp::LooseNotEqual => 16,
        BinaryOp::BitwiseAnd => 17,
        BinaryOp::BitwiseOr => 18,
        BinaryOp::BitwiseXor => 19,
        BinaryOp::ShiftLeft => 20,
        BinaryOp::ShiftRight => 21,
        BinaryOp::UnsignedShiftRight => 22,
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
            // Tag 5, appended: a persisted key never renumbers.
            Constant::BigInt(digits) => {
                byte(&mut hash, 5);
                word(&mut hash, digits.len() as u64);
                for digit in digits.bytes() {
                    byte(&mut hash, digit);
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
            Instruction::CallMethod { name, argc } => {
                byte(&mut hash, 31);
                word(&mut hash, u64::from(*name));
                word(&mut hash, u64::from(*argc));
            }
            Instruction::CallComputed { argc } => {
                byte(&mut hash, 32);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::SuperForward => byte(&mut hash, 36),
            Instruction::SetPrototype => byte(&mut hash, 34),
            Instruction::SuperCall { argc } => {
                byte(&mut hash, 35);
                word(&mut hash, u64::from(*argc));
            }
            Instruction::PushHandler(target) => {
                byte(&mut hash, 38);
                word(&mut hash, u64::from(*target));
            }
            Instruction::PopHandler => byte(&mut hash, 39),
            Instruction::Yield => byte(&mut hash, 42),
            Instruction::Await => byte(&mut hash, 60),
            Instruction::Throw => byte(&mut hash, 40),
            Instruction::LoadArguments => byte(&mut hash, 37),
            Instruction::RestArguments(count) => {
                byte(&mut hash, 58);
                word(&mut hash, u64::from(*count));
            }
            Instruction::LoadThis => byte(&mut hash, 33),
            Instruction::LoadNewTarget => byte(&mut hash, 59),
            Instruction::Dup2 => byte(&mut hash, 27),
            Instruction::IterableToArray => byte(&mut hash, 44),
            Instruction::SpreadInto => byte(&mut hash, 41),
            Instruction::SpreadObject => byte(&mut hash, 45),
            Instruction::DeleteComputed => byte(&mut hash, 47),
            Instruction::Apply => byte(&mut hash, 48),
            Instruction::SuperApply => byte(&mut hash, 61),
            Instruction::EnumerateKeys => byte(&mut hash, 50),
            Instruction::PushHole => byte(&mut hash, 55),
            Instruction::ThrowReference(value) => {
                byte(&mut hash, 57);
                word(&mut hash, u64::from(*value));
            }
            Instruction::NewRegExp { pattern, flags } => {
                byte(&mut hash, 56);
                word(&mut hash, u64::from(*pattern));
                word(&mut hash, u64::from(*flags));
            }
            Instruction::GetPrivate(value) => {
                byte(&mut hash, 53);
                word(&mut hash, u64::from(*value));
            }
            Instruction::SetPrivate(value) => {
                byte(&mut hash, 54);
                word(&mut hash, u64::from(*value));
            }
            Instruction::DefineAccessor {
                setter,
                enumerable,
            } => {
                byte(&mut hash, 52);
                byte(&mut hash, u8::from(*setter));
                byte(&mut hash, u8::from(*enumerable));
            }
            Instruction::DefineMethod => byte(&mut hash, 43),
            Instruction::MarkUninitialized(value) => {
                byte(&mut hash, 51);
                word(&mut hash, u64::from(*value));
            }
            Instruction::ConstructSpread => byte(&mut hash, 49),
            Instruction::DeleteProperty(value) => {
                byte(&mut hash, 46);
                word(&mut hash, u64::from(*value));
            }
            Instruction::NewArray { count } => {
                byte(&mut hash, 28);
                word(&mut hash, u64::from(*count));
            }
            Instruction::GetComputed => byte(&mut hash, 29),
            Instruction::SetComputed => byte(&mut hash, 30),
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
    // Functions are a flat, stable table.  Keep the table marker and count even when it is
    // empty: otherwise an empty table and a stream ending at local_count could be confused by
    // future extensions.  MakeFunction operands are table indices, so hashing the table in
    // order also handles cyclic references without recursive traversal.
    byte(&mut hash, 0x60);
    word(&mut hash, program.functions.len() as u64);
    for function in &program.functions {
        hash_function_proto(&mut hash, function);
    }
    hash
}

fn hash_function_proto(hash: &mut u64, function: &FunctionProto) {
    fn byte(hash: &mut u64, value: u8) {
        *hash ^= u64::from(value);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    fn word(hash: &mut u64, value: u64) {
        for value in value.to_le_bytes() {
            byte(hash, value);
        }
    }

    byte(hash, 0x61);
    byte(hash, u8::from(function.asynchronous));
    match &function.name {
        Some(name) => {
            byte(hash, 1);
            word(hash, name.len() as u64);
            for value in name.bytes() {
                byte(hash, value);
            }
        }
        None => byte(hash, 0),
    }
    byte(hash, u8::from(function.arrow));
    byte(hash, u8::from(function.generator));
    // Appended after `generator`, which is where a new flag has to go: the bytes before it are a
    // persisted cache key and cannot move.
    byte(hash, u8::from(function.async_generator));
    word(hash, u64::from(function.parameter_count));
    word(hash, u64::from(function.local_count));

    // Reuse the canonical instruction encoder, including its persisted tag assignments. Seed it
    // with the hash accumulated so far and retain the resulting state directly: reducing the
    // function body to an independently seeded 64-bit digest here would introduce an avoidable
    // collision boundary inside the program identity. The synthetic program's empty function
    // table is also what makes numeric MakeFunction cycles terminate without recursive hashing.
    byte(hash, 0x62);
    *hash = hash_program_with_seed(
        &Program {
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: function.code.clone(),
            local_count: 0,
            functions: Vec::new(),
        },
        *hash,
    );

    byte(hash, 0x63);
    word(hash, function.upvalues.len() as u64);
    for upvalue in &function.upvalues {
        match upvalue {
            Upvalue::ParentLocal(index) => {
                byte(hash, 0);
                word(hash, u64::from(*index));
            }
            Upvalue::ParentUpvalue(index) => {
                byte(hash, 1);
                word(hash, u64::from(*index));
            }
        }
    }
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

    /// The engine half of the provider ABI, for tests that link the vendored C number runtime.
    ///
    /// `kiln-native-number`'s C is a *test fixture*: production links the Rust provider archive,
    /// which defines these. Tests here exercise the emitted C's own logic and none of them
    /// compiles a pattern or does arbitrary-precision arithmetic -- so a fixture that refuses
    /// every one is honest, and a test that did need one would get a visible error rather than a
    /// quiet wrong answer. Such a test belongs in `conformance/native`, where the real archive is
    /// linked.
    const REGEXP_FIXTURE: &str = r#"
int32_t kiln_regexp_compile(const unsigned char *pattern, size_t pattern_len,
                            const unsigned char *flags, size_t flags_len, void **out_handle) {
    (void)pattern; (void)pattern_len; (void)flags; (void)flags_len; (void)out_handle;
    return -1;
}
void kiln_regexp_free(void *handle) { (void)handle; }
int32_t kiln_regexp_search(void *handle, const uint16_t *subject, size_t subject_len, size_t start,
                           int64_t *out_ranges, size_t out_capacity) {
    (void)handle; (void)subject; (void)subject_len; (void)start; (void)out_ranges;
    (void)out_capacity;
    return -1;
}
size_t kiln_regexp_group_count(const void *handle) { (void)handle; return 0; }
int32_t kiln_regexp_group_name(const void *handle, size_t index, unsigned char *buffer,
                               size_t capacity) {
    (void)handle; (void)index; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_bigint_parse(const unsigned char *text, size_t len, uint32_t radix, void **out) {
    (void)text; (void)len; (void)radix; (void)out;
    return -1;
}
int32_t kiln_bigint_from_double(double value, void **out) { (void)value; (void)out; return -1; }
void kiln_bigint_free(void *handle) { (void)handle; }
double kiln_bigint_to_double(const void *handle) { (void)handle; return 0.0; }
int32_t kiln_bigint_to_string(const void *handle, uint32_t radix, unsigned char *buffer,
                              size_t capacity) {
    (void)handle; (void)radix; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_bigint_binary(uint32_t operation, const void *left, const void *right, void **out) {
    (void)operation; (void)left; (void)right; (void)out;
    return -1;
}
int32_t kiln_bigint_negate(const void *handle, void **out) { (void)handle; (void)out; return -1; }
int32_t kiln_bigint_compare(const void *left, const void *right) {
    (void)left; (void)right;
    return 0;
}
int32_t kiln_bigint_is_zero(const void *handle) { (void)handle; return 1; }
"#;

    fn linked_source(module_source: &str, wrapper: &str) -> String {
        format!(
            "{}\n{}\n{}\n{}",
            kiln_native_number::runtime_source(),
            REGEXP_FIXTURE,
            module_source,
            wrapper
        )
    }

    fn function(code: Vec<Instruction>) -> FunctionProto {
        FunctionProto {
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            parameter_count: 0,
            local_count: 0,
            code,
            upvalues: Vec::new(),
        }
    }

    #[test]
    fn program_hash_covers_function_table_and_proto_contents() {
        let base = program(Vec::new(), vec![Instruction::Return], 0);
        let with_function = |functions| Program {
            functions,
            ..base.clone()
        };
        let empty_hash = hash_program(&with_function(Vec::new()));
        let one_hash = hash_program(&with_function(vec![function(vec![Instruction::Return])]));
        assert_ne!(
            empty_hash, one_hash,
            "empty and nonempty tables must differ"
        );

        let mut changed_body = function(vec![Instruction::Return]);
        changed_body.code = vec![Instruction::Pop, Instruction::Return];
        assert_ne!(one_hash, hash_program(&with_function(vec![changed_body])));

        for change in [
            FunctionProto {
                asynchronous: true,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                name: Some("named".into()),
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                name: Some(String::new()),
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                arrow: true,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                generator: true,
                async_generator: false,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                parameter_count: 1,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                local_count: 1,
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                upvalues: vec![Upvalue::ParentLocal(0)],
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                upvalues: vec![Upvalue::ParentUpvalue(0)],
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                upvalues: vec![Upvalue::ParentLocal(1)],
                ..function(vec![Instruction::Return])
            },
            FunctionProto {
                upvalues: vec![Upvalue::ParentUpvalue(1)],
                ..function(vec![Instruction::Return])
            },
        ] {
            assert_ne!(one_hash, hash_program(&with_function(vec![change])));
        }

        // Table order is semantic because MakeFunction stores stable table indices.  Referencing
        // each entry from the other function also exercises cyclic references without recursion.
        let first = function(vec![Instruction::MakeFunction(1), Instruction::Return]);
        let second = function(vec![Instruction::MakeFunction(0), Instruction::Return]);
        let cyclic = hash_program(&with_function(vec![first.clone(), second.clone()]));
        let reordered = hash_program(&with_function(vec![second, first]));
        assert_ne!(cyclic, reordered);
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
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
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
    fn generated_numeric_conversion_ignores_the_embedding_process_locale() {
        let program = program(
            vec![
                Constant::String("1.5".into()),
                Constant::Number(0.0_f64.to_bits()),
                Constant::String("value=".into()),
                Constant::Number(1.5_f64.to_bits()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::Subtract),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Pop,
                Instruction::Const(2),
                Instruction::Const(3),
                Instruction::Binary(BinaryOp::Add),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            0,
        );
        let generated = emit(&program).expect("locale probe emits");
        assert!(
            generated
                .source
                .contains("kiln_string_to_number(value.ptr, value.len)")
        );
        // A *call*, matching the printf check below. Both names still appear in the runtime's
        // prose, where they are naming the things that must not be reached for.
        assert!(!generated.source.contains("strtod("));
        assert!(!generated.source.contains("kiln_c99_strtod_c"));
        assert!(generated.source.contains("KILN_NUMBER_FORMAT_VERSION 1"));
        assert!(generated.source.contains("KILN_NUMBER_PARSE_VERSION 1"));
        assert!(
            generated
                .source
                .contains("kiln_number_to_js_string(number, buffer, sizeof(buffer))")
        );
        assert!(!generated.source.contains("snprintf("));
        assert!(!generated.source.contains("setlocale("));
        let wrapper = format!(
            r#"
#include <locale.h>

static int calls = 0;

static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL)
        return KILN_C99_HOST_ERROR;
    if (calls == 0) {{
        if (arguments[0].tag != KILN_VALUE_NUMBER ||
            arguments[0].bits != UINT64_C({number_bits}))
            return KILN_C99_HOST_ERROR;
    }} else if (calls == 1) {{
        static const char expected[] = "value=1.5";
        if (arguments[0].tag != KILN_VALUE_STRING || arguments[0].ptr == NULL ||
            arguments[0].len != sizeof(expected) - 1 ||
            memcmp(arguments[0].ptr, expected, sizeof(expected) - 1) != 0)
            return KILN_C99_HOST_ERROR;
    }} else {{
        return KILN_C99_HOST_ERROR;
    }}
    ++calls;
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
}}

int main(void) {{
    static const char *candidates[] = {{
        "de_DE.UTF-8", "de_DE.utf8", "de_DE",
        "fr_FR.UTF-8", "fr_FR.utf8", "fr_FR",
        "it_IT.UTF-8", "es_ES.UTF-8", "pt_BR.UTF-8"
    }};
    kiln_host host = {{ host_call, NULL }};
    kiln_value result;
    size_t index;
    int found = 0;
    for (index = 0; index < sizeof(candidates) / sizeof(candidates[0]); ++index) {{
        struct lconv *conventions;
        if (setlocale(LC_NUMERIC, candidates[index]) == NULL) continue;
        conventions = localeconv();
        if (conventions != NULL && strcmp(conventions->decimal_point, ",") == 0) {{
            found = 1;
            break;
        }}
    }}
    if (!found) return 77;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    return calls == 2 ? 0 : 3;
}}
"#,
            number_bits = 1.5_f64.to_bits(),
            symbol = generated.symbol,
        );
        let output_path =
            std::env::temp_dir().join(format!("kiln-backend-c-locale-{}", std::process::id()));
        let mut child = Command::new("cc")
            .args(["-std=c11", "-x", "c", "-"])
            .arg("-lm")
            .arg("-o")
            .arg(&output_path)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a C compiler is available for locale test");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("locale source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "locale-independent generated C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&output_path)
            .output()
            .expect("locale probe runs");
        let _ = std::fs::remove_file(output_path);
        if run.status.code() == Some(77) {
            eprintln!("skipping hostile-locale execution: no comma-decimal locale is installed");
            return;
        }
        assert!(
            run.status.success(),
            "generated numeric conversion inherited the embedding locale: {:?}",
            run.status.code()
        );
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
    fn executable_scalar_operators_match_javascript() {
        fn push_constant(constants: &mut Vec<Constant>, value: Constant) -> u32 {
            let index = constants.len() as u32;
            constants.push(value);
            index
        }

        fn check_unary(
            constants: &mut Vec<Constant>,
            code: &mut Vec<Instruction>,
            failures: &mut Vec<usize>,
            operator: UnaryOp,
            input: Constant,
            expected: Constant,
        ) {
            let input = push_constant(constants, input);
            let expected = push_constant(constants, expected);
            code.extend([
                Instruction::Const(input),
                Instruction::Unary(operator),
                Instruction::Const(expected),
                Instruction::Binary(BinaryOp::StrictEqual),
            ]);
            failures.push(code.len());
            code.push(Instruction::JumpIfFalse(0));
        }

        fn check_binary(
            constants: &mut Vec<Constant>,
            code: &mut Vec<Instruction>,
            failures: &mut Vec<usize>,
            operator: BinaryOp,
            left: Constant,
            right: Constant,
            expected: Constant,
        ) {
            let left = push_constant(constants, left);
            let right = push_constant(constants, right);
            let expected = push_constant(constants, expected);
            code.extend([
                Instruction::Const(left),
                Instruction::Const(right),
                Instruction::Binary(operator),
                Instruction::Const(expected),
                Instruction::Binary(BinaryOp::StrictEqual),
            ]);
            failures.push(code.len());
            code.push(Instruction::JumpIfFalse(0));
        }

        let number = |value: f64| Constant::Number(value.to_bits());
        let mut constants = Vec::new();
        let mut code = Vec::new();
        let mut failures = Vec::new();

        for (input, expected) in [
            (Constant::Undefined, "undefined"),
            (Constant::Null, "object"),
            (Constant::Boolean(true), "boolean"),
            (number(1.0), "number"),
            (Constant::String("x".into()), "string"),
        ] {
            check_unary(
                &mut constants,
                &mut code,
                &mut failures,
                UnaryOp::TypeOf,
                input,
                Constant::String(expected.into()),
            );
        }
        check_unary(
            &mut constants,
            &mut code,
            &mut failures,
            UnaryOp::BitwiseNot,
            number(5.0),
            number(-6.0),
        );
        check_unary(
            &mut constants,
            &mut code,
            &mut failures,
            UnaryOp::BitwiseNot,
            number(4_294_967_296.0),
            number(-1.0),
        );
        check_unary(
            &mut constants,
            &mut code,
            &mut failures,
            UnaryOp::Void,
            number(42.0),
            Constant::Undefined,
        );

        for (left, right, expected) in [
            (Constant::Null, Constant::Undefined, true),
            (number(0.0), Constant::Boolean(false), true),
            (Constant::String("42".into()), number(42.0), true),
            (Constant::String(String::new()), number(0.0), true),
            (Constant::Null, number(0.0), false),
        ] {
            check_binary(
                &mut constants,
                &mut code,
                &mut failures,
                BinaryOp::LooseEqual,
                left,
                right,
                Constant::Boolean(expected),
            );
        }
        check_binary(
            &mut constants,
            &mut code,
            &mut failures,
            BinaryOp::LooseNotEqual,
            number(f64::NAN),
            number(f64::NAN),
            Constant::Boolean(true),
        );

        for (operator, left, right, expected) in [
            (BinaryOp::BitwiseAnd, 5.0, 3.0, 1.0),
            (BinaryOp::BitwiseOr, 5.0, 2.0, 7.0),
            (BinaryOp::BitwiseXor, 5.0, 3.0, 6.0),
            (BinaryOp::ShiftLeft, 1.0, 31.0, -2_147_483_648.0),
            (BinaryOp::ShiftLeft, 1.0, 32.0, 1.0),
            (BinaryOp::ShiftRight, -1.0, 1.0, -1.0),
            (BinaryOp::UnsignedShiftRight, -1.0, 1.0, 2_147_483_647.0),
            (BinaryOp::BitwiseOr, 4_294_967_296.0, 0.0, 0.0),
            (BinaryOp::BitwiseOr, -1.9, 0.0, -1.0),
            (BinaryOp::BitwiseOr, f64::INFINITY, 0.0, 0.0),
        ] {
            check_binary(
                &mut constants,
                &mut code,
                &mut failures,
                operator,
                number(left),
                number(right),
                number(expected),
            );
        }

        let success = push_constant(&mut constants, Constant::Boolean(true));
        code.extend([Instruction::Const(success), Instruction::Return]);
        let failure = code.len() as u32;
        let failed = push_constant(&mut constants, Constant::Boolean(false));
        code.extend([Instruction::Const(failed), Instruction::Return]);
        for instruction in failures {
            code[instruction] = Instruction::JumpIfFalse(failure);
        }

        let generated = emit(&program(constants, code, 0)).expect("scalar operators emit");
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-operators-{}-{}",
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
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("operator source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated operator C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("generated operator executable runs");
        let _ = std::fs::remove_file(&executable);
        assert!(
            run.status.success(),
            "generated scalar operator returned the wrong result"
        );
    }

    #[test]
    fn executable_concat_accepts_a_null_pointer_for_an_empty_host_string() {
        let generated = emit(&program(
            vec![Constant::String("x".into())],
            vec![
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 0,
                },
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::Add),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            0,
        ))
        .expect("empty host string program emits");
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-empty-host-string-{}-{}",
            std::process::id(),
            generated.program_hash
        ));
        let wrapper = format!(
            r#"
static int calls = 0;

static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || result == NULL) return KILN_C99_HOST_ERROR;
    if (calls == 0 && argc == 0) {{
        *result = kiln_c99_value(KILN_VALUE_STRING, 0, NULL, 0);
    }} else if (calls == 1 && argc == 1 && arguments != NULL &&
               arguments[0].tag == KILN_VALUE_STRING && arguments[0].len == 1 &&
               arguments[0].ptr != NULL && arguments[0].ptr[0] == 'x') {{
        *result = kiln_c99_undefined();
    }} else {{
        return KILN_C99_HOST_ERROR;
    }}
    ++calls;
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    return calls == 2 ? 0 : 3;
}}
"#,
            symbol = generated.symbol
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
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
            .expect("empty host string source is accepted by cc stdin");
        let compile = child.wait_with_output().expect("cc exits");
        assert!(
            compile.status.success(),
            "generated empty host string C did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("generated empty host string executable runs");
        let _ = std::fs::remove_file(&executable);
        assert!(run.status.success(), "empty host string concat failed");
    }

    /// Every binary operator emits now. The rule the two refused ones stood for still holds and
    /// is worth a test of its own: an instruction this backend cannot do must say so rather than
    /// emit a silent undefined, which is what `unreachable_unsupported_instructions` covers.
    #[test]
    fn the_reference_operators_emit_rather_than_answering_undefined() {
        for operator in [BinaryOp::In, BinaryOp::InstanceOf] {
            let result = emit(&program(
                vec![Constant::Null],
                vec![
                    Instruction::Const(0),
                    Instruction::Const(0),
                    Instruction::Binary(operator),
                    Instruction::Return,
                ],
                0,
            ));
            assert!(result.is_ok(), "{operator:?} should emit");
        }
    }

    #[test]
    fn unreachable_unsupported_instructions_are_refused_before_rendering() {
        // Every *instruction* is emitted now, so the thing that can still be refused is a
        // builtin -- which is the same question about reachability with the one operand that has
        // any left to refuse.
        let result = emit(&program(
            vec![Constant::Boolean(true)],
            vec![
                Instruction::Jump(2),
                Instruction::LoadBuiltin(kiln_ir::Builtin::Proxy),
                Instruction::Const(0),
                Instruction::Return,
            ],
            0,
        ));
        assert!(
            matches!(result, Err(EmitError::Unsupported { feature }) if feature.contains("Proxy")),
            "dead code must not bypass the native builtin allowlist"
        );
    }

    /// Compile one emitted unit with a `main` of the caller's choosing and report its exit code.
    fn run_with_main(generated: &CModule, wrapper: &str, label: &str) -> std::process::Output {
        let executable = std::env::temp_dir().join(format!(
            "kiln-backend-c-{label}-{}-{}",
            std::process::id(),
            generated.program_hash
        ));
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
            .write_all(linked_source(&generated.source, wrapper).as_bytes())
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
        run
    }

    /// A closure over a module-level binding, which is the shape every counter has: the module
    /// body and the function must write through one box, and the module must still read the
    /// current value out of the slot afterwards.
    #[test]
    fn executable_closure_shares_a_cell_with_the_module_body() {
        let bump = FunctionProto {
            local_count: 0,
            upvalues: vec![Upvalue::ParentLocal(0)],
            code: vec![
                Instruction::LoadUpvalue(0),
                Instruction::Const(1),
                Instruction::Binary(BinaryOp::Add),
                Instruction::StoreUpvalue(0),
                Instruction::LoadUpvalue(0),
                Instruction::Return,
            ],
            ..function(Vec::new())
        };
        let generated = emit(&Program {
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::Number(0f64.to_bits()),
                Constant::Number(1f64.to_bits()),
            ],
            code: vec![
                Instruction::NewCell(0),
                Instruction::Const(0),
                Instruction::StoreCell(0),
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(1),
                Instruction::LoadLocal(1),
                Instruction::Call { argc: 0 },
                Instruction::Pop,
                Instruction::LoadLocal(1),
                Instruction::Call { argc: 0 },
                Instruction::Pop,
                // The module reads the shared box, which must now hold 2.
                Instruction::LoadCell(0),
                Instruction::Return,
            ],
            local_count: 2,
            functions: vec![bump],
        })
        .expect("program emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; if (result.tag != KILN_VALUE_NUMBER) return 3; return kiln_c99_as_number(result) == 2.0 ? 0 : 4; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "closure-cell");
        assert!(
            run.status.success(),
            "a closure and its module body did not share one cell (exit {:?})",
            run.status.code()
        );
    }

    /// Recursion is bounded rather than left to exhaust the C stack, which aborts the process
    /// instead of reporting anything. It reports as a *thrown* `RangeError` rather than a
    /// status, because that is what a real engine raises and what a program that recurses on its
    /// input has to be able to catch.
    #[test]
    fn executable_runaway_recursion_reports_rather_than_crashing() {
        let forever = FunctionProto {
            local_count: 0,
            upvalues: vec![Upvalue::ParentLocal(0)],
            code: vec![
                Instruction::LoadUpvalue(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            ..function(Vec::new())
        };
        let generated = emit(&Program {
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::NewCell(0),
                Instruction::MakeFunction(0),
                Instruction::StoreCell(0),
                Instruction::LoadCell(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            local_count: 1,
            functions: vec![forever],
        })
        .expect("program emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; return {}(NULL, &result) == KILN_C99_THROWN ? 0 : 1; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "depth");
        assert!(
            run.status.success(),
            "unbounded recursion did not report a thrown RangeError (exit {:?})",
            run.status.code()
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
            .write_all(linked_source(&generated.source, &wrapper).as_bytes())
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
