
/* The JavaScript name of a runtime-supplied value, which is what a console shows and what
 * `String(x)` produces. The interpreter derives its equivalent from the variant's spelling; here
 * the tags carry no spelling to derive from, so this is the derivation -- and
 * `conformance/differential/builtin-names.js` is what checks the two agree with node. */
static const char *kiln_c99_builtin_name(uint64_t builtin) {
    switch (builtin) {
    /* The globals this backend has not implemented still have to name themselves, because the
     * name is what the refusal reports and what `X.name` answers. */
    case KILN_C99_BUILTIN_INTL: return "Intl";
    case KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER: return "SharedArrayBuffer";
    case KILN_C99_BUILTIN_WEAK_REF: return "WeakRef";
    case KILN_C99_BUILTIN_FINALIZATION_REGISTRY: return "FinalizationRegistry";
    case KILN_C99_BUILTIN_SHADOW_REALM: return "ShadowRealm";
    case KILN_C99_BUILTIN_FLOAT16_ARRAY: return "Float16Array";
    case KILN_C99_BUILTIN_BIG_INT64_ARRAY: return "BigInt64Array";
    case KILN_C99_BUILTIN_BIG_UINT64_ARRAY: return "BigUint64Array";
    case KILN_C99_BUILTIN_DISPOSABLE_STACK: return "DisposableStack";
    case KILN_C99_BUILTIN_ASYNC_DISPOSABLE_STACK: return "AsyncDisposableStack";
    case KILN_C99_BUILTIN_SUPPRESSED_ERROR: return "SuppressedError";
    case KILN_C99_BUILTIN_EVAL_ERROR: return "EvalError";
    case KILN_C99_BUILTIN_TEXT_DECODER: return "TextDecoder";
    case KILN_C99_BUILTIN_EVAL: return "eval";
    /* Lower-case because that is what the language calls it: `performance` is an object a
     * program reads `now` from, not a constructor it names. */
    case KILN_C99_BUILTIN_PERFORMANCE: return "performance";
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
    case KILN_C99_BUILTIN_WEAK_MAP: return "WeakMap";
    case KILN_C99_BUILTIN_WEAK_SET: return "WeakSet";
    case KILN_C99_BUILTIN_PROMISE: return "Promise";
    case KILN_C99_BUILTIN_BOOLEAN: return "Boolean";
    case KILN_C99_BUILTIN_REFLECT: return "Reflect";
    case KILN_C99_BUILTIN_PROXY: return "Proxy";
    case KILN_C99_BUILTIN_URL: return "URL";
    case KILN_C99_BUILTIN_BUFFER: return "Buffer";
    /* The three web constructors this realm publishes. Absent from this switch they fell through
     * to the error-name lookup and reported "anonymous" -- a name no program can have asked for
     * and the very thing `X.name` is read to distinguish. `Object.getOwnPropertyNames(globalThis)`
     * publishes them; `every_global_reports_the_name_it_is_published_under` now proves each one
     * answers the name it went out under. */
    case KILN_C99_BUILTIN_ABORT_CONTROLLER: return "AbortController";
    case KILN_C99_BUILTIN_ABORT_SIGNAL: return "AbortSignal";
    case KILN_C99_BUILTIN_TEXT_ENCODER: return "TextEncoder";
    case KILN_C99_BUILTIN_TEMPORAL: return "Temporal";
    case KILN_C99_BUILTIN_TEMPORAL_NOW: return "Now";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE: return "PlainDate";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME: return "PlainTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME: return "PlainDateTime";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH: return "PlainYearMonth";
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY: return "PlainMonthDay";
    case KILN_C99_BUILTIN_TEMPORAL_DURATION: return "Duration";
    case KILN_C99_BUILTIN_TEMPORAL_INSTANT: return "Instant";
    case KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME: return "ZonedDateTime";
    case KILN_C99_BUILTIN_OBJECT_PROTOTYPE: return "Object";
    case KILN_C99_BUILTIN_ARRAY_PROTOTYPE: return "Array";
    case KILN_C99_BUILTIN_ERROR_PROTOTYPE: return "Error";
    case KILN_C99_BUILTIN_AGGREGATE_ERROR_PROTOTYPE: return "AggregateError";
    case KILN_C99_BUILTIN_MAP_PROTOTYPE: return "Map";
    case KILN_C99_BUILTIN_SET_PROTOTYPE: return "Set";
    case KILN_C99_BUILTIN_WEAK_MAP_PROTOTYPE: return "WeakMap";
    case KILN_C99_BUILTIN_WEAK_SET_PROTOTYPE: return "WeakSet";
    case KILN_C99_BUILTIN_STRING_PROTOTYPE: return "String";
    /* Both spell "Iterator": the constructor answers it for `Iterator.name`, and the table
     * answers it because a refusal naming the table has to name the global a reader can find. */
    case KILN_C99_BUILTIN_ITERATOR: return "Iterator";
    case KILN_C99_BUILTIN_ITERATOR_PROTOTYPE: return "Iterator";
    case KILN_C99_BUILTIN_SYMBOL: return "Symbol";
    case KILN_C99_BUILTIN_SYMBOL_PROTOTYPE: return "Symbol";
    case KILN_C99_BUILTIN_REGEXP: return "RegExp";
    case KILN_C99_BUILTIN_REGEXP_PROTOTYPE: return "RegExp";
    case KILN_C99_BUILTIN_DATE_PROTOTYPE: return "Date";
    case KILN_C99_BUILTIN_NUMBER_PROTOTYPE: return "Number";
    case KILN_C99_BUILTIN_BOOLEAN_PROTOTYPE: return "Boolean";
    case KILN_C99_BUILTIN_FUNCTION_PROTOTYPE: return "Function";
    case KILN_C99_BUILTIN_FUNCTION: return "Function";
    case KILN_C99_BUILTIN_ARRAY_BUFFER: return "ArrayBuffer";
    case KILN_C99_BUILTIN_DATA_VIEW: return "DataView";
    /* A prototype table answers with its constructor's name, as every other pair here does. */
    case KILN_C99_BUILTIN_DATA_VIEW_PROTOTYPE: return "DataView";
    case KILN_C99_BUILTIN_INT8_ARRAY: return "Int8Array";
    case KILN_C99_BUILTIN_UINT8_ARRAY: return "Uint8Array";
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY: return "Uint8ClampedArray";
    case KILN_C99_BUILTIN_INT16_ARRAY: return "Int16Array";
    case KILN_C99_BUILTIN_UINT16_ARRAY: return "Uint16Array";
    case KILN_C99_BUILTIN_INT32_ARRAY: return "Int32Array";
    case KILN_C99_BUILTIN_UINT32_ARRAY: return "Uint32Array";
    case KILN_C99_BUILTIN_FLOAT32_ARRAY: return "Float32Array";
    case KILN_C99_BUILTIN_FLOAT64_ARRAY: return "Float64Array";
    case KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE: return "TypedArray";
    case KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE: return "Int8Array";
    case KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE: return "Uint8Array";
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE: return "Uint8ClampedArray";
    case KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE: return "Int16Array";
    case KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE: return "Uint16Array";
    case KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE: return "Int32Array";
    case KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE: return "Uint32Array";
    case KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE: return "Float32Array";
    case KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE: return "Float64Array";
    case KILN_C99_BUILTIN_ARRAY_BUFFER_PROTOTYPE: return "ArrayBuffer";
    default: {
        /* Every other tag is an error constructor; a tag with no name at all is a tag added
         * without one, which shows as anonymous rather than dereferencing a null. */
        const char *name = kiln_c99_error_name(builtin);
        return name == NULL ? "anonymous" : name;
    }
    }
}

/* The canonical arity (fn.length) of a built-in callable per ECMAScript specification.
 * Returns 1 and sets *out if known; returns 0 if unknown/unmodeled. */
static int kiln_c99_builtin_length(uint64_t builtin, double *out) {
    switch (builtin) {
    case KILN_C99_BUILTIN_SYMBOL:
    case KILN_C99_BUILTIN_MAP:
    case KILN_C99_BUILTIN_SET:
    case KILN_C99_BUILTIN_WEAK_MAP:
    case KILN_C99_BUILTIN_WEAK_SET:
    case KILN_C99_BUILTIN_ABORT_CONTROLLER:
    case KILN_C99_BUILTIN_ABORT_SIGNAL:
    case KILN_C99_BUILTIN_TEXT_ENCODER:
    case KILN_C99_BUILTIN_TEXT_DECODER:
    case KILN_C99_BUILTIN_DISPOSABLE_STACK:
    case KILN_C99_BUILTIN_ASYNC_DISPOSABLE_STACK:
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_TIME:
    case KILN_C99_BUILTIN_TEMPORAL_DURATION:
        *out = 0.0;
        return 1;

    case KILN_C99_BUILTIN_ARRAY:
    case KILN_C99_BUILTIN_OBJECT:
    case KILN_C99_BUILTIN_STRING:
    case KILN_C99_BUILTIN_NUMBER:
    case KILN_C99_BUILTIN_BOOLEAN:
    case KILN_C99_BUILTIN_BIGINT:
    case KILN_C99_BUILTIN_FUNCTION:
    case KILN_C99_BUILTIN_PROMISE:
    case KILN_C99_BUILTIN_ARRAY_BUFFER:
    case KILN_C99_BUILTIN_SHARED_ARRAY_BUFFER:
    case KILN_C99_BUILTIN_DATA_VIEW:
    case KILN_C99_BUILTIN_ERROR:
    case KILN_C99_BUILTIN_TYPE_ERROR:
    case KILN_C99_BUILTIN_RANGE_ERROR:
    case KILN_C99_BUILTIN_REFERENCE_ERROR:
    case KILN_C99_BUILTIN_SYNTAX_ERROR:
    case KILN_C99_BUILTIN_URI_ERROR:
    case KILN_C99_BUILTIN_EVAL_ERROR:
    case KILN_C99_BUILTIN_EVAL:
    case KILN_C99_BUILTIN_URL:
    case KILN_C99_BUILTIN_BUFFER:
    case KILN_C99_BUILTIN_WEAK_REF:
    case KILN_C99_BUILTIN_FINALIZATION_REGISTRY:
    case KILN_C99_BUILTIN_TEMPORAL_INSTANT:
        *out = 1.0;
        return 1;

    case KILN_C99_BUILTIN_REGEXP:
    case KILN_C99_BUILTIN_PROXY:
    case KILN_C99_BUILTIN_AGGREGATE_ERROR:
    case KILN_C99_BUILTIN_SUPPRESSED_ERROR:
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_YEAR_MONTH:
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_MONTH_DAY:
    case KILN_C99_BUILTIN_TEMPORAL_ZONED_DATE_TIME:
        *out = 2.0;
        return 1;

    case KILN_C99_BUILTIN_INT8_ARRAY:
    case KILN_C99_BUILTIN_UINT8_ARRAY:
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY:
    case KILN_C99_BUILTIN_INT16_ARRAY:
    case KILN_C99_BUILTIN_UINT16_ARRAY:
    case KILN_C99_BUILTIN_INT32_ARRAY:
    case KILN_C99_BUILTIN_UINT32_ARRAY:
    case KILN_C99_BUILTIN_FLOAT16_ARRAY:
    case KILN_C99_BUILTIN_FLOAT32_ARRAY:
    case KILN_C99_BUILTIN_FLOAT64_ARRAY:
    case KILN_C99_BUILTIN_BIG_INT64_ARRAY:
    case KILN_C99_BUILTIN_BIG_UINT64_ARRAY:
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE:
    case KILN_C99_BUILTIN_TEMPORAL_PLAIN_DATE_TIME:
        *out = 3.0;
        return 1;

    case KILN_C99_BUILTIN_DATE:
        *out = 7.0;
        return 1;

    default:
        return 0;
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
            uint8_t *encoded = NULL;
            size_t encoded_len = 0;
            if (text.tag != KILN_VALUE_STRING ||
                kiln_c99_string_to_utf8_lossy(text, &encoded, &encoded_len) != KILN_C99_OK ||
                kiln_c99_text_append(out, (const char *)encoded, encoded_len) != KILN_C99_OK) {
                free(encoded);
                return KILN_C99_OUT_OF_MEMORY;
            }
            free(encoded);
        } else {
            kiln_value text = kiln_c99_from_units(&literal, 1);
            uint8_t *encoded = NULL;
            size_t encoded_len = 0;
            if (text.tag != KILN_VALUE_STRING ||
                kiln_c99_string_to_utf8_lossy(text, &encoded, &encoded_len) != KILN_C99_OK ||
                kiln_c99_text_append(out, (const char *)encoded, encoded_len) != KILN_C99_OK) {
                free(encoded);
                return KILN_C99_OUT_OF_MEMORY;
            }
            free(encoded);
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

/* Why a typed entry returned `KILN_VAL_TYPED_FAIL`, and what it threw when the status is a
 * throw. A typed entry carries no `thrown` slot of its own unless its body needs one, so a
 * failure leaves the way an exception leaves any frame: pending, for the caller to pick up at
 * once. Both are frame state; nothing reads them past the call that set them. */
static int kiln_c99_typed_status;
static kiln_value kiln_c99_typed_thrown;

/* What a derived constructor's `super()` produced, when a built-in parent produced something
 * other than the instance `new` allocated.
 *
 * A constructor body reassigns its own `receiver` when that happens, but the *caller* -- the
 * `new` that allocated the abandoned instance, or an outer `super()` one link down the chain --
 * has no other way to hear about it: a constructor returns undefined, so nothing in the value it
 * hands back says which object `this` ended up being. This is that channel. Saved and restored
 * around every construction and every `super()` so a nested one cannot be mistaken for this
 * one. */
static kiln_value kiln_c99_adopted_this;

/* The running call's own arguments, and the list an arrow sees instead.
 *
 * An arrow has no `arguments` of its own -- `arguments` inside one is the enclosing function's,
 * which is a *different list* from the arrow's own rest parameter. So an arrow captures the
 * visible list where it was written, exactly as it captures `this` and `new.target`, and the
 * three are saved and restored together around every call for the same reason.
 *
 * The raw pointer and count rather than an array, because a call must not pay to materialise an
 * `arguments` nothing asked for; the array is built where one is actually read. */
static const kiln_val *kiln_c99_frame_args;
static size_t kiln_c99_frame_argc;
static kiln_value kiln_c99_captured_arguments;
static int kiln_c99_frame_is_arrow;
/* The one `arguments` object this call has handed out, built on first mention and undefined
 * until then.
 *
 * `arguments` names a single object per call, not a snapshot: `arguments[0] = 1` has to be
 * visible to the next read of `arguments`, and `arguments === arguments` is true. Materialising
 * a fresh array per mention made both false, and every definition made on one was discarded by
 * the statement after it. Still built lazily, so a call nothing asks it of pays nothing. */
static kiln_value kiln_c99_frame_arguments_object;
/* Whether this call's `arguments` is the *mapped* kind: sloppy code with a simple parameter
 * list. Written by the body's own prologue, because it is a fact about the function that the
 * compiler decided and the emitted code carries as a constant. */
static int kiln_c99_frame_mapped_arguments;
/* The function this call is of, for a mapped `arguments` object's `callee`. */
static kiln_value kiln_c99_frame_callee;

/* `CreateUnmappedArgumentsObject`: an ordinary object carrying the call's arguments.
 *
 * It used to be an array, which is the wrong answer to four questions a program actually asks:
 * `Array.isArray(arguments)` said true, `Object.prototype.toString.call(arguments)` said
 * `[object Array]`, `Object.getPrototypeOf(arguments)` was not `Object.prototype`, and `length`
 * was the array's own rather than an ordinary writable property nothing keeps in step with the
 * indices. Every array-generic idiom that reaches for `arguments` -- `slice.call`, `apply`,
 * `Array.from`, spread, `for-of` -- goes through the array-like path instead, which is where the
 * language defines them.
 *
 * `@@iterator` is `%Array.prototype.values%` as an *own* property: `arguments` inherits from
 * `Object.prototype`, which has no iterator, so without it a spread would stop working the
 * moment this stopped being an array.
 *
 * The parameter *map* of a mapped `arguments` -- where `arguments[0]` and the first parameter
 * are two names for one binding -- is deliberately not here; the interpreter does not have it
 * either, and the two engines have to gain it together. `callee` still tells the two objects
 * apart, because that much is decidable from the function alone. */
static int kiln_c99_build_arguments_object(kiln_value *out) {
    size_t at;
    int status = kiln_c99_new_object(out);
    kiln_property *slot;
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_c99_object_of(*out)->arguments_brand = 1;
    /* `length` first, then the indices: integer keys sort ahead of string ones however they were
     * inserted, so what this order decides is that `length` precedes `callee`. */
    status = kiln_c99_own_slot(*out, "length", 6, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->accessor = 0;
    slot->value = kiln_c99_number((double)kiln_c99_frame_argc);
    slot->enumerable = 0;
    slot->writable = 1;
    slot->configurable = 1;
    for (at = 0; at < kiln_c99_frame_argc; ++at) {
        kiln_value key = kiln_c99_property_key(kiln_c99_number((double)at));
        if (key.tag != KILN_VALUE_STRING) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        size_t key_len = 0;
        const char *key_bytes = kiln_c99_string_ascii_bytes(key, &key_len);
        if (key_bytes == NULL) return KILN_C99_OUT_OF_MEMORY;
        status = kiln_c99_own_slot(*out, key_bytes, key_len, &slot);
        if (status != KILN_C99_OK) {
            return status;
        }
        slot->accessor = 0;
        slot->value = kiln_val_to_boundary(kiln_c99_frame_args[at]);
        slot->enumerable = 1;
        slot->writable = 1;
        slot->configurable = 1;
    }
    status = kiln_c99_own_slot(*out, "callee", 6, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    if (kiln_c99_frame_mapped_arguments) {
        slot->accessor = 0;
        slot->value = kiln_c99_frame_callee;
        slot->enumerable = 0;
        slot->writable = 1;
        slot->configurable = 1;
    } else {
        /* An unmapped `arguments` refuses to name its function: reading `callee` is a TypeError
         * rather than `undefined`, because a program reaching for it wants the caller identity
         * strict mode exists to withhold. */
        slot->accessor = 1;
        kiln_c99_object_of(*out)->accessor_count += 1;
        slot->getter = kiln_c99_method(KILN_C99_METHOD_THROW_TYPE_ERROR);
        slot->setter = kiln_c99_method(KILN_C99_METHOD_THROW_TYPE_ERROR);
        slot->enumerable = 0;
        slot->writable = 0;
        slot->configurable = 0;
    }
    status = kiln_c99_own_slot(*out, "@@iterator", 10, &slot);
    if (status != KILN_C99_OK) {
        return status;
    }
    slot->accessor = 0;
    slot->value = kiln_c99_method(KILN_C99_METHOD_ARRAY_VALUES);
    slot->enumerable = 0;
    slot->writable = 1;
    slot->configurable = 1;
    return KILN_C99_OK;
}

/* The `arguments` this frame can see: an arrow's captured object, or this call's own. */
static int kiln_c99_visible_arguments(kiln_value *out) {
    int status;
    if (kiln_c99_frame_is_arrow) {
        *out = kiln_c99_captured_arguments;
        return KILN_C99_OK;
    }
    if (kiln_c99_frame_arguments_object.tag == KILN_VALUE_OBJECT) {
        *out = kiln_c99_frame_arguments_object;
        return KILN_C99_OK;
    }
    status = kiln_c99_build_arguments_object(out);
    if (status != KILN_C99_OK) {
        return status;
    }
    kiln_c99_frame_arguments_object = *out;
    return KILN_C99_OK;
}

/* The realm's global object, created on first use.
 *
 * A real object rather than a tag, because a program *writes* to it: `globalThis.x = 1` has to be
 * found again by `globalThis.x`, and a builtin has nowhere to keep it. The names it starts with
 * are non-enumerable, which is what node does and what stops `Object.keys(globalThis)` from being
 * a list of the realm's furniture -- a program's own additions are enumerable, which is the
 * distinction that makes the question worth asking. */
#define KILN_C99_REALM_SLOT_GLOBALS UINT32_C(1)
#define KILN_C99_REALM_SLOT_PROCESS UINT32_C(2)
#define KILN_C99_REALM_SLOT_ATOMICS UINT32_C(3)
#define KILN_C99_REALM_SLOT_CONSOLE UINT32_C(4)
/* The prototype every `node:http` request object links to. A realm slot rather than a static,
 * because a realm owns it and the runtime archive already enumerates these slots as GC roots
 * (ADR 0041) -- and because one identity across translation units is what a prototype is for. */
#define KILN_C99_REALM_SLOT_INCOMING_MESSAGE UINT32_C(5)
