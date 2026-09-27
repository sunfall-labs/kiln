
/* The block arrives unzeroed, and every field the tracer and the finaliser read is written below
 * before anything here can allocate again.
 *
 * That is a narrower claim than the one `kiln_c99_gc_alloc_pointer_free` rests on, and a
 * different one. An array does have a tracer case, but what it reaches is bounded by a field the
 * allocation's caller sets first: it traces `elements[0 .. count)`, and `count` is zero from here
 * until elements are written in one at a time. Zeroing the rest bought nothing and cost a
 * `memset` of the array header and its eight inline slots -- 152 bytes, twice per `split`.
 * Whatever else changes here, `elements`, `count`, `properties` and `match_input` must be set
 * before anything that can allocate. */
static int kiln_c99_new_array(kiln_value *out) {
    kiln_array *array = (kiln_array *)kiln_c99_gc_alloc_hot(KILN_GC_ARRAY, sizeof *array, 0);
    if (array == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    array->elements = NULL;
    array->count = 0;
    array->capacity = 0;
    array->properties = kiln_c99_undefined();
    array->frozen = 0;
    array->not_extensible = 0;
    array->match_index = -1;
    array->match_input = kiln_c99_undefined();
    array->match_groups = kiln_c99_undefined();
    array->is_numeric = 1;
    *out = kiln_c99_value(KILN_VALUE_ARRAY, (uint64_t)(uintptr_t)array, NULL, 0);
    return KILN_C99_OK;
}

/* A capacity of eight or fewer fits the array's own `inline_elements` -- the shape
 * `kiln_c99_val_array_push_fast` (boxed.c) already grows out of and the GC already knows not to
 * free, both by asking `kiln_c99_array_elements_owned` first. */
/* Grow an array's element storage to hold `needed`. The one place that knows an array may be
 * using storage the allocator never handed out -- its own `inline_elements`, or the run inside
 * its block: the general `kiln_c99_reserve` would hand either to `realloc`, which is exactly the
 * crash a bundled `ajv` produced the day small arrays were first given their inline storage.
 * Every element growth in the runtime goes through here.
 *
 * Capacity is not reachability. This raises `capacity`; it does not raise `count`, and the
 * collector traces an array's first `count` elements and no further -- so anything a caller
 * writes into the room this just made is invisible to the collector until `count` is published
 * over it. A caller that fills such a gap with values whose conversion allocates (any string of
 * more than one byte allocates, via `kiln_val_from_boundary`) must therefore make the gap
 * traceable and publish `count` *before* the filling loop, not after it; otherwise the first
 * conversion collects what the previous one stored. `splice` and `unshift` (dispatch.c) fill a
 * gap with `KILN_VAL_UNDEFINED` and publish the final count before converting anything, and the
 * regexp match array (boxed.c) publishes each group as it lands. This cost three wrong answers
 * before it was written down; `conformance/native/array-insert-gc.js` pins it, and only
 * `mise run native-stress` makes it fail. */
static int kiln_c99_array_reserve(kiln_array *array, size_t needed) {
    size_t grown;
    kiln_val *moved;
    if (needed <= array->capacity) {
        return KILN_C99_OK;
    }
    if (array->elements == NULL && needed <= sizeof array->inline_elements / sizeof array->inline_elements[0]) {
        /* An array's first elements live in the array: a parsed `[1, 2, 3]` or a `push` onto
         * `[]` used to malloc four slots it had room for already. */
        array->elements = array->inline_elements;
        array->capacity = sizeof array->inline_elements / sizeof array->inline_elements[0];
        return KILN_C99_OK;
    }
    grown = array->capacity == 0 ? 4 : array->capacity;
    while (grown < needed) {
        if (grown > ((size_t)-1) / 2 / sizeof(kiln_val)) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        grown *= 2;
    }
    if (array->elements != NULL && !kiln_c99_array_elements_owned(array)) {
        moved = (kiln_val *)malloc(grown * sizeof(kiln_val));
        if (moved == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        memcpy(moved, array->elements, array->capacity * sizeof(kiln_val));
    } else {
        moved = (kiln_val *)realloc(array->elements, grown * sizeof(kiln_val));
        if (moved == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
    }
    array->elements = moved;
    array->capacity = grown;
    return KILN_C99_OK;
}

/* How many elements past `inline_elements` an array will carry inside its own block rather than
 * in a separate `malloc`. Above this the array is large enough that one `malloc` next to one
 * collected block is the cheaper arrangement, and a block that big would leave the size classes
 * for the general allocator's large path anyway. */
#define KILN_C99_ARRAY_INBLOCK_MAX ((size_t)64)

static int kiln_c99_new_array_with_capacity(kiln_value *out, size_t capacity, int is_numeric) {
    /* A count known here buys the elements a place inside the array's own block: one allocation
     * instead of a collected block plus a `malloc`, and one free instead of a `free` inside the
     * collector's finaliser. `split` on a line with nine fields paid both, per call. */
    size_t inline_capacity = sizeof(((kiln_array *)0)->inline_elements) / sizeof(kiln_val);
    size_t in_block = (capacity > inline_capacity && capacity <= KILN_C99_ARRAY_INBLOCK_MAX)
                          ? capacity
                          : 0;
    kiln_array *array =
        (kiln_array *)kiln_c99_gc_alloc_hot(KILN_GC_ARRAY, sizeof *array + in_block * sizeof(kiln_val), 0);
    if (array == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    array->elements = NULL;
    array->count = 0;
    array->capacity = 0;
    array->properties = kiln_c99_undefined();
    array->frozen = 0;
    array->not_extensible = 0;
    array->match_index = -1;
    array->match_input = kiln_c99_undefined();
    array->match_groups = kiln_c99_undefined();
    array->is_numeric = is_numeric;
    if (in_block > 0) {
        array->elements = (kiln_val *)(array + 1);
        array->capacity = in_block;
    } else if (capacity > 0) {
        if (capacity <= inline_capacity) {
            array->elements = array->inline_elements;
            array->capacity = inline_capacity;
        } else {
            int status = kiln_c99_array_reserve(array, capacity);
            if (status != KILN_C99_OK) return status;
        }
    }
    *out = kiln_c99_value(KILN_VALUE_ARRAY, (uint64_t)(uintptr_t)array, NULL, 0);
    return KILN_C99_OK;
}

static size_t kiln_c99_typed_width(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8:
    case KILN_TYPED_UINT8:
    case KILN_TYPED_UINT8_CLAMPED: return 1;
    case KILN_TYPED_INT16:
    case KILN_TYPED_UINT16: return 2;
    case KILN_TYPED_INT32:
    case KILN_TYPED_UINT32:
    case KILN_TYPED_FLOAT32: return 4;
    case KILN_TYPED_FLOAT64:
    case KILN_TYPED_BIG_INT64:
    case KILN_TYPED_BIG_UINT64: return 8;
    }
    return 1;
}

static const char *kiln_c99_typed_name(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8: return "Int8Array";
    case KILN_TYPED_UINT8: return "Uint8Array";
    case KILN_TYPED_UINT8_CLAMPED: return "Uint8ClampedArray";
    case KILN_TYPED_INT16: return "Int16Array";
    case KILN_TYPED_UINT16: return "Uint16Array";
    case KILN_TYPED_INT32: return "Int32Array";
    case KILN_TYPED_UINT32: return "Uint32Array";
    case KILN_TYPED_FLOAT32: return "Float32Array";
    case KILN_TYPED_FLOAT64: return "Float64Array";
    case KILN_TYPED_BIG_INT64: return "BigInt64Array";
    case KILN_TYPED_BIG_UINT64: return "BigUint64Array";
    }
    return "TypedArray";
}

static int kiln_c99_builtin_typed_kind(uint64_t builtin, kiln_typed_kind *kind) {
    switch (builtin) {
    case KILN_C99_BUILTIN_INT8_ARRAY: *kind = KILN_TYPED_INT8; return 1;
    case KILN_C99_BUILTIN_UINT8_ARRAY: *kind = KILN_TYPED_UINT8; return 1;
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY: *kind = KILN_TYPED_UINT8_CLAMPED; return 1;
    case KILN_C99_BUILTIN_INT16_ARRAY: *kind = KILN_TYPED_INT16; return 1;
    case KILN_C99_BUILTIN_UINT16_ARRAY: *kind = KILN_TYPED_UINT16; return 1;
    case KILN_C99_BUILTIN_INT32_ARRAY: *kind = KILN_TYPED_INT32; return 1;
    case KILN_C99_BUILTIN_UINT32_ARRAY: *kind = KILN_TYPED_UINT32; return 1;
    case KILN_C99_BUILTIN_FLOAT32_ARRAY: *kind = KILN_TYPED_FLOAT32; return 1;
    case KILN_C99_BUILTIN_FLOAT64_ARRAY: *kind = KILN_TYPED_FLOAT64; return 1;
    case KILN_C99_BUILTIN_BIG_INT64_ARRAY: *kind = KILN_TYPED_BIG_INT64; return 1;
    case KILN_C99_BUILTIN_BIG_UINT64_ARRAY: *kind = KILN_TYPED_BIG_UINT64; return 1;
    default: return 0;
    }
}

static uint64_t kiln_c99_typed_constructor(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8: return KILN_C99_BUILTIN_INT8_ARRAY;
    case KILN_TYPED_UINT8: return KILN_C99_BUILTIN_UINT8_ARRAY;
    case KILN_TYPED_UINT8_CLAMPED: return KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY;
    case KILN_TYPED_INT16: return KILN_C99_BUILTIN_INT16_ARRAY;
    case KILN_TYPED_UINT16: return KILN_C99_BUILTIN_UINT16_ARRAY;
    case KILN_TYPED_INT32: return KILN_C99_BUILTIN_INT32_ARRAY;
    case KILN_TYPED_UINT32: return KILN_C99_BUILTIN_UINT32_ARRAY;
    case KILN_TYPED_FLOAT32: return KILN_C99_BUILTIN_FLOAT32_ARRAY;
    case KILN_TYPED_FLOAT64: return KILN_C99_BUILTIN_FLOAT64_ARRAY;
    case KILN_TYPED_BIG_INT64: return KILN_C99_BUILTIN_BIG_INT64_ARRAY;
    case KILN_TYPED_BIG_UINT64: return KILN_C99_BUILTIN_BIG_UINT64_ARRAY;
    }
    return KILN_C99_BUILTIN_UINT8_ARRAY;
}

static uint64_t kiln_c99_typed_prototype(kiln_typed_kind kind) {
    switch (kind) {
    case KILN_TYPED_INT8: return KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT8: return KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT8_CLAMPED: return KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE;
    case KILN_TYPED_INT16: return KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT16: return KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE;
    case KILN_TYPED_INT32: return KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE;
    case KILN_TYPED_UINT32: return KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE;
    case KILN_TYPED_FLOAT32: return KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE;
    case KILN_TYPED_FLOAT64: return KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE;
    case KILN_TYPED_BIG_INT64: return KILN_C99_BUILTIN_BIG_INT64_ARRAY_PROTOTYPE;
    case KILN_TYPED_BIG_UINT64: return KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE;
    }
    return KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE;
}

static int kiln_c99_typed_prototype_kind(uint64_t builtin, kiln_typed_kind *kind) {
    switch (builtin) {
    case KILN_C99_BUILTIN_INT8_ARRAY_PROTOTYPE: *kind = KILN_TYPED_INT8; return 1;
    case KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE: *kind = KILN_TYPED_UINT8; return 1;
    case KILN_C99_BUILTIN_UINT8_CLAMPED_ARRAY_PROTOTYPE:
        *kind = KILN_TYPED_UINT8_CLAMPED;
        return 1;
    case KILN_C99_BUILTIN_INT16_ARRAY_PROTOTYPE: *kind = KILN_TYPED_INT16; return 1;
    case KILN_C99_BUILTIN_UINT16_ARRAY_PROTOTYPE: *kind = KILN_TYPED_UINT16; return 1;
    case KILN_C99_BUILTIN_INT32_ARRAY_PROTOTYPE: *kind = KILN_TYPED_INT32; return 1;
    case KILN_C99_BUILTIN_UINT32_ARRAY_PROTOTYPE: *kind = KILN_TYPED_UINT32; return 1;
    case KILN_C99_BUILTIN_FLOAT32_ARRAY_PROTOTYPE: *kind = KILN_TYPED_FLOAT32; return 1;
    case KILN_C99_BUILTIN_FLOAT64_ARRAY_PROTOTYPE: *kind = KILN_TYPED_FLOAT64; return 1;
    case KILN_C99_BUILTIN_BIG_INT64_ARRAY_PROTOTYPE: *kind = KILN_TYPED_BIG_INT64; return 1;
    case KILN_C99_BUILTIN_BIG_UINT64_ARRAY_PROTOTYPE: *kind = KILN_TYPED_BIG_UINT64; return 1;
    default: return 0;
    }
}

/* Byte blocks of typed buffers that died, kept for a buffer of the same size: a server or a
 * `Buffer.concat` loop makes buffers of one size over and over, and a large block otherwise
 * goes back to the allocator and returns through the kernel's page reclaim each time. */
#define KILN_C99_BYTES_CACHE 8
#define KILN_C99_BYTES_CACHE_MIN ((size_t)64 * 1024)
typedef struct kiln_bytes_entry {
    uint8_t *bytes;
    size_t size;
} kiln_bytes_entry;
/* Per thread, not per realm: the blocks are plain memory that hold no realm's values, any realm
 * on the thread may reuse one, and `kiln_c99_bytes_retire` runs from a sweep -- including the
 * teardown sweep, where no realm is current to name a context. */
static _Thread_local kiln_bytes_entry kiln_c99_bytes_cache[KILN_C99_BYTES_CACHE];

/* Called by the collector when a typed buffer dies: keep the block if it is worth keeping. */
static void kiln_c99_bytes_retire(uint8_t *bytes, size_t size) {
    size_t i;
    if (bytes != NULL && size >= KILN_C99_BYTES_CACHE_MIN) {
        for (i = 0; i < KILN_C99_BYTES_CACHE; ++i) {
            if (kiln_c99_bytes_cache[i].bytes == NULL) {
                kiln_c99_bytes_cache[i].bytes = bytes;
                kiln_c99_bytes_cache[i].size = size;
                return;
            }
        }
    }
    free(bytes);
}

/* `zeroed` clear means the caller writes every byte before anything can read one. */
static int kiln_c99_new_typed_buffer_ex(kiln_value *out, size_t byte_length, int zeroed) {
    kiln_typed_buffer *buffer =
        (kiln_typed_buffer *)kiln_c99_gc_alloc(KILN_GC_TYPED_BUFFER, sizeof *buffer);
    size_t i;
    if (buffer == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    buffer->bytes = NULL;
    if (byte_length >= KILN_C99_BYTES_CACHE_MIN) {
        for (i = 0; i < KILN_C99_BYTES_CACHE; ++i) {
            if (kiln_c99_bytes_cache[i].bytes != NULL && kiln_c99_bytes_cache[i].size == byte_length) {
                buffer->bytes = kiln_c99_bytes_cache[i].bytes;
                kiln_c99_bytes_cache[i].bytes = NULL;
                if (zeroed) memset(buffer->bytes, 0, byte_length);
                break;
            }
        }
    }
    if (buffer->bytes == NULL) {
        buffer->bytes = zeroed ? (uint8_t *)calloc(byte_length == 0 ? 1 : byte_length, 1)
                               : (uint8_t *)malloc(byte_length == 0 ? 1 : byte_length);
        /* The block is the buffer's memory as much as the cell is: counted toward the next
         * collection, or a loop of large buffers would run to hundreds of megabytes of dead
         * blocks before the cells that own them were ever swept. */
        kiln_c99_gc_note_external(byte_length);
    }
    if (buffer->bytes == NULL) {
        kiln_c99_gc_release(buffer);
        return KILN_C99_OUT_OF_MEMORY;
    }
    buffer->byte_length = byte_length;
    buffer->owns_bytes = 1;
    *out = kiln_c99_value(KILN_VALUE_ARRAY_BUFFER, (uint64_t)(uintptr_t)buffer, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_new_typed_buffer(kiln_value *out, size_t byte_length) {
    return kiln_c99_new_typed_buffer_ex(out, byte_length, 1);
}

/* Where a detached buffer points: never NULL, so a `memcpy` of its zero bytes is defined, and
 * never owned, so the finalizer frees nothing. Read-only: its length is zero, so nothing may write
 * through it, and a bug that did would fault here rather than corrupt a neighbour. */
static const uint8_t kiln_c99_detached_bytes[1] = {0};

/* `__kilnArrayBufferDetach(buffer, length, preserveResizability)`: the one primitive a transfer
 * needs (ADR 0065, representation; ADR 0070 primitive C). The backing store *moves* to a new
 * ArrayBuffer -- the same block, realloc'd only when the length changes -- and the source is left
 * detached: zero bytes, and every view over it empty from its next use
 * (`kiln_c99_typed_array_of`). Every check a program can observe is made above this, in
 * `kiln-builtins/src/arraybuffer.ts` and `structured_clone.ts`; the two here keep a caller that
 * skipped them from corrupting a buffer. This backend has no resizable buffer, so the third
 * argument has nothing to preserve. Called by the bootstrap only: captured and deleted before
 * user code. */
static int kiln_c99_array_buffer_detach_raw(kiln_value *out, const kiln_value *args, size_t argc,
                                            kiln_value *thrown) {
    kiln_typed_buffer *source;
    kiln_typed_buffer *moved;
    size_t length;
    int status;
    if (argc == 0 || args[0].tag != KILN_VALUE_ARRAY_BUFFER) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "__kilnArrayBufferDetach: not an ArrayBuffer");
    }
    if (kiln_c99_typed_buffer_of(args[0])->detached) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot perform ArrayBuffer transfer on a detached ArrayBuffer");
    }
    length = kiln_c99_typed_buffer_of(args[0])->byte_length;
    if (argc > 1 && args[1].tag == KILN_VALUE_NUMBER) {
        status = kiln_c99_to_index(args[1], &length, thrown);
        if (status != KILN_C99_OK) return status;
    }
    /* Allocated before the source is read again: the allocation may collect, and the source is
     * reachable only through `args`, which the caller roots. */
    moved = (kiln_typed_buffer *)kiln_c99_gc_alloc(KILN_GC_TYPED_BUFFER, sizeof *moved);
    if (moved == NULL) return KILN_C99_OUT_OF_MEMORY;
    source = kiln_c99_typed_buffer_of(args[0]);
    moved->bytes = source->bytes;
    moved->byte_length = source->byte_length;
    moved->owns_bytes = source->owns_bytes;
    if (length != source->byte_length || !source->owns_bytes) {
        /* A new length, or bytes this runtime does not own (a borrowed FFI view): the result
         * needs its own block. Growth is zero-filled, as `transfer(n)` specifies. */
        uint8_t *bytes;
        size_t kept = length < source->byte_length ? length : source->byte_length;
        if (source->owns_bytes) {
            bytes = (uint8_t *)realloc(source->bytes, length == 0 ? 1 : length);
        } else {
            bytes = (uint8_t *)malloc(length == 0 ? 1 : length);
            if (bytes != NULL && kept != 0) memcpy(bytes, source->bytes, kept);
        }
        if (bytes == NULL) {
            moved->bytes = (uint8_t *)kiln_c99_detached_bytes;
            moved->byte_length = 0;
            moved->owns_bytes = 0;
            return KILN_C99_OUT_OF_MEMORY;
        }
        if (length > kept) memset(bytes + kept, 0, length - kept);
        if (length > source->byte_length) kiln_c99_gc_note_external(length - source->byte_length);
        moved->bytes = bytes;
        moved->byte_length = length;
        moved->owns_bytes = 1;
    }
    source->bytes = (uint8_t *)kiln_c99_detached_bytes;
    source->byte_length = 0;
    source->owns_bytes = 0;
    source->detached = 1;
    *out = kiln_c99_value(KILN_VALUE_ARRAY_BUFFER, (uint64_t)(uintptr_t)moved, NULL, 0);
    return KILN_C99_OK;
}

/* ECMAScript's ToIndex: missing/undefined/NaN become zero, but negative, infinite and unsafe
 * integer values are RangeErrors.  The size_t bound is separate because a 32-bit C target is
 * narrower than JavaScript's maximum safe integer.  Checking both before the cast keeps the
 * generated C out of the implementation-defined/undefined floating-to-integer range. */
static int kiln_c99_to_index(kiln_value value, size_t *out, kiln_value *thrown) {
    double numeric = kiln_c99_as_number(value);
    double integral;
    if (isnan(numeric) || numeric == 0.0) {
        *out = 0;
        return KILN_C99_OK;
    }
    integral = trunc(numeric);
    if (!isfinite(integral) || integral < 0.0 || integral > 9007199254740991.0 ||
        integral > (double)((size_t)-1)) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "invalid array buffer or typed array length");
    }
    *out = (size_t)integral;
    return KILN_C99_OK;
}

static double kiln_c99_typed_get(kiln_typed_array *view, size_t index) {
    size_t width = kiln_c99_typed_width(view->kind);
    size_t at;
    const uint8_t *bytes;
    uint32_t bits32;
    uint64_t bits64;
    int32_t signed32;
    int16_t signed16;
    uint16_t unsigned16;
    float float32;
    double float64;
    if (index >= view->length || view->byte_offset > view->buffer->byte_length ||
        index > (((size_t)-1) - view->byte_offset) / width) {
        return NAN;
    }
    at = view->byte_offset + index * width;
    if (at > view->buffer->byte_length || width > view->buffer->byte_length - at) {
        return NAN;
    }
    bytes = view->buffer->bytes + at;
    switch (view->kind) {
    case KILN_TYPED_INT8: return (double)(int8_t)bytes[0];
    case KILN_TYPED_UINT8:
    case KILN_TYPED_UINT8_CLAMPED: return (double)bytes[0];
    case KILN_TYPED_INT16:
        signed16 = (int16_t)((uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8));
        return (double)signed16;
    case KILN_TYPED_UINT16:
        unsigned16 = (uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8);
        return (double)unsigned16;
    case KILN_TYPED_INT32:
        bits32 = (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
                 ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
        memcpy(&signed32, &bits32, sizeof signed32);
        return (double)signed32;
    case KILN_TYPED_UINT32:
        bits32 = (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
                 ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
        return (double)bits32;
    case KILN_TYPED_FLOAT32:
        bits32 = (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) |
                 ((uint32_t)bytes[2] << 16) | ((uint32_t)bytes[3] << 24);
        memcpy(&float32, &bits32, sizeof float32);
        return (double)float32;
    case KILN_TYPED_FLOAT64:
        bits64 = (uint64_t)bytes[0] | ((uint64_t)bytes[1] << 8) |
                 ((uint64_t)bytes[2] << 16) | ((uint64_t)bytes[3] << 24) |
                 ((uint64_t)bytes[4] << 32) | ((uint64_t)bytes[5] << 40) |
                 ((uint64_t)bytes[6] << 48) | ((uint64_t)bytes[7] << 56);
        memcpy(&float64, &bits64, sizeof float64);
        return float64;
    }
    return NAN;
}

/* Whether a view's elements are BigInts rather than Numbers. */
static int kiln_c99_typed_kind_is_bigint(kiln_typed_kind kind) {
    return kind == KILN_TYPED_BIG_INT64 || kind == KILN_TYPED_BIG_UINT64;
}

/* The eight bytes of element `index` of a view, little-endian as every view here is stored, or
 * NULL past its end: the storage of a BigInt64Array or BigUint64Array element. */
static uint8_t *kiln_c99_typed_bits_at(kiln_typed_array *view, size_t index) {
    size_t at;
    if (index >= view->length || view->byte_offset > view->buffer->byte_length ||
        index > (((size_t)-1) - view->byte_offset) / 8) {
        return NULL;
    }
    at = view->byte_offset + index * 8;
    if (at > view->buffer->byte_length || 8 > view->buffer->byte_length - at) return NULL;
    return view->buffer->bytes + at;
}

/* Element `index` as the language reads it: a BigInt from the two BigInt kinds, whose elements
 * `kiln_c99_typed_get` has no double for, and a Number from the rest. */
static int kiln_c99_typed_element(kiln_typed_array *view, size_t index, kiln_value *out) {
    const uint8_t *bytes;
    uint64_t bits = 0;
    size_t shift;
    if (!kiln_c99_typed_kind_is_bigint(view->kind)) {
        *out = kiln_c99_number(kiln_c99_typed_get(view, index));
        return KILN_C99_OK;
    }
    bytes = kiln_c99_typed_bits_at(view, index);
    if (bytes == NULL) {
        *out = kiln_c99_number(NAN);
        return KILN_C99_OK;
    }
    for (shift = 0; shift < 8; ++shift) bits |= (uint64_t)bytes[shift] << (8 * shift);
    return kiln_c99_u64_bigint(bits,
                               view->kind == KILN_TYPED_BIG_INT64 && (bits >> 63) != 0, out);
}

/* Store a language value as element `index`: `ToBigInt` and the low sixty-four bits for the
 * two BigInt kinds, `ToNumber` for the rest -- the conversion that can call back into the
 * program, and throws for a Number stored into a BigInt view. */
static int kiln_c99_typed_store(kiln_host *host, kiln_typed_array *view, size_t index,
                                kiln_value value, int depth, kiln_value *thrown) {
    uint8_t *bytes;
    uint64_t bits;
    size_t shift;
    int status;
    if (!kiln_c99_typed_kind_is_bigint(view->kind)) {
        double number;
        status = kiln_c99_to_number_checked(host, value, &number, depth, thrown);
        if (status == KILN_C99_OK) kiln_c99_typed_set(view, index, number);
        return status;
    }
    status = kiln_c99_to_bigint64_bits(host, value, &bits, depth, thrown);
    if (status != KILN_C99_OK) return status;
    bytes = kiln_c99_typed_bits_at(view, index);
    if (bytes == NULL) return KILN_C99_OK;
    for (shift = 0; shift < 8; ++shift) bytes[shift] = (uint8_t)(bits >> (8 * shift));
    return KILN_C99_OK;
}

static void kiln_c99_typed_set(kiln_typed_array *view, size_t index, double value) {
    size_t width = kiln_c99_typed_width(view->kind);
    size_t at;
    uint8_t *bytes;
    uint32_t bits32;
    uint64_t bits64;
    int32_t signed32;
    int16_t signed16;
    uint16_t unsigned16;
    float float32;
    double float64;
    double rounded;
    if (index >= view->length || view->byte_offset > view->buffer->byte_length ||
        index > (((size_t)-1) - view->byte_offset) / width) {
        return;
    }
    at = view->byte_offset + index * width;
    if (at > view->buffer->byte_length || width > view->buffer->byte_length - at) {
        return;
    }
    bytes = view->buffer->bytes + at;
    switch (view->kind) {
    case KILN_TYPED_INT8: bytes[0] = (uint8_t)kiln_c99_to_uint32(value); return;
    case KILN_TYPED_UINT8:
        bytes[0] = (uint8_t)kiln_c99_to_uint32(value);
        return;
    case KILN_TYPED_UINT8_CLAMPED:
        if (isnan(value) || value <= 0.0) {
            bytes[0] = 0;
        } else if (value >= 255.0) {
            bytes[0] = 255;
        } else {
            rounded = floor(value);
            if (value - rounded > 0.5 ||
                (value - rounded == 0.5 && fmod(rounded, 2.0) != 0.0)) {
                rounded += 1.0;
            }
            bytes[0] = (uint8_t)rounded;
        }
        return;
    case KILN_TYPED_INT16:
        signed16 = (int16_t)kiln_c99_to_uint32(value);
        unsigned16 = (uint16_t)signed16;
        bytes[0] = (uint8_t)unsigned16;
        bytes[1] = (uint8_t)(unsigned16 >> 8);
        return;
    case KILN_TYPED_UINT16:
        unsigned16 = (uint16_t)kiln_c99_to_uint32(value);
        bytes[0] = (uint8_t)unsigned16;
        bytes[1] = (uint8_t)(unsigned16 >> 8);
        return;
    case KILN_TYPED_INT32:
        signed32 = (int32_t)kiln_c99_to_uint32(value);
        memcpy(&bits32, &signed32, sizeof bits32);
        bytes[0] = (uint8_t)bits32;
        bytes[1] = (uint8_t)(bits32 >> 8);
        bytes[2] = (uint8_t)(bits32 >> 16);
        bytes[3] = (uint8_t)(bits32 >> 24);
        return;
    case KILN_TYPED_UINT32:
        bits32 = kiln_c99_to_uint32(value);
        bytes[0] = (uint8_t)bits32;
        bytes[1] = (uint8_t)(bits32 >> 8);
        bytes[2] = (uint8_t)(bits32 >> 16);
        bytes[3] = (uint8_t)(bits32 >> 24);
        return;
    case KILN_TYPED_FLOAT32:
        float32 = (float)value;
        memcpy(&bits32, &float32, sizeof bits32);
        bytes[0] = (uint8_t)bits32;
        bytes[1] = (uint8_t)(bits32 >> 8);
        bytes[2] = (uint8_t)(bits32 >> 16);
        bytes[3] = (uint8_t)(bits32 >> 24);
        return;
    case KILN_TYPED_FLOAT64:
        float64 = value;
        memcpy(&bits64, &float64, sizeof bits64);
        bytes[0] = (uint8_t)bits64;
        bytes[1] = (uint8_t)(bits64 >> 8);
        bytes[2] = (uint8_t)(bits64 >> 16);
        bytes[3] = (uint8_t)(bits64 >> 24);
        bytes[4] = (uint8_t)(bits64 >> 32);
        bytes[5] = (uint8_t)(bits64 >> 40);
        bytes[6] = (uint8_t)(bits64 >> 48);
        bytes[7] = (uint8_t)(bits64 >> 56);
        return;
    }
}

static int kiln_c99_new_typed_array(kiln_value *out, kiln_typed_kind kind,
                                    kiln_value source, size_t byte_offset, size_t length,
                                    int has_length, kiln_value *thrown) {
    size_t width = kiln_c99_typed_width(kind);
    kiln_value buffer_value;
    kiln_typed_array *view;
    size_t count;
    size_t index;
    int status;
    if (source.tag == KILN_VALUE_ARRAY_BUFFER) {
        kiln_typed_buffer *buffer = kiln_c99_typed_buffer_of(source);
        if (buffer->detached) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                        "Cannot perform Construct on a detached ArrayBuffer");
        }
        if (byte_offset % width != 0 || byte_offset > buffer->byte_length ||
            (!has_length && (buffer->byte_length - byte_offset) % width != 0) ||
            (has_length && (length > (buffer->byte_length - byte_offset) / width))) {
            return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                        "invalid typed array length or offset");
        }
        count = has_length ? length : (buffer->byte_length - byte_offset) / width;
        view = (kiln_typed_array *)kiln_c99_gc_alloc(KILN_GC_TYPED_ARRAY, sizeof *view);
        if (view == NULL) {
            return KILN_C99_OUT_OF_MEMORY;
        }
        view->buffer = buffer;
        view->byte_offset = byte_offset;
        view->length = count;
        view->kind = kind;
        view->is_buffer = 0;
        *out = kiln_c99_value(KILN_VALUE_TYPED_ARRAY, (uint64_t)(uintptr_t)view, NULL, 0);
        return KILN_C99_OK;
    }
    if (source.tag == KILN_VALUE_ARRAY) {
        count = kiln_c99_array_of(source)->count;
    } else if (source.tag == KILN_VALUE_TYPED_ARRAY) {
        count = kiln_c99_typed_array_of(source)->length;
    } else {
        status = kiln_c99_to_index(source, &count, thrown);
        if (status != KILN_C99_OK) {
            return status;
        }
    }
    if (count > ((size_t)-1) / width) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_new_typed_buffer(&buffer_value, count * width);
    if (status != KILN_C99_OK) {
        return status;
    }
    view = (kiln_typed_array *)kiln_c99_gc_alloc(KILN_GC_TYPED_ARRAY, sizeof *view);
    if (view == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    view->buffer = kiln_c99_typed_buffer_of(buffer_value);
    view->byte_offset = 0;
    view->length = count;
    view->kind = kind;
    view->is_buffer = 0;
    for (index = 0; index < count; ++index) {
        double item;
        if (kiln_c99_typed_kind_is_bigint(kind) ||
            (source.tag == KILN_VALUE_TYPED_ARRAY &&
             kiln_c99_typed_kind_is_bigint(kiln_c99_typed_array_of(source)->kind))) {
            /* Either side a BigInt view: through the conversions that tell the two numeric
             * types apart, and a TypeError where they meet. */
            kiln_value element = kiln_c99_undefined();
            int status = KILN_C99_OK;
            if (source.tag == KILN_VALUE_ARRAY)
                element = kiln_val_to_boundary(kiln_c99_array_of(source)->elements[index]);
            else if (source.tag == KILN_VALUE_TYPED_ARRAY)
                status = kiln_c99_typed_element(kiln_c99_typed_array_of(source), index, &element);
            else
                continue;
            if (status == KILN_C99_OK)
                status = kiln_c99_typed_store(NULL, view, index, element, 0, thrown);
            if (status != KILN_C99_OK) return status;
            continue;
        }
        item = source.tag == KILN_VALUE_ARRAY
                   ? kiln_c99_as_number(kiln_val_to_boundary(kiln_c99_array_of(source)->elements[index]))
               : source.tag == KILN_VALUE_TYPED_ARRAY
                   ? kiln_c99_typed_get(kiln_c99_typed_array_of(source), index)
                   : 0.0;
        kiln_c99_typed_set(view, index, item);
    }
    *out = kiln_c99_value(KILN_VALUE_TYPED_ARRAY, (uint64_t)(uintptr_t)view, NULL, 0);
    return KILN_C99_OK;
}

/* An all-zero view of `count` elements over a buffer of its own.
 *
 * `map`, `filter` and `slice` each answer with a *view of the receiver's kind* rather than with a
 * plain array: `new Uint8Array([3, 1, 2]).map(f)` is a `Uint8Array`, and an Array would lose both
 * the width and the wrapping that decide what the values mean. */
/* `zeroed` clear means the caller writes every element before anything can read one. */
static int kiln_c99_typed_alloc_ex(kiln_value *out, kiln_typed_kind kind, size_t count, int zeroed) {
    size_t width = kiln_c99_typed_width(kind);
    kiln_value buffer;
    kiln_typed_array *view;
    int status;
    if (count > ((size_t)-1) / width) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    status = kiln_c99_new_typed_buffer_ex(&buffer, count * width, zeroed);
    if (status != KILN_C99_OK) {
        return status;
    }
    view = (kiln_typed_array *)kiln_c99_gc_alloc(KILN_GC_TYPED_ARRAY, sizeof *view);
    if (view == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    view->buffer = kiln_c99_typed_buffer_of(buffer);
    view->byte_offset = 0;
    view->length = count;
    view->kind = kind;
    view->is_buffer = 0;
    *out = kiln_c99_value(KILN_VALUE_TYPED_ARRAY, (uint64_t)(uintptr_t)view, NULL, 0);
    return KILN_C99_OK;
}

static int kiln_c99_typed_alloc(kiln_value *out, kiln_typed_kind kind, size_t count) {
    return kiln_c99_typed_alloc_ex(out, kind, count, 1);
}

/* `new DataView(buffer[, byteOffset[, byteLength]])`.
 *
 * The two failures are different errors and a program tells them apart: a first argument that is
 * not an ArrayBuffer is a TypeError, and a window that does not fit inside the buffer is a
 * RangeError. Both messages are the interpreter's, word for word -- `error.message` is
 * program-visible, so a fault the two engines spell differently is a divergence like any other.
 *
 * There is no alignment rule to check, which is the other half of why this type exists: byte
 * offset 1 is a legal DataView and an illegal `Uint16Array`, and `kiln_c99_new_typed_array`'s
 * `byte_offset % width` test has no counterpart here. */
static int kiln_c99_new_data_view(kiln_value *out, kiln_value source, size_t byte_offset,
                                  size_t byte_length, int has_length, kiln_value *thrown) {
    kiln_typed_buffer *buffer;
    kiln_data_view *view;
    if (source.tag != KILN_VALUE_ARRAY_BUFFER) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Method called on an incompatible receiver");
    }
    buffer = kiln_c99_typed_buffer_of(source);
    if (buffer->detached) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_TYPE_ERROR,
                                    "Cannot perform DataView constructor on a detached ArrayBuffer");
    }
    if (byte_offset > buffer->byte_length) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Offset is outside the bounds of the DataView");
    }
    /* A subtraction from a quantity already known to be large enough, never `byte_offset +
     * byte_length`: both are size_t, that sum can wrap, and a wrapped sum compares *small* --
     * which would admit exactly the window this test exists to reject. */
    if (has_length && byte_length > buffer->byte_length - byte_offset) {
        return kiln_c99_throw_error(thrown, KILN_C99_BUILTIN_RANGE_ERROR,
                                    "Offset is outside the bounds of the DataView");
    }
    view = (kiln_data_view *)kiln_c99_gc_alloc(KILN_GC_DATA_VIEW, sizeof *view);
    if (view == NULL) {
        return KILN_C99_OUT_OF_MEMORY;
    }
    view->buffer = buffer;
    view->byte_offset = byte_offset;
    /* With no explicit length the view runs to the end of the buffer, and keeps running there:
     * nothing here can resize a buffer, so the length is settled once rather than tracked. */
    view->byte_length = has_length ? byte_length : buffer->byte_length - byte_offset;
    *out = kiln_c99_value(KILN_VALUE_DATA_VIEW, (uint64_t)(uintptr_t)view, NULL, 0);
    return KILN_C99_OK;
}

/* Where an access of `width` bytes at `offset` lands, or zero when it does not fit entirely
 * inside the window.
 *
 * Every bound is a subtraction, for the reason given in the constructor: `offset + width` on two
 * size_t values can wrap, and the wrapped sum is small enough to pass a naive comparison. Written
 * this way, an access that ends *exactly* at the last byte is admitted (`offset == byte_length -
 * width`) and one byte further is not. */
static int kiln_c99_data_view_at(kiln_data_view *view, size_t offset, size_t width,
                                 uint8_t **out) {
    if (view->byte_length < width || offset > view->byte_length - width) {
        return 0;
    }
    /* The window was measured against the buffer when the view was made and no buffer here can
     * shrink afterwards. Asked again anyway, because this is the test that decides whether these
     * particular bytes are ours to touch, and a mistake in it is a read out of bounds rather than
     * a wrong number. */
    if (view->byte_offset > view->buffer->byte_length ||
        view->byte_length > view->buffer->byte_length - view->byte_offset) {
        return 0;
    }
    *out = view->buffer->bytes + view->byte_offset + offset;
    return 1;
}

/* Gather `width` bytes into an unsigned integer, most significant byte first unless the call
 * asked for little-endian.
 *
 * BIG-ENDIAN IS THE DEFAULT, and it is the one thing about this type that surprises everybody. A
 * typed array is always the platform's order and cannot be asked for another; a DataView is
 * always network order unless the call passes a truthy `littleEndian`. So `dv.getUint16(0)` and
 * `new Uint16Array(b)[0]` disagree on every machine this runs on, and both are right -- which is
 * why getting the default backwards is invisible until the bytes meet something outside the
 * program.
 *
 * A byte loop rather than a `memcpy` and a swap: the width is 1, 2, 4 or 8 and the buffer offset
 * is arbitrary, so there is no aligned load to make anyway. Unaligned access is the ordinary case
 * here, not the exception. */
static uint64_t kiln_c99_data_view_gather(const uint8_t *bytes, size_t width, int little_endian) {
    uint64_t bits = 0;
    size_t index;
    for (index = 0; index < width; ++index) {
        bits = (bits << 8) | (uint64_t)bytes[little_endian ? width - 1 - index : index];
    }
    return bits;
}

/* The inverse, and deliberately the same shape: one function per direction, each reading the flag
 * the same way, so the pair cannot disagree about which way the reversal goes. A disagreement
 * there round-trips perfectly through the same view and only shows once the bytes leave. */
static void kiln_c99_data_view_scatter(uint8_t *bytes, size_t width, int little_endian,
                                       uint64_t bits) {
    size_t index;
    for (index = 0; index < width; ++index) {
        bytes[little_endian ? index : width - 1 - index] = (uint8_t)(bits & UINT64_C(0xFF));
        bits >>= 8;
    }
}

/* An unsigned 64-bit value as decimal digits, most significant first, answering the length.
 *
 * `out` needs room for twenty, which is what 18446744073709551615 takes. Written out rather than
 * reached through `snprintf` because `%llu` is the one length modifier this backend cannot assume
 * (ADR 0013 keeps the number formatting in the provider for the same reason). */
static size_t kiln_c99_uint64_digits(uint64_t value, char *out) {
    char reversed[20];
    size_t count = 0;
    size_t index;
    do {
        reversed[count++] = (char)('0' + (int)(value % 10));
        value /= 10;
    } while (value != 0);
    for (index = 0; index < count; ++index) {
        out[index] = reversed[count - 1 - index];
    }
    return count;
}

/* `0xFFFFFFFFFFFFFFFF`, built once.
 *
 * `BigInt.asUintN(64, v)` spelled as the provider's own bitwise AND, which is what
 * `setBigInt64`/`setBigUint64` need: the language *wraps* there rather than refusing, so
 * `setBigUint64(0, -1n)` stores all ones and `setBigInt64(0, 2n ** 200n)` stores the low sixty-
 * four bits. `kiln_bigint_to_i128` alone would have called the second of those out of range.
 *
 * Nothing frees a BigInt handle here, so a shared one is safe to keep -- the same bargain
 * `kiln_c99_bigint_one` makes for `1n`. */
static const void *kiln_c99_bigint_uint64_mask(void) {
    static void *mask = NULL;
    if (mask == NULL) {
        static const unsigned char digits[] = "18446744073709551615";
        if (kiln_bigint_parse(digits, sizeof digits - 1, 10, &mask) != 0) {
            return NULL;
        }
    }
    return mask;
}

/* The order `%TypedArray%.prototype.sort` uses with no comparator: ascending *numeric*.
 *
 * Not `Array.prototype.sort`'s order, which spells its elements out and compares the text --
 * `[10, 9].sort()` is `[10, 9]` and `new Uint8Array([10, 9]).sort()` is `[9, 10]`. A view holds
 * numbers and nothing else, so there is nothing to spell out. NaN sorts last and `-0` before
 * `+0`, which are the two places the ordinary comparison has no answer. */
static int kiln_c99_typed_order(double left, double right) {
    int left_nan = left != left;
    int right_nan = right != right;
    if (left_nan || right_nan) {
        return left_nan - right_nan;
    }
    if (left < right) {
        return -1;
    }
    if (left > right) {
        return 1;
    }
    return kiln_c99_signbit(right) - kiln_c99_signbit(left);
}
