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
    KILN_VALUE_DATE = 13
};

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
    /* Calling this builds a generator object instead of running the body. */
    int generator;
    /* What a generator's frame has to be big enough for, known when the body is emitted. */
    uint32_t local_count;
    uint32_t stack_size;
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
} kiln_object;

/* A prototype chain is built by the program, so its length is the program's business -- but the
 * walk is on the C stack, where running out aborts rather than reports. Bounded, like the
 * interpreter's. */
#define KILN_C99_MAX_PROTOTYPE_DEPTH 10000

/* An array's elements are dense and its `length` is derived, not stored: writing past the end
 * grows it and the gap reads as undefined.  Holes are not represented -- a program that can
 * observe the difference between a hole and an undefined element needs `in`, which this backend
 * refuses. */
typedef struct kiln_array {
    kiln_value *elements;
    size_t count;
    size_t capacity;
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
    kiln_value *locals;
    kiln_value *stack;
    size_t sp;
    /* Which `Yield` to come back to. Zero before the first `next`, when the body starts at its
     * beginning rather than resuming. */
    size_t resume;
    /* What `next(v)` passed in, which becomes the result of the `yield` that suspended. */
    kiln_value sent;
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

/* Internal link-time runtime contract. The final executable/shared library supplies this symbol
 * from kiln-native-number; it is not part of the public embedding ABI. */
int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
double kiln_string_to_number(const char *bytes, size_t len);


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
static int kiln_c99_truthy(kiln_value value);
static int kiln_c99_signbit(double value);
static int kiln_c99_strict_equal(kiln_value left, kiln_value right);
static int kiln_c99_is_callable(kiln_value value);
static int kiln_c99_is_reference(kiln_value value);
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
static int kiln_c99_array_push(kiln_value target, kiln_value value);
static int kiln_c99_prototype_object(kiln_value function, kiln_value *out);
static int kiln_c99_to_units(kiln_value value, uint16_t **out, size_t *count);
static kiln_value kiln_c99_from_units(const uint16_t *units, size_t count);
static const char *kiln_c99_method_name(uint64_t code);
static int64_t kiln_c99_days_from_civil(int64_t year, int64_t month, int64_t day);
static kiln_date_parts kiln_c99_date_parts(double time);
static double kiln_c99_date_time(kiln_value value);
static int kiln_c99_new_date(kiln_value *out, double time);
static int kiln_c99_iso_string(kiln_value *out, double time);
static int kiln_c99_date_to_string(kiln_value *out, double time);
static double kiln_c99_instant_from_parts(const double parts[7]);
static double kiln_c99_parse_date(const char *bytes, size_t len);
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
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_CONTROL_ERROR;
    }
    object = kiln_c99_object_of(target);
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
    if (frame->locals == NULL || frame->stack == NULL) {
        free(frame->locals);
        free(frame->stack);
        free(frame);
        return KILN_C99_OUT_OF_MEMORY;
    }
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
    case KILN_C99_METHOD_MAP: return "map";
    case KILN_C99_METHOD_FILTER: return "filter";
    case KILN_C99_METHOD_FOR_EACH: return "forEach";
    case KILN_C99_METHOD_REDUCE: return "reduce";
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
    default: return NULL;
    }
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
    return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_PUSH, KILN_C99_METHOD_REDUCE);
}

static uint64_t kiln_c99_string_method(const char *key, size_t key_len) {
    return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_STRING_SLICE,
                                  KILN_C99_METHOD_STRING_CONCAT);
}

/* A namespace's own methods, by which namespace it is: `Math.min` and `Number.isNaN` are
 * different functions and the ranges keep them apart. */
static uint64_t kiln_c99_namespace_method(uint64_t builtin, const char *key, size_t key_len) {
    switch (builtin) {
    case KILN_C99_BUILTIN_MATH:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_MATH_FLOOR,
                                      KILN_C99_METHOD_MATH_ATAN);
    case KILN_C99_BUILTIN_NUMBER:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_IS_INTEGER,
                                      KILN_C99_METHOD_IS_FINITE);
    case KILN_C99_BUILTIN_OBJECT:
        if (kiln_c99_same_name(key, key_len, "create")) {
            return KILN_C99_METHOD_OBJECT_CREATE;
        }
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_OBJECT_KEYS,
                                      KILN_C99_METHOD_HAS_OWN);
    case KILN_C99_BUILTIN_ARRAY:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_IS_ARRAY,
                                      KILN_C99_METHOD_ARRAY_OF);
    case KILN_C99_BUILTIN_JSON:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_JSON_STRINGIFY,
                                      KILN_C99_METHOD_JSON_PARSE);
    case KILN_C99_BUILTIN_DATE:
        return kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_DATE_NOW,
                                      KILN_C99_METHOD_DATE_PARSE);
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
           value.tag == KILN_VALUE_METHOD;
}

/* Every reference value reduces to a primitive before a coercing operator, and none of them can
 * do it by calling: this backend has no user `valueOf` to reach, because a property holding a
 * function is refused at the point it would be called. So reduction is conversion to text. */
static int kiln_c99_is_reference(kiln_value value) {
    /* A Date is a reference, but it is *not* in this set: `as_number` already gives its instant,
     * which is what `valueOf` would produce, so reducing it to text first would turn every date
     * subtraction into NaN. */
    return kiln_c99_is_callable(value) || value.tag == KILN_VALUE_OBJECT ||
           value.tag == KILN_VALUE_ARRAY || value.tag == KILN_VALUE_GENERATOR;
}

/* Negative zero compares equal to zero, so the sign is the only thing that tells them apart --
 * and a console shows the difference where `String` does not. */
static int kiln_c99_signbit(double value) {
    return signbit(value) != 0;
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
                                 size_t name_len, uint32_t proto, int generator,
                                 uint32_t local_count, uint32_t stack_size,
                                 const kiln_value *captured, uint32_t count) {
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
    closure->local_count = local_count;
    closure->stack_size = stack_size;
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
        return KILN_C99_CONTROL_ERROR;
    }
    if (depth >= KILN_C99_MAX_CALL_DEPTH) {
        return KILN_C99_CALL_DEPTH_ERROR;
    }
    if (callee.tag == KILN_VALUE_METHOD) {
        return kiln_c99_invoke_method(host, result, callee.bits, receiver, args, argc, depth,
                                      thrown);
    }
    if (callee.tag == KILN_VALUE_BUILTIN) {
        static const char empty[] = "";
        switch (callee.bits) {
        case KILN_C99_BUILTIN_STRING:
            /* `String()` with no argument is the empty string, not `"undefined"`. */
            *result = argc == 0 ? kiln_c99_value(KILN_VALUE_STRING, 0, empty, 0)
                                : kiln_c99_text(args[0]);
            return KILN_C99_OK;
        default:
            return KILN_C99_CONTROL_ERROR;
        }
    }
    closure = kiln_c99_closure_of(callee);
    if (closure->generator) {
        /* Calling a generator function does not run it: it builds the object that will. */
        return kiln_c99_new_generator(result, callee, receiver, args, argc);
    }
    return closure->body(host, result, closure->upvalues, receiver, args, argc, depth + 1, thrown,
                         NULL);
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
    /* An object is always truthy, empty or not. */
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_DATE:
        return 1;
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
            if (element.tag == KILN_VALUE_UNDEFINED || element.tag == KILN_VALUE_NULL) {
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
    case KILN_VALUE_BUILTIN: {
        /* The interpreter names a runtime-supplied callable by the native it resolves to, and
         * this has to be the same text for the same program. */
        static const char string_constructor[] = "[Function: String]";
        if (value.bits == KILN_C99_BUILTIN_STRING) {
            return kiln_c99_value(KILN_VALUE_STRING, 0, string_constructor,
                                  sizeof(string_constructor) - 1);
        }
        return kiln_c99_undefined();
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
static int kiln_c99_get(kiln_host *host, kiln_value target, const char *key, size_t key_len,
                        kiln_value *out, int depth, kiln_value *thrown) {
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
        if (closure->properties.tag != KILN_VALUE_OBJECT) {
            *out = kiln_c99_undefined();
            return KILN_C99_OK;
        }
        return kiln_c99_get(host, closure->properties, key, key_len, out, depth, thrown);
    }
    case KILN_VALUE_ARRAY: {
        uint64_t code;
        if (is_length) {
            *out = kiln_c99_number((double)kiln_c99_array_of(target)->count);
            return KILN_C99_OK;
        }
        code = kiln_c99_array_method(key, key_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        /* A method this backend does not have.  Answering undefined would turn `a.sort(f)` into
         * "undefined is not a function" three frames from the cause, so it says so here. */
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_STRING: {
        uint64_t code;
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
        uint64_t code = kiln_c99_lookup_method(key, key_len, KILN_C99_METHOD_GENERATOR_NEXT,
                                               KILN_C99_METHOD_GENERATOR_SELF);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        return KILN_C99_CONTROL_ERROR;
    }
    case KILN_VALUE_BUILTIN: {
        uint64_t code = kiln_c99_namespace_method(target.bits, key, key_len);
        if (code != 0) {
            *out = kiln_c99_method(code);
            return KILN_C99_OK;
        }
        /* A namespace's constants are values rather than methods, so they are answered here
         * instead of through a call. */
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
        return KILN_C99_CONTROL_ERROR;
    default:
        /* A number, boolean or callable has no own properties here. */
        return KILN_C99_CONTROL_ERROR;
    }
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
        *out = (size_t)index < array->count ? array->elements[(size_t)index]
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
        /* An object key is a string, whatever it was written as. */
        kiln_value text = kiln_c99_text(key);
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
        int status = kiln_c99_reserve((void **)&array->elements, &array->capacity, slot + 1,
                                      sizeof *array->elements);
        if (status != KILN_C99_OK) {
            return status;
        }
        /* Writing past the end grows the array and leaves undefined behind, which is what the
         * language does; a hole and an undefined element are only distinguishable through `in`,
         * which this backend refuses. */
        for (fill = array->count; fill < slot; ++fill) {
            array->elements[fill] = kiln_c99_undefined();
        }
        array->elements[slot] = value;
        if (slot + 1 > array->count) {
            array->count = slot + 1;
        }
        return KILN_C99_OK;
    }
    if (target.tag != KILN_VALUE_OBJECT) {
        return KILN_C99_CONTROL_ERROR;
    }
    {
        kiln_value text = kiln_c99_text(key);
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
        if (array->count == 0) {
            return kiln_c99_text_append_c(text, "[]");
        }
        if (kiln_c99_text_append_c(text, "[ ") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < array->count; ++index) {
            if (index != 0 && kiln_c99_text_append_c(text, ", ") != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
            if (kiln_c99_inspect_into(text, array->elements[index], depth + 1) != KILN_C99_OK) {
                return KILN_C99_OUT_OF_MEMORY;
            }
        }
        return kiln_c99_text_append_c(text, " ]");
    }
    {
        kiln_object *object = kiln_c99_object_of(value);
        if (object->count == 0) {
            return kiln_c99_text_append_c(text, "{}");
        }
        if (kiln_c99_text_append_c(text, "{ ") != KILN_C99_OK) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        for (index = 0; index < object->count; ++index) {
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
    case KILN_VALUE_GENERATOR:
        return kiln_c99_text_append_c(text, "Object [Generator] {}");
    case KILN_VALUE_BUILTIN:
        return kiln_c99_text_append_c(text, "[Function: String]");
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
            kiln_value tail = kiln_c99_from_units(replacement, replacement_count);
            if (kiln_c99_text_append(&built, head.ptr, head.len) != KILN_C99_OK ||
                kiln_c99_text_append(&built, tail.ptr, tail.len) != KILN_C99_OK) {
                status = KILN_C99_OUT_OF_MEMORY;
                break;
            }
            from = (size_t)at + needle_count;
            /* `replace` takes the first occurrence; `replaceAll` takes every one. An empty
             * needle would otherwise never advance. */
            if (code == KILN_C99_METHOD_REPLACE || needle_count == 0) {
                break;
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

static int kiln_c99_invoke_method(kiln_host *host, kiln_value *result, uint64_t code,
                                  kiln_value receiver, const kiln_value *args, size_t argc,
                                  int depth, kiln_value *thrown) {
    kiln_array *array;
    size_t index;
    int status;
    if (code >= KILN_C99_METHOD_MATH_FLOOR) {
        /* A namespace method takes no receiver, so `Math` contributes nothing beyond having been
         * the way the function was reached. */
        double first = argc > 0 ? kiln_c99_as_number(args[0]) : 0.0 / 0.0;
        double second = argc > 1 ? kiln_c99_as_number(args[1]) : 0.0 / 0.0;
        switch (code) {
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
        case KILN_C99_METHOD_MATH_HYPOT: {
            /* A fold of `hypot` rather than `sqrt(x*x + y*y)`: squaring overflows for arguments
             * whose result is perfectly representable, which is the whole reason it exists. */
            double total = 0.0;
            for (index = 0; index < argc; ++index) {
                total = hypot(total, kiln_c99_as_number(args[index]));
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
                double value = kiln_c99_as_number(args[index]);
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
            if (argc == 0 || args[0].tag != KILN_VALUE_OBJECT) {
                return KILN_C99_CONTROL_ERROR;
            }
            object = kiln_c99_object_of(args[0]);
            status = kiln_c99_new_array(&built);
            for (index = 0; index < object->count && status == KILN_C99_OK; ++index) {
                kiln_value key = kiln_c99_copy_text(object->properties[index].key,
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
    if (code >= KILN_C99_METHOD_STRING_SLICE) {
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
            /* A nullish element contributes nothing, which is what makes `[1, , 3].join()`
             * produce "1,,3" rather than "1,undefined,3". */
            if (element.tag == KILN_VALUE_UNDEFINED || element.tag == KILN_VALUE_NULL) {
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
                visit[0] = array->elements[index];
                visit[1] = kiln_c99_number((double)index);
                visit[2] = receiver;
                status = kiln_c99_call(host, &produced, callback, self_value, visit, 3, depth, thrown);
                if (status != KILN_C99_OK) {
                    return status;
                }
                if (code == KILN_C99_METHOD_MAP) {
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
        *result = built;
        return KILN_C99_OK;
    }
    case KILN_C99_METHOD_REDUCE: {
        kiln_value callback = argc > 0 ? args[0] : kiln_c99_undefined();
        kiln_value accumulator;
        size_t length = array->count;
        index = 0;
        if (argc > 1) {
            accumulator = args[1];
        } else {
            /* Without a seed the first element is the accumulator, and an empty array is a
             * TypeError rather than undefined -- there is no answer to give. */
            if (length == 0) {
                return KILN_C99_CONTROL_ERROR;
            }
            accumulator = array->elements[0];
            index = 1;
        }
        for (; index < length && index < array->count; ++index) {
            kiln_value visit[4];
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
            status = kiln_c99_array_push(target, array->elements[index]);
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
 * property; defining the second must not discard the first. */
static int kiln_c99_define_accessor(kiln_value target, const char *key, size_t key_len,
                                    kiln_value getter, kiln_value setter) {
    kiln_object *object;
    size_t index;
    int status;
    if (target.tag == KILN_VALUE_FUNCTION) {
        kiln_value store;
        status = kiln_c99_function_store(target, &store);
        if (status != KILN_C99_OK) {
            return status;
        }
        return kiln_c99_define_accessor(store, key, key_len, getter, setter);
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
            /* An element with no JSON spelling becomes null rather than disappearing: an array's
             * positions are part of its meaning. */
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
    int status;
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
    status = kiln_c99_call(host, &produced, callee, instance, args, argc, depth, thrown);
    if (status != KILN_C99_OK) {
        return status;
    }
    *result = produced.tag == KILN_VALUE_OBJECT || produced.tag == KILN_VALUE_ARRAY ? produced
                                                                                   : instance;
    return KILN_C99_OK;
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
        case KILN_C99_BUILTIN_ARRAY: *answer = value.tag == KILN_VALUE_ARRAY; return KILN_C99_OK;
        case KILN_C99_BUILTIN_OBJECT:
            *answer = value.tag == KILN_VALUE_OBJECT || value.tag == KILN_VALUE_ARRAY ||
                      value.tag == KILN_VALUE_DATE;
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
    switch (value.tag) {
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
        if (value.bits == KILN_C99_BUILTIN_MATH || value.bits == KILN_C99_BUILTIN_JSON) {
            return kiln_c99_value(KILN_VALUE_STRING, 0, object_text, sizeof(object_text) - 1);
        }
        return kiln_c99_value(KILN_VALUE_STRING, 0, function_text, sizeof(function_text) - 1);
    case KILN_VALUE_FUNCTION:
    case KILN_VALUE_METHOD:
        return kiln_c99_value(KILN_VALUE_STRING, 0, function_text, sizeof(function_text) - 1);
    case KILN_VALUE_OBJECT:
    case KILN_VALUE_ARRAY:
    case KILN_VALUE_GENERATOR:
    case KILN_VALUE_DATE:
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
    /* A function reduces to a primitive before every coercing operator, exactly as it does in
     * the interpreter: its `valueOf` hands back the object itself, so `toString` decides.
     * Strict equality is identity and never coerces, and a nullish `==` is answered without any
     * conversion at all. */
    if (kiln_c99_is_reference(left) || kiln_c99_is_reference(right)) {
        int coerces = 1;
        if (operation == 7 || operation == 8) {
            coerces = 0;
        } else if ((operation == 15 || operation == 16) &&
                   (left.tag == KILN_VALUE_UNDEFINED || left.tag == KILN_VALUE_NULL ||
                    right.tag == KILN_VALUE_UNDEFINED || right.tag == KILN_VALUE_NULL)) {
            coerces = 0;
        }
        if (coerces) {
            if (kiln_c99_is_reference(left)) {
                left = kiln_c99_text(left);
            }
            if (kiln_c99_is_reference(right)) {
                right = kiln_c99_text(right);
            }
        }
    }
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
    // The native value representation has no arbitrary-precision integer, and truncating one
    // into a double is the one thing a BigInt exists to prevent.
    if program
        .constants
        .iter()
        .any(|constant| matches!(constant, Constant::BigInt(_)))
    {
        return Err(EmitError::Unsupported {
            feature: "BigInt literals in the native backend".to_string(),
        });
    }
    for proto in &program.functions {
        // A generator does not run when it is called: it builds an object that runs in pieces,
        // each resumption picking up where the last left off. That is a suspended frame, which
        // an automatic C frame cannot be. An `async` function is compiled as one for the same
        // reason -- `await` suspends exactly as `yield` does.
        if proto.asynchronous || proto.async_generator {
            return Err(EmitError::Unsupported {
                feature: "async functions in the native backend".to_string(),
            });
        }
        // A generator's state survives its own suspension, and a handler stack is part of that
        // state. Refused rather than half-supported: a `try` that silently stopped catching
        // across a `yield` is the kind of thing that shows up three frames away.
        if proto.generator
            && proto
                .code
                .iter()
                .any(|instruction| matches!(instruction, Instruction::PushHandler(_)))
        {
            return Err(EmitError::Unsupported {
                feature: "a `try` inside a generator in the native backend".to_string(),
            });
        }
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

/// Name the instruction a refusal is about.
///
/// The catch-all used to answer "functions and objects in the native backend" for all forty of
/// them, which tells a reader nothing about which one their program actually reached -- the same
/// defect the frontend's "unsupported expression: expression" had. A variant name is not a
/// polished feature name, but it is the truth and it sorts a backlog.
fn instruction_feature(instruction: &Instruction) -> String {
    let rendered = format!("{instruction:?}");
    let name = rendered
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("instruction");
    format!("`{name}` in the native backend")
}

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
            | kiln_ir::Builtin::NotANumber
            | kiln_ir::Builtin::Infinity,
        ) => Ok(()),
        Instruction::LoadBuiltin(builtin) => Err(EmitError::Unsupported {
            feature: format!("the `{builtin:?}` builtin in the native backend"),
        }),
        Instruction::Binary(BinaryOp::In) => Err(EmitError::Unsupported {
            feature: "the `in` operator in the native backend".to_string(),
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
        | Instruction::New { .. }
        | Instruction::SetPrototype
        | Instruction::LoadGlobal(_)
        | Instruction::StoreGlobal(_)
        | Instruction::DefineAccessor { .. }
        | Instruction::SpreadObject
        | Instruction::SpreadInto
        | Instruction::IterableToArray
        | Instruction::Yield
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
        _ => Err(EmitError::Unsupported {
            feature: instruction_feature(instruction),
        }),
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
        Instruction::Dup2
        | Instruction::DeleteProperty(_)
        | Instruction::DeleteComputed
        | Instruction::Apply
        | Instruction::EnumerateKeys
        | Instruction::GetPrivate(_)
        | Instruction::PushHole
        | Instruction::NewRegExp { .. }
        | Instruction::ThrowReference(_)
        | Instruction::SetPrivate(_)
        | Instruction::ConstructSpread
        | Instruction::CallComputed { .. }
        | Instruction::LoadNewTarget
        | Instruction::LoadArguments
        | Instruction::RestArguments(_)
        | Instruction::Await
        | Instruction::SuperCall { .. }
        | Instruction::SuperForward => Err(EmitError::Unsupported {
            feature: instruction_feature(instruction),
        }),
        Instruction::Const(_)
        | Instruction::LoadLocal(_)
        | Instruction::LoadCell(_)
        | Instruction::LoadUpvalue(_)
        | Instruction::LoadBuiltin(_)
        | Instruction::LoadGlobal(_)
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
        Instruction::DefineAccessor { .. } => {
            if height < 3 {
                Err(underflow(3))
            } else {
                Ok((height - 2, false))
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
        // the result of the `yield` expression.
        Instruction::Yield => {
            if height < 1 {
                Err(underflow(1))
            } else {
                Ok((height, false))
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
        Instruction::Binary(BinaryOp::In) => Err(EmitError::Unsupported {
            feature: "the `in` operator in the native backend".to_string(),
        }),
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

/// Whether any body in this program can produce a callable value.
///
/// The boundary conversions depend on it, and they are not free to emit unconditionally:
/// reaching `kiln_c99_text` from every entry point makes every unit reference the number
/// provider, which the graph link tests deliberately link without. It would also change the
/// bytes of a scalar unit, and those bytes are a cache key.
fn produces_callables(program: &Program) -> bool {
    let makes_one = |code: &[Instruction]| {
        code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::MakeFunction(_) | Instruction::LoadBuiltin(_)
            )
        })
    };
    makes_one(&program.code) || program.functions.iter().any(|proto| makes_one(&proto.code))
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

/// Emit one C function per compiled body, forward-declared so a closure made before a body is
/// rendered can still point at it.
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

/// Whether this program needs a call context -- a depth counter and somewhere to put a thrown
/// value -- in scope.
///
/// It is not only calls. Reading a property can call, because the property may be an accessor;
/// writing one can call a setter; building an object literal converts its keys. Anything that
/// reaches the runtime's property machinery reaches its calling machinery with it.
///
/// A program with none of these is the scalar shape this backend started with, and it must keep
/// producing byte-for-byte the same unit: those bytes are a cache key, so even a declaration
/// nothing reads would invalidate every cached object.
fn needs_call_context(program: &Program) -> bool {
    let needs = |code: &[Instruction]| {
        code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::Throw
                    | Instruction::Call { .. }
                    | Instruction::CallMethod { .. }
                    | Instruction::New { .. }
                    | Instruction::GetProperty(_)
                    | Instruction::SetProperty(_)
                    | Instruction::GetComputed
                    | Instruction::SetComputed
                    | Instruction::NewObject { .. }
                    | Instruction::DefineAccessor { .. }
                    | Instruction::SpreadObject
                    | Instruction::SpreadInto
                    | Instruction::IterableToArray
            )
        })
    };
    needs(&program.code) || program.functions.iter().any(|proto| needs(&proto.code))
}

/// What a call site does when the callee threw: jump to this body's handler, or hand it on.
fn unwind_or_return(handlers: &[u32]) -> &'static str {
    if handlers.is_empty() {
        "return status;"
    } else {
        "goto kiln_unwind;"
    }
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
fn render_handler_prologue(output: &mut String, handlers: &[u32], depth: usize) {
    if handlers.is_empty() {
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
fn render_unwind(output: &mut String, handlers: &[u32]) {
    if handlers.is_empty() {
        return;
    }
    output.push_str("kiln_unwind:;\n");
    output.push_str("    if (kiln_handler_top == 0) return KILN_C99_THROWN;\n");
    output.push_str("    --kiln_handler_top;\n");
    output.push_str("    sp = kiln_handler_stack[kiln_handler_top];\n");
    output.push_str("    stack[sp++] = *thrown;\n");
    output.push_str("    switch (kiln_handlers[kiln_handler_top]) {\n");
    for target in handlers {
        writeln!(output, "    case {target}: goto kiln_label_{target};").unwrap();
    }
    output.push_str("    default: return KILN_C99_CONTROL_ERROR;\n    }\n");
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
    let yields: Vec<usize> = proto
        .code
        .iter()
        .enumerate()
        .filter(|(_, instruction)| matches!(instruction, Instruction::Yield))
        .map(|(position, _)| position)
        .collect();
    if proto.generator {
        // The frame outlives this call, so neither array can be automatic: both belong to the
        // generator, and every `locals[i]` and `stack[sp]` below reads through them unchanged.
        output.push_str("    kiln_value *stack = frame->stack;\n");
        output.push_str("    kiln_value *locals = frame->locals;\n");
        render_handler_prologue(output, &handlers, proto.code.len());
        output.push_str("    if (result == NULL) return KILN_C99_INVALID_ARGUMENT;\n");
        output.push_str("    (void)thrown;\n");
        output.push_str("    (void)host; (void)upvalues; (void)depth; (void)receiver;\n");
        output.push_str("    (void)args; (void)argc; (void)slot;\n");
        // Re-entry. A generator that has not started begins at its first instruction; one that
        // has jumps back to just after the `Yield` that suspended it, with the operand stack
        // restored to the height it had there.
        if !yields.is_empty() {
            output.push_str(
                "    if (frame->started) {\n        sp = frame->sp;\n        switch (frame->resume) {\n",
            );
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
        render_handler_prologue(output, &handlers, proto.code.len());
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
        callables: produces_callables(program),
        handlers,
        site: ReturnSite::Body,
        guarded: dead_zone_slots(&proto.code),
    };
    for position in 0..proto.code.len() {
        writeln!(output, "kiln_label_{position}:;").unwrap();
        emit_instruction(output, &proto.code[position], position, &emitter);
    }
    render_unwind(output, &emitter.handlers);
    // A body whose control reaches the end without a `Return` is a compiler bug, not a program
    // error: the lowering always ends a body with one.
    output.push_str("    *result = kiln_c99_undefined();\n    return KILN_C99_CONTROL_ERROR;\n}\n");
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
    if needs_call_context(program) {
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
        callables: produces_callables(program),
        handlers: handler_targets(&program.code),
        site: ReturnSite::Entry { exports: None },
        guarded: dead_zone_slots(&program.code),
    };
    for index in 0..program.code.len() {
        writeln!(output, "kiln_label_{index}:;").unwrap();
        emit_instruction(&mut output, &program.code[index], index, &emitter);
    }
    render_unwind(&mut output, &emitter.handlers);
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
    if needs_call_context(program) {
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
        callables: produces_callables(program),
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
    render_unwind(&mut output, &emitter.handlers);
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
        Instruction::Dup2
        | Instruction::DeleteProperty(_)
        | Instruction::DeleteComputed
        | Instruction::Apply
        | Instruction::EnumerateKeys
        | Instruction::GetPrivate(_)
        | Instruction::PushHole
        | Instruction::NewRegExp { .. }
        | Instruction::ThrowReference(_)
        | Instruction::SetPrivate(_)
        | Instruction::ConstructSpread
        | Instruction::CallComputed { .. }
        | Instruction::LoadNewTarget
        | Instruction::LoadArguments
        | Instruction::RestArguments(_)
        | Instruction::Await
        | Instruction::SuperCall { .. }
        | Instruction::SuperForward => {
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
            Constant::BigInt(_) => {
                unreachable!("verification rejects a BigInt constant before emission")
            }
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
            let generator = i32::from(target.generator);
            let frame_locals = target.local_count.max(1);
            let frame_stack = frame_stacks[*proto as usize].max(1);
            output.push_str("    {\n        int status;\n");
            if captures.is_empty() {
                writeln!(
                    output,
                    "        status = kiln_c99_make_closure(&stack[sp], {symbol}_fn_{proto}, {name}, {name_len}, UINT32_C({proto}), {generator}, UINT32_C({frame_locals}), UINT32_C({frame_stack}), NULL, 0);"
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
                    "        status = kiln_c99_make_closure(&stack[sp], {symbol}_fn_{proto}, {name}, {name_len}, UINT32_C({proto}), {generator}, UINT32_C({frame_locals}), UINT32_C({frame_stack}), captured, UINT32_C({}));",
                    captures.len()
                )
                .unwrap();
            }
            output.push_str("        if (status != KILN_C99_OK) return status;\n        ++sp;\n    }\n");
        }
        Instruction::Call { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers);
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
            let unwind = unwind_or_return(handlers);
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
            let unwind = unwind_or_return(handlers);
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
            let unwind = unwind_or_return(handlers);
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
            let unwind = unwind_or_return(handlers);
            writeln!(
                output,
                "    {{ kiln_value key = stack[--sp]; kiln_value read; int status = kiln_c99_get_computed(host, stack[sp - 1], key, &read, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = read; }}"
            )
            .unwrap();
        }
        Instruction::SetComputed => {
            let unwind = unwind_or_return(handlers);
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
        Instruction::New { argc } => {
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers);
            writeln!(
                output,
                "    {{ kiln_value built; int status; const kiln_value *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; status = kiln_c99_construct(host, &built, stack[sp - {argc} - 1], call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = built; }}",
                argc + 1
            )
            .unwrap();
        }
        Instruction::DefineAccessor { setter } => {
            let half = if *setter { "setter" } else { "getter" };
            let other = if *setter { "getter" } else { "setter" };
            writeln!(
                output,
                "    {{ kiln_value {half} = stack[--sp]; kiln_value key = stack[--sp]; kiln_value {other} = kiln_c99_undefined(); kiln_value name = kiln_c99_text(key); int status; if (name.tag != KILN_VALUE_STRING) return KILN_C99_CONTROL_ERROR; status = kiln_c99_define_accessor(stack[sp - 1], name.ptr, name.len, getter, setter); if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        Instruction::SpreadInto => {
            let unwind = unwind_or_return(handlers);
            writeln!(
                output,
                "    {{ kiln_value source = stack[--sp]; int status = kiln_c99_iterate(host, stack[sp - 1], source, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; }}"
            )
            .unwrap();
        }
        Instruction::IterableToArray => {
            let unwind = unwind_or_return(handlers);
            writeln!(
                output,
                "    {{ kiln_value built; int status = kiln_c99_new_array(&built); if (status != KILN_C99_OK) return status; status = kiln_c99_iterate(host, built, stack[sp - 1], depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; stack[sp - 1] = built; }}"
            )
            .unwrap();
        }
        Instruction::SpreadObject => {
            let unwind = unwind_or_return(handlers);
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
                "    { kiln_value prototype = stack[--sp]; if (stack[sp - 1].tag != KILN_VALUE_OBJECT) return KILN_C99_CONTROL_ERROR; kiln_c99_object_of(stack[sp - 1])->prototype = prototype; }\n",
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
        Instruction::Yield => {
            writeln!(
                output,
                "    frame->sp = sp - 1; frame->resume = {index}; *result = stack[sp - 1]; return KILN_C99_YIELDED;"
            )
            .unwrap();
            // Where `next` comes back to. What the caller sent becomes the value of the `yield`
            // expression, which is why the height here is one more than it was at the suspension.
            writeln!(output, "kiln_resume_{index}:;").unwrap();
            output.push_str("    stack[sp++] = frame->sent;\n");
        }
        Instruction::Throw => {
            output.push_str("    *thrown = stack[--sp];\n");
            if handlers.is_empty() {
                // Nothing here can catch it, so it leaves immediately.
                output.push_str("    return KILN_C99_THROWN;\n");
            } else {
                output.push_str("    goto kiln_unwind;\n");
            }
        }
        // Not a `GetProperty` followed by a `Call`: that would discard the receiver, and the
        // read has to yield the *unbound* function so `a.f === a.f` holds. The receiver stays on
        // the stack until the call, which is what lets both be true at once.
        Instruction::CallMethod { name, argc } => {
            let Constant::String(text) = &constants[*name as usize] else {
                unreachable!("verification rejects a non-string method name before emission")
            };
            let argc = usize::from(*argc);
            let unwind = unwind_or_return(handlers);
            writeln!(
                output,
                "    {{ kiln_value method; kiln_value call_result; int status; kiln_value self_value = stack[sp - {argc} - 1]; const kiln_value *call_args = {argc} == 0 ? NULL : &stack[sp - {argc}]; status = kiln_c99_get(host, self_value, {symbol}_constant_{name}, {}, &method, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; status = kiln_c99_call(host, &call_result, method, self_value, call_args, (size_t){argc}, depth, thrown); if (status == KILN_C99_THROWN) {unwind} if (status != KILN_C99_OK) return status; sp -= (size_t){}; stack[sp++] = call_result; }}",
                text.len(),
                argc + 1
            )
            .unwrap();
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
                kiln_ir::Builtin::NotANumber => "kiln_c99_number(0.0 / 0.0)",
                kiln_ir::Builtin::Infinity => "kiln_c99_number(1.0 / 0.0)",
                _ => unreachable!("verification rejects every other builtin before emission"),
            };
            writeln!(output, "    stack[sp++] = {pushed};").unwrap();
        }
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
            // The embedding boundary carries only the five scalar tags, so a function argument
            // is reduced to its text before the host sees it -- which is also what the
            // interpreter prints for one.
            if *argc > 0 && *callables {
                writeln!(output, "    {{ size_t arg; for (arg = sp - {argc}; arg < sp; ++arg) if (kiln_c99_is_reference(stack[arg])) stack[arg] = kiln_c99_inspect(stack[arg]); }}").unwrap();
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
                "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp]; if (kiln_c99_is_reference(*result)) *result = kiln_c99_text(*result); return KILN_C99_OK;\n",
            ),
            ReturnSite::Entry {
                exports: Some(exports),
            } => {
                output.push_str(
                    "    *result = sp == 0 ? kiln_c99_undefined() : stack[--sp];\n",
                );
                if *callables {
                    output.push_str(
                        "    if (kiln_c99_is_reference(*result)) *result = kiln_c99_text(*result);\n",
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
            Instruction::DefineAccessor { setter } => {
                byte(&mut hash, 52);
                byte(&mut hash, u8::from(*setter));
            }
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

    fn linked_source(module_source: &str, wrapper: &str) -> String {
        format!(
            "{}\n{}\n{}",
            kiln_native_number::runtime_source(),
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
        assert!(!generated.source.contains("strtod"));
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

    /// `instanceof` emits now that a value can carry a prototype chain; `in` does not, because it
    /// asks whether a *name* is present and nothing here enumerates one yet. The point of the
    /// test is unchanged: an operator this backend cannot do must say so rather than emit a
    /// silent undefined.
    #[test]
    fn the_in_operator_is_refused_rather_than_emitting_undefined() {
        let result = emit(&program(
            vec![Constant::Null],
            vec![
                Instruction::Const(0),
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::In),
                Instruction::Return,
            ],
            0,
        ));
        assert!(
            matches!(result, Err(EmitError::Unsupported { feature }) if feature.contains("`in` operator")),
            "`in` must not emit a silent undefined result"
        );
    }

    #[test]
    fn unreachable_unsupported_instructions_are_refused_before_rendering() {
        let result = emit(&program(
            vec![Constant::Boolean(true)],
            vec![
                Instruction::Jump(2),
                Instruction::EnumerateKeys,
                Instruction::Const(0),
                Instruction::Return,
            ],
            0,
        ));
        assert!(
            matches!(result, Err(EmitError::Unsupported { feature }) if feature.contains("`EnumerateKeys`")),
            "dead code must not bypass the native instruction allowlist"
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
    /// instead of reporting anything.
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
            "int main(void) {{ kiln_value result; return {}(NULL, &result) == KILN_C99_CALL_DEPTH_ERROR ? 0 : 1; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "depth");
        assert!(
            run.status.success(),
            "unbounded recursion did not report a call-depth error (exit {:?})",
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
