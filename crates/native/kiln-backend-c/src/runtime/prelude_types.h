
struct kiln_gc_heap;
void kiln_c99_gc_mark_symbol_key(struct kiln_gc_heap *heap, const kiln_string *key);

static inline int kiln_c99_key_is_reserved(const kiln_string *key) {
    return key != NULL &&
           (key->flags & (KILN_STRING_SYMBOL_KEY | KILN_STRING_PRIVATE_KEY)) != 0;
}

static inline int kiln_c99_key_is_symbol(const kiln_string *key) {
    return key != NULL && (key->flags & KILN_STRING_SYMBOL_KEY) != 0;
}

static inline uint32_t kiln_c99_key_domain(const kiln_string *key) {
    return key == NULL ? 0 : key->flags & (KILN_STRING_SYMBOL_KEY | KILN_STRING_PRIVATE_KEY);
}

/* The C runtime's fixed byte names predate headed property keys. A user-computed key is always
 * passed as a headed value and keeps the ordinary string domain, even when its units start @@.
 * Only these fixed-name adapters classify the runtime's private slots and well-known symbols. */
static inline uint32_t kiln_c99_fixed_key_flags(const char *name, size_t length) {
    static const char *const symbols[] = {
        "@@iterator", "@@asyncIterator", "@@hasInstance", "@@toStringTag",
        "@@toPrimitive", "@@isConcatSpreadable", "@@match", "@@matchAll",
        "@@replace", "@@search", "@@species", "@@split", "@@unscopables",
        "@@dispose", "@@asyncDispose"
    };
    size_t index;
    if (length < 2 || name[0] != '@' || name[1] != '@') return KILN_STRING_LATIN1;
    for (index = 0; index < sizeof symbols / sizeof *symbols; ++index) {
        if (strlen(symbols[index]) == length && memcmp(symbols[index], name, length) == 0)
            return KILN_STRING_LATIN1 | KILN_STRING_SYMBOL_KEY;
    }
    return KILN_STRING_LATIN1 | KILN_STRING_PRIVATE_KEY;
}

/* A symbol the program made -- `Symbol()` is stored as `@@symbol:<n>`, `Symbol.for(k)` as
 * `@@symbol-for:<k>` -- as opposed to a well-known symbol (`@@iterator`) or one of the runtime's
 * own slots (`@@state`). A built-in can carry one of these only through its property store, so
 * a read that the store did not answer is absent by construction, never a member unbuilt. */
static inline int kiln_c99_key_has_prefix(const kiln_string *key, const char *prefix) {
    size_t at;
    for (at = 0; prefix[at] != '\0'; ++at) {
        if (at >= key->unit_len || kiln_string_unit_at(key, at) != (unsigned char)prefix[at]) {
            return 0;
        }
    }
    return 1;
}

static inline int kiln_c99_key_is_program_symbol(const kiln_string *key) {
    return key != NULL && (kiln_c99_key_has_prefix(key, "@@symbol:") ||
                           kiln_c99_key_has_prefix(key, "@@symbol-for:"));
}

/* =========================================================================
 * ADR 0053: 64-Bit Quiet-NaN Boxed Internal Value Representation
 * ========================================================================= */
typedef uint64_t kiln_val;

#define KILN_QNAN_BASE          UINT64_C(0x7FF8000000000000)
#define KILN_TAG_MASK           UINT64_C(0xFFFF800000000000)
#define KILN_PAYLOAD_MASK       UINT64_C(0x00007FFFFFFFFFFF)

#define KILN_VAL_UNDEFINED      UINT64_C(0xFFF8000000000001)
#define KILN_VAL_NULL           UINT64_C(0xFFF8000000000002)
#define KILN_VAL_FALSE          UINT64_C(0xFFF8000000000004)
#define KILN_VAL_TRUE           UINT64_C(0xFFF8000000000005)
#define KILN_VAL_HOLE           UINT64_C(0xFFF8000000000006)
#define KILN_VAL_UNINITIALIZED  UINT64_C(0xFFF8000000000007)
#define KILN_VAL_RETURN_SIGNAL  UINT64_C(0xFFF8000000000009)

#define KILN_VAL_BUILTIN_PREFIX UINT64_C(0xFFF8000100000000)
#define KILN_VAL_BUILTIN(id)    (KILN_VAL_BUILTIN_PREFIX | ((uint64_t)(id) & UINT64_C(0xFFFFFFFF)))
#define KILN_VAL_IS_BUILTIN(v)  (((uint64_t)(v) & UINT64_C(0xFFFFFFFF00000000)) == KILN_VAL_BUILTIN_PREFIX)
#define KILN_VAL_BUILTIN_ID(v)  ((uint32_t)((uint64_t)(v) & UINT64_C(0xFFFFFFFF)))

#define KILN_VAL_METHOD_PREFIX  UINT64_C(0xFFF8000200000000)
#define KILN_VAL_METHOD(id)     (KILN_VAL_METHOD_PREFIX | ((uint64_t)(id) & UINT64_C(0xFFFFFFFF)))
#define KILN_VAL_IS_METHOD(v)   (((uint64_t)(v) & UINT64_C(0xFFFFFFFF00000000)) == KILN_VAL_METHOD_PREFIX)
#define KILN_VAL_METHOD_ID(v)   ((uint32_t)((uint64_t)(v) & UINT64_C(0xFFFFFFFF)))

#define KILN_TAG_STRING_PREFIX   UINT64_C(0xFFF9000000000000)
#define KILN_TAG_OBJECT_PREFIX   UINT64_C(0xFFFA000000000000)
#define KILN_TAG_CLOSURE_PREFIX  UINT64_C(0xFFFB000000000000)
#define KILN_TAG_BUFFER_PREFIX   UINT64_C(0xFFFC000000000000)
#define KILN_TAG_SYMBOL_PREFIX   UINT64_C(0xFFFD000000000000)
#define KILN_TAG_BIGINT_PREFIX   UINT64_C(0xFFFE000000000000)
#define KILN_TAG_TEMPORAL_PREFIX UINT64_C(0xFFFE800000000000)
#define KILN_TAG_CELL_PREFIX     UINT64_C(0xFFFF000000000000)
/* Bit 47 is outside the pointer payload. Keep shared facades distinct from cells and from
 * object/array prefixes so every generated fast path misses them without inspecting a payload. */
#define KILN_TAG_SHARED_PREFIX   UINT64_C(0xFFFF800000000000)

/* What a typed entry returns instead of its number when it failed -- a throw, a refusal, or the
 * call-depth limit. No arithmetic produces this NaN payload and no value carries it, so the
 * caller tells failure from any result by one comparison; the status and the thrown value wait
 * in `kiln_c99_typed_status` and `kiln_c99_typed_thrown`. */
#define KILN_VAL_TYPED_FAIL     UINT64_C(0xFFF8000000000008)
static inline double kiln_c99_typed_fail_double(void) {
    union { uint64_t u; double d; } bits = { .u = KILN_VAL_TYPED_FAIL };
    return bits.d;
}
static inline int kiln_c99_typed_failed(double d) {
    /* A number that is a number is not the failure value; the bit comparison, which needs the
     * constant in a register, runs only for a NaN. */
    if (KILN_LIKELY(d == d)) {
        return 0;
    }
    union { double d; uint64_t u; } bits = { .d = d };
    return bits.u == KILN_VAL_TYPED_FAIL;
}

#define KILN_VAL_IS_NUMBER(v)    ((uint64_t)(v) < UINT64_C(0xFFF8000000000000))
#define KILN_VAL_IS_BOOLEAN(v)   (((uint64_t)(v) & UINT64_C(0xFFFFFFFFFFFFFFFE)) == KILN_VAL_FALSE)
#define KILN_VAL_IS_NULL(v)      ((v) == KILN_VAL_NULL)
#define KILN_VAL_IS_UNDEFINED(v) ((v) == KILN_VAL_UNDEFINED)
#define KILN_VAL_IS_NULLISH(v)   ((v) == KILN_VAL_UNDEFINED || (v) == KILN_VAL_NULL)
/* undefined, null, true or false: the values whose identity is their bits, so `===` against one
 * of them is a word compare whatever the other operand is. */
#define KILN_VAL_IS_IMMEDIATE(v) (KILN_VAL_IS_NULLISH(v) || KILN_VAL_IS_BOOLEAN(v))
/* The equality operators (codes 7/8 strict, 15/16 loose) when an operand is an immediate: `===`
 * compares words, and `==` against undefined or null holds exactly for undefined and null.
 * `x === undefined`, `opts.signal != null`: the common tests of an optional member, which went
 * through the boundary conversion and the full operator before. HOLE if the pair is not one. */
#define KILN_VAL_EQUALITY_IMMEDIATE(op, left, right) \
    (((op) == 7 || (op) == 8) && (KILN_VAL_IS_IMMEDIATE(left) || KILN_VAL_IS_IMMEDIATE(right)) \
         ? ((((left) == (right)) == ((op) == 7)) ? KILN_VAL_TRUE : KILN_VAL_FALSE) \
     : ((op) == 15 || (op) == 16) && (KILN_VAL_IS_NULLISH(left) || KILN_VAL_IS_NULLISH(right)) \
         ? (((KILN_VAL_IS_NULLISH(left) && KILN_VAL_IS_NULLISH(right)) == ((op) == 15)) ? KILN_VAL_TRUE \
                                                                                        : KILN_VAL_FALSE) \
         : KILN_VAL_HOLE)
#define KILN_VAL_IS_STRING(v)    (((v) & KILN_TAG_MASK) == KILN_TAG_STRING_PREFIX)
#define KILN_VAL_IS_OBJECT(v)    (((v) & KILN_TAG_MASK) == KILN_TAG_OBJECT_PREFIX)
#define KILN_VAL_IS_SHARED(v)    (((v) & KILN_TAG_MASK) == KILN_TAG_SHARED_PREFIX)
#define KILN_VAL_IS_CLOSURE(v)   (((v) & KILN_TAG_MASK) == KILN_TAG_CLOSURE_PREFIX)
#define KILN_VAL_IS_SYMBOL(v)    (((v) & KILN_TAG_MASK) == KILN_TAG_SYMBOL_PREFIX)
#define KILN_VAL_PTR(v)          ((void *)((uint64_t)(v) & KILN_PAYLOAD_MASK))
#define KILN_VAL_TAG_PTR(pfx, p) ((kiln_val)((pfx) | ((uint64_t)(uintptr_t)(p) & KILN_PAYLOAD_MASK)))
#define KILN_VAL_IS_DOUBLE(v)    KILN_VAL_IS_NUMBER(v)

/* KILN_RUNTIME_ONLY_ASCII_STRINGS_BEGIN */
static const uint8_t kiln_c99_empty_chars[] = { 0 };
static const kiln_string kiln_c99_empty_string = { 0, KILN_STRING_LATIN1, kiln_c99_empty_chars };

static const char kiln_c99_single_char_bytes[256][2] = {
#define ROW4(a)   { (char)(a), 0 }, { (char)((a)+1), 0 }, { (char)((a)+2), 0 }, { (char)((a)+3), 0 }
#define ROW16(a)  ROW4(a), ROW4((a)+4), ROW4((a)+8), ROW4((a)+12)
#define ROW64(a)  ROW16(a), ROW16((a)+16), ROW16((a)+32), ROW16((a)+48)
    ROW64(0), ROW64(64), ROW64(128), ROW64(192)
#undef ROW64
#undef ROW16
#undef ROW4
};
static const kiln_string kiln_c99_ascii_strings[256] = {
#define S4(a)   { 1, KILN_STRING_LATIN1, kiln_c99_single_char_bytes[a] }, { 1, KILN_STRING_LATIN1, kiln_c99_single_char_bytes[(a)+1] }, { 1, KILN_STRING_LATIN1, kiln_c99_single_char_bytes[(a)+2] }, { 1, KILN_STRING_LATIN1, kiln_c99_single_char_bytes[(a)+3] }
#define S16(a)  S4(a), S4((a)+4), S4((a)+8), S4((a)+12)
#define S64(a)  S16(a), S16((a)+16), S16((a)+32), S16((a)+48)
    S64(0), S64(64), S64(128), S64(192)
#undef S64
#undef S16
#undef S4
};
/* KILN_RUNTIME_ONLY_ASCII_STRINGS_END */

static inline kiln_val kiln_c99_val_single_char(unsigned char ch) {
    return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_ascii_strings[ch]);
}

/* A short copy without the call. A `memcpy` of seventeen bytes is mostly the cost of getting to
 * `memcpy`, through the dynamic linker's stub and its own length dispatch, and a `split` makes
 * one per part. Anything long enough for that to stop being true goes to the real one. */
/* Inlined whatever the optimiser thinks: the point of the thing is to not be a call. */
#if defined(__GNUC__) || defined(__clang__)
#define KILN_ALWAYS_INLINE __attribute__((always_inline))
#else
#define KILN_ALWAYS_INLINE
#endif

static inline void kiln_c99_copy_short(char *dst, const char *src, size_t len) {
    size_t at = 0;
    if (len > 64) {
        memcpy(dst, src, len);
        return;
    }
    while (at + 8 <= len) {
        uint64_t word;
        memcpy(&word, src + at, sizeof word);
        memcpy(dst + at, &word, sizeof word);
        at += 8;
    }
    while (at < len) {
        dst[at] = src[at];
        at += 1;
    }
}

/* The next `sep` at or after `from`, eight bytes at a time.
 *
 * `memchr` is the better answer on a long haystack and this is not trying to beat it there. A
 * split's spans are one field wide -- seventeen bytes on the log line this was measured on -- and
 * over a span that short the call, the dynamic linker's stub in front of it and the setup inside
 * it all cost more than the scan does. Nine of those per line was six percent of the benchmark.
 *
 * The word test is the classic one: exclusive-or against the byte repeated eight times leaves a
 * zero byte exactly where the byte matched, and `(w - 0x01..01) & ~w & 0x80..80` is non-zero
 * exactly when some byte of `w` is zero. Which byte it was is a count of trailing zeros where
 * the byte order makes that the earliest byte, and a look at the eight otherwise. */
/* Whether the lowest-addressed byte of a word is its least significant one, which is what makes
 * a count of trailing zeros name the earliest matching byte below. Everything this repository
 * targets says yes; the other branch is there so the answer never depends on it. */
#if defined(__GNUC__) && defined(__BYTE_ORDER__) && defined(__ORDER_LITTLE_ENDIAN__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define KILN_C99_LOW_BYTE_FIRST 1
#else
#define KILN_C99_LOW_BYTE_FIRST 0
#endif

static inline const char *kiln_c99_find_byte(const char *from, const char *end, char sep) {
    const uint64_t ones = UINT64_C(0x0101010101010101);
    uint64_t broadcast = ones * (uint64_t)(unsigned char)sep;
    while ((size_t)(end - from) >= 8) {
        uint64_t word;
        uint64_t zero;
        memcpy(&word, from, sizeof word);
        word ^= broadcast;
        zero = (word - ones) & ~word & UINT64_C(0x8080808080808080);
        if (zero != 0) {
#if KILN_C99_LOW_BYTE_FIRST
            /* The lowest set flag is the earliest byte, so trailing zeros name it directly. */
            return from + ((size_t)__builtin_ctzll(zero) >> 3);
#else
            size_t at;
            for (at = 0; at < 8; ++at) {
                if (from[at] == sep) return from + at;
            }
#endif
        }
        from += 8;
    }
    while (from < end) {
        if (*from == sep) return from;
        from += 1;
    }
    return NULL;
}


/* Whether two runs of bytes are equal, without the call. The needles here are a word or two --
 * "amount=" is seven bytes -- and over that length `memcmp` is mostly the cost of reaching
 * `memcmp`, through the dynamic linker's stub and its own length dispatch. */
static inline int kiln_c99_same_bytes(const char *left, const char *right, size_t len) {
    size_t at = 0;
    while (at + 8 <= len) {
        uint64_t a;
        uint64_t b;
        memcpy(&a, left + at, sizeof a);
        memcpy(&b, right + at, sizeof b);
        if (a != b) return 0;
        at += 8;
    }
    while (at < len) {
        if (left[at] != right[at]) return 0;
        at += 1;
    }
    return 1;
}

/* Find `needle` in `haystack`.
 *
 * The first byte is found by the word scan above rather than by `memchr`, and the rest is
 * compared inline; between them a search makes no library call at all. The last byte is checked
 * before the middle because it is the cheapest way to reject a candidate: searching a joined log
 * line for "amount=" passes about ten "a"s on the way, and every one of them used to be a full
 * `memcmp` call. */
static inline const char *kiln_c99_memmem(const char *haystack, size_t haystack_len,
                                          const char *needle, size_t needle_len) {
    const char *end;
    const char *limit;
    const char *p;
    if (needle_len == 0) return haystack;
    if (needle_len > haystack_len) return NULL;
    end = haystack + haystack_len;
    if (needle_len == 1) return kiln_c99_find_byte(haystack, end, needle[0]);
    limit = end - needle_len;
    p = haystack;
    while (p <= limit) {
        p = kiln_c99_find_byte(p, limit + 1, needle[0]);
        if (p == NULL) return NULL;
        if (p[needle_len - 1] == needle[needle_len - 1] &&
            kiln_c99_same_bytes(p + 1, needle + 1, needle_len - 2)) {
            return p;
        }
        p += 1;
    }
    return NULL;
}

#ifdef KILN_SEPARATE_PRELUDE
extern kiln_val kiln_c99_val_from_text(const char *bytes, size_t len);
#else
static kiln_val kiln_c99_val_from_text(const char *bytes, size_t len);
static kiln_value kiln_val_to_boundary(kiln_val v);
static kiln_value kiln_val_binding_to_boundary(kiln_val v);
#endif

static inline kiln_val kiln_val_from_boundary(kiln_value v) {
    if (v.tag == KILN_VALUE_NUMBER) {
        /* Canonicalize any quiet-NaN to prevent collision with tagged pointer prefixes */
        if (KILN_UNLIKELY((v.bits & UINT64_C(0x7FF8000000000000)) == UINT64_C(0x7FF8000000000000))) {
            return KILN_QNAN_BASE;
        }
        return (kiln_val)v.bits;
    }
    switch (v.tag) {
        case KILN_VALUE_UNDEFINED:     return KILN_VAL_UNDEFINED;
        case KILN_VALUE_NULL:          return KILN_VAL_NULL;
        case KILN_VALUE_BOOLEAN:       return v.bits ? KILN_VAL_TRUE : KILN_VAL_FALSE;
        case KILN_VALUE_STRING: {
            if (v.ptr == NULL) {
                return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
            }
            /* Preserve even a zero-length string's header: an actor candidate may own that
             * block. Boundary strings carry canonical header pointers, never byte pointers. */
            return KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, v.ptr);
        }
        case KILN_VALUE_FUNCTION:      return KILN_VAL_TAG_PTR(KILN_TAG_CLOSURE_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_OBJECT:        return KILN_VAL_TAG_PTR(KILN_TAG_OBJECT_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_ARRAY:         return KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_TYPED_ARRAY:   return KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_ARRAY_BUFFER:  return KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_DATA_VIEW:     return KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_SYMBOL:        return KILN_VAL_TAG_PTR(KILN_TAG_SYMBOL_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_BUILTIN:       return KILN_VAL_BUILTIN(v.bits);
        case KILN_VALUE_METHOD:        return KILN_VAL_METHOD(v.bits);
        case KILN_VALUE_BIGINT:        return KILN_VAL_TAG_PTR(KILN_TAG_BIGINT_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_TEMPORAL:      return KILN_VAL_TAG_PTR(KILN_TAG_TEMPORAL_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_CELL:          return KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_SHARED:        return KILN_VAL_TAG_PTR(KILN_TAG_SHARED_PREFIX, (const void *)(uintptr_t)v.bits);
        case KILN_VALUE_HOLE:          return KILN_VAL_HOLE;
        case KILN_VALUE_UNINITIALIZED: return KILN_VAL_UNINITIALIZED;
        case KILN_VALUE_RETURN_SIGNAL: return KILN_VAL_RETURN_SIGNAL;
        default:                       return KILN_VAL_TAG_PTR(KILN_TAG_OBJECT_PREFIX, (const void *)(uintptr_t)v.bits);
    }
}

static inline kiln_value kiln_c99_return_signal(void) {
    kiln_value v = { .tag = KILN_VALUE_RETURN_SIGNAL, .flags = 0, .len = 0, .bits = 0 };
    return v;
}

static inline kiln_val kiln_c99_val_from_double(double d) {
    if (KILN_UNLIKELY(isnan(d))) {
        return KILN_QNAN_BASE;
    }
    union { double d; uint64_t u; } res = { .d = d };
    return res.u;
}

int32_t kiln_bigint_is_zero(const void *handle);
/* `parallelMap`'s fork-join (ADR 0058, application 2), linked from the provider archive. `chunk`
 * is an emitted static worker over `[start, end)` of the two arrays; the provider splits `[0,
 * count)` across its private pool, or runs the whole range inline below its floor. The arrays
 * cross as opaque pointers: the provider never reads an element, so nothing is unboxed or copied,
 * and a worker runs on a thread that owns no realm -- which is why the backend admits only a
 * body that reads its double parameters and returns a double (`parallel.rs`). */
typedef void (*kiln_parallel_chunk_fn)(const void * restrict in, void * restrict out, size_t start, size_t end,
                                       void *ctx);
int32_t kiln_parallel_map(const void *in, size_t count, void *out, kiln_parallel_chunk_fn chunk,
                          void *ctx);
/* How many threads `kiln_parallel_map` splits a call across: the pool's size, or 1 when there is
 * no pool (`KILN_PARALLELISM=1`, one core, a `KILN_WORKERS` share of one). A site asks before it
 * takes its pooled lowering: one thread cannot win anything, so the sequential method's own loop
 * runs instead (parallel issue 22); a tier-3 site also sizes its arena table by it. */
size_t kiln_parallel_threads(void);
/* `parallelSort` (ADR 0069): a stable in-place sort of `count` elements -- a `kiln_typed_kind`
 * (0..8), or 100 for an array's own numeric elements -- by 0 (the typed-array default order), 1
 * (`(a, b) => a - b`) or 2 (`(a, b) => b - a`), on the same pool. 0 sorted; 1 a comparator order
 * met a NaN and nothing was written; -1 an argument it does not take. The emitter asks only where
 * the order is proven and the elements are numbers, so the answer is `sort`'s. */
int32_t kiln_parallel_sort(void *data, size_t count, int32_t kind, int32_t order);

static inline int kiln_c99_val_truthy(kiln_val v) {
    if (v == KILN_VAL_FALSE || v == KILN_VAL_UNDEFINED || v == KILN_VAL_NULL) return 0;
    if (v == KILN_VAL_TRUE) return 1;
    if (KILN_VAL_IS_NUMBER(v)) {
        double d;
        memcpy(&d, &v, sizeof(d));
        return d != 0.0 && !isnan(d);
    }
    if ((v & UINT64_C(0xFFFF800000000000)) == KILN_TAG_BIGINT_PREFIX) {
        return !kiln_bigint_is_zero(KILN_VAL_PTR(v));
    }
    if ((v & KILN_TAG_MASK) == KILN_TAG_STRING_PREFIX) {
        const kiln_string *s = (const kiln_string *)KILN_VAL_PTR(v);
        return s != NULL && s->unit_len > 0;
    }
    return 1;
}

/* Host-facing C routines that explicitly require UTF-8 may borrow bytes only from a Latin1
 * string. UTF-16 callers must select an encoding operation instead of treating the header as a
 * byte pointer. */
static inline const char *kiln_c99_string_ascii_bytes(kiln_value value, size_t *length) {
    const kiln_string *string;
    if (length != NULL) *length = 0;
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return NULL;
    string = (const kiln_string *)value.ptr;
    if (kiln_string_is_utf16(string)) return NULL;
    if (length != NULL) *length = string->unit_len;
    return (const char *)kiln_string_latin1(string);
}

/* Fixed host predicates still receive a JavaScript string value, so compare its code units
 * through the canonical header.  The shared helper is also used by generated graph code; this
 * wrapper keeps the boundary-value tag check beside the other host adapters. */
static inline int kiln_c99_string_equals_ascii(kiln_value value, const char *ascii,
                                               size_t length) {
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0;
    return kiln_string_equals_ascii((const kiln_string *)value.ptr, ascii, length);
}

static inline int kiln_c99_string_starts_ascii(kiln_value value, const char *ascii,
                                               size_t length) {
    if (value.tag != KILN_VALUE_STRING || value.ptr == NULL) return 0;
    return kiln_string_starts_ascii((const kiln_string *)value.ptr, ascii, length);
}

/* =========================================================================
 * Step 2.1: Speculation and Versioning Runtime Support
 * ========================================================================= */
static int kiln_c99_speculate_mode = -2;

static inline int kiln_c99_speculate_exit_active(void) {
    /* One word, the same from every thread that reads the environment. */
    int mode = __atomic_load_n(&kiln_c99_speculate_mode, __ATOMIC_RELAXED);
    if (mode == -2) {
        const char *s = getenv("KILN_SPECULATE");
        if (s && strcmp(s, "exit") == 0) {
            mode = 1;
        } else if (s && strcmp(s, "0") == 0) {
            mode = -1;
        } else {
            mode = 0;
        }
        __atomic_store_n(&kiln_c99_speculate_mode, mode, __ATOMIC_RELAXED);
    }
    return mode;
}

/* The guard with the mode already read: a versioned body reads it once at entry, and a loop
 * head then tests a register instead of re-reading the flag every iteration. */
/* Build a derived constant -- a sorted index over a constant table -- exactly once per process
 * copy of the prelude, whichever realm's thread asks first. `state` is 0 unbuilt, 1 building, 2
 * built; a thread that finds another building waits for it, which happens at most once, for
 * microseconds. Building in place twice at once would let one thread search the table while the
 * other is still sorting it. */
static __attribute__((noinline, unused)) void kiln_c99_once_slow(int *state, void (*build)(void)) {
    int unbuilt = 0;
    if (__atomic_compare_exchange_n(state, &unbuilt, 1, 0, __ATOMIC_ACQUIRE, __ATOMIC_ACQUIRE)) {
        build();
        __atomic_store_n(state, 2, __ATOMIC_RELEASE);
        return;
    }
    while (__atomic_load_n(state, __ATOMIC_ACQUIRE) != 2) {
    }
}

static KILN_ALWAYS_INLINE inline void kiln_c99_once(int *state, void (*build)(void)) {
    if (KILN_UNLIKELY(__atomic_load_n(state, __ATOMIC_ACQUIRE) != 2)) kiln_c99_once_slow(state, build);
}

static inline int kiln_c99_guard_should_exit_in(int mode, uint32_t *counter) {
    if (KILN_UNLIKELY(mode == -1)) {
        return 1;
    }
    if (KILN_UNLIKELY(mode == 1)) {
        if (__atomic_fetch_add(counter, 1u, __ATOMIC_RELAXED) == 0) {
            return 1;
        }
        /* Per thread: the exits it draws only have to be arbitrary, not shared. */
        static _Thread_local uint64_t kiln_c99_speculate_rng = 0x853c49e6748fea9bULL;
        kiln_c99_speculate_rng ^= kiln_c99_speculate_rng >> 12;
        kiln_c99_speculate_rng ^= kiln_c99_speculate_rng << 25;
        kiln_c99_speculate_rng ^= kiln_c99_speculate_rng >> 27;
        return (kiln_c99_speculate_rng & 1) != 0;
    }
    return 0;
}

static inline int kiln_c99_guard_should_exit(uint32_t *counter) {
    return kiln_c99_guard_should_exit_in(kiln_c99_speculate_exit_active(), counter);
}

/* IEEE 754 binary16 conversions shared by emitted reads and the generic view accessors. */
static inline double kiln_c99_half_to_double(uint16_t bits) {
    unsigned sign = bits >> 15;
    unsigned exponent = (bits >> 10) & 31u;
    unsigned fraction = bits & 1023u;
    double value;
    if (exponent == 31u) {
        if (fraction != 0) return NAN;
        return sign ? -INFINITY : INFINITY;
    }
    if (exponent == 0) value = ldexp((double)fraction, -24);
    else value = ldexp((double)(1024u + fraction), (int)exponent - 25);
    return sign ? -value : value;
}

static inline uint16_t kiln_c99_double_to_half(double value) {
    uint64_t bits, significand, remainder, halfway;
    unsigned sign, exponent_bits;
    int exponent, shift;
    uint64_t rounded;
    memcpy(&bits, &value, sizeof bits);
    sign = (unsigned)(bits >> 63);
    exponent_bits = (unsigned)((bits >> 52) & 0x7ffu);
    significand = bits & UINT64_C(0x000fffffffffffff);
    if (exponent_bits == 0x7ffu) {
        if (significand == 0) return (uint16_t)((sign << 15) | 0x7c00u);
        return (uint16_t)((sign << 15) | 0x7e00u);
    }
    if (exponent_bits == 0) return (uint16_t)(sign << 15); /* All binary64 subnormals underflow. */
    exponent = (int)exponent_bits - 1023;
    significand |= UINT64_C(1) << 52;
    if (exponent > 15) return (uint16_t)((sign << 15) | 0x7c00u);
    if (exponent >= -14) {
        rounded = significand >> 42;
        remainder = significand & ((UINT64_C(1) << 42) - 1);
        halfway = UINT64_C(1) << 41;
        if (remainder > halfway || (remainder == halfway && (rounded & 1u))) ++rounded;
        if (rounded == 2048u) {
            rounded = 1024u;
            if (++exponent > 15) return (uint16_t)((sign << 15) | 0x7c00u);
        }
        return (uint16_t)((sign << 15) | ((unsigned)(exponent + 15) << 10) |
                          ((unsigned)rounded & 1023u));
    }
    shift = 28 - exponent;
    if (shift > 63) return (uint16_t)(sign << 15);
    rounded = significand >> shift;
    remainder = significand & ((UINT64_C(1) << shift) - 1);
    halfway = UINT64_C(1) << (shift - 1);
    if (remainder > halfway || (remainder == halfway && (rounded & 1u))) ++rounded;
    return (uint16_t)((sign << 15) | (unsigned)rounded);
}

typedef enum kiln_typed_kind {
    KILN_TYPED_INT8,
    KILN_TYPED_UINT8,
    KILN_TYPED_UINT8_CLAMPED,
    KILN_TYPED_INT16,
    KILN_TYPED_UINT16,
    KILN_TYPED_INT32,
    KILN_TYPED_UINT32,
    KILN_TYPED_FLOAT16,
    KILN_TYPED_FLOAT32,
    KILN_TYPED_FLOAT64,
    KILN_TYPED_BIG_INT64,
    KILN_TYPED_BIG_UINT64
} kiln_typed_kind;

typedef struct kiln_typed_buffer {
    uint8_t *bytes;
    size_t byte_length;
    /* A borrowed FFI view points into a loaded native library's memory. Ordinary ArrayBuffers
     * own their allocation; borrowed ones must never feed that pointer to free(). */
    int owns_bytes;
    /* Bytes adopted from a worker message, counted as this realm's moved external memory.
     * Cleared when ownership leaves the realm or passes to another buffer here. */
    int moved_external;
    /* Set once, by `__kilnArrayBufferDetach`, and never cleared: the bytes went to another
     * buffer, `bytes` points at a static empty block and `byte_length` is zero. */
    int detached;
    /* A `SharedArrayBuffer`'s store (`kiln_shared_create`), shared with every realm that was
     * sent the buffer; NULL for an `ArrayBuffer`. The buffer holds one strong reference, which
     * its finalizer releases; `bytes` points into the store and is never freed here. */
    void *shared;
    /* A `SharedArrayBuffer`'s ceiling (`maxByteLength`), and whether it can grow to it. */
    size_t shared_capacity;
    int shared_growable;
} kiln_typed_buffer;

/* The scalar-only dynamic-call ABI. Its layout is pinned by the Rust provider's repr(C) type. */
typedef struct kiln_ffi_value {
    uint32_t kind;
    uint32_t reserved;
    uint64_t bits;
} kiln_ffi_value;
typedef int32_t (*kiln_ffi_dispatch_fn)(void *context, const kiln_ffi_value *arguments,
                                        uint64_t argument_count, kiln_ffi_value *out_result);

typedef struct kiln_typed_array {
    kiln_typed_buffer *buffer;
    /* Named own properties live beside the indexed element storage. Most views never allocate
     * this store; it is created only when a program writes a non-index key. */
    kiln_value properties;
    size_t byte_offset;
    size_t length;
    kiln_typed_kind kind;
    /* Node's Buffer is a Uint8Array with a different prototype surface, not another byte
     * container. Keep that distinction on the view so slices can share storage and every
     * numeric typed-array operation continues to use the battle-tested path above it. */
    int is_buffer;
} kiln_typed_array;

/* Element `index` of a view whose kind is a Number kind (not `KILN_TYPED_BIG_INT64` or
 * `KILN_TYPED_BIG_UINT64`), for the emitted element reads that have already checked `index <
 * view->length` and that the buffer is not detached. A view never outgrows its buffer (no buffer
 * here is resizable), so what is left is the load. `kiln_c99_typed_get` is the same read for a
 * caller that has checked nothing; it lives in the prelude object, so a loop over a view paid a
 * call into another object and a second bounds proof per element -- 6 ns a read, against node's
 * half nanosecond. Little-endian by assembly, as there, which the compiler folds to one load. */
static inline double kiln_typed_load(const kiln_typed_array *view, size_t index) {
    const uint8_t *bytes = view->buffer->bytes + view->byte_offset;
    uint32_t bits32;
    uint64_t bits64;
    int32_t signed32;
    float float32;
    double float64;
    switch (view->kind) {
    case KILN_TYPED_UINT8:
    case KILN_TYPED_UINT8_CLAMPED:
    default: return (double)bytes[index];
    case KILN_TYPED_INT8: return (double)(int8_t)bytes[index];
    case KILN_TYPED_INT16:
        bytes += index * 2;
        return (double)(int16_t)((uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8));
    case KILN_TYPED_FLOAT16:
        bytes += index * 2;
        return kiln_c99_half_to_double((uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8));
    case KILN_TYPED_UINT16:
        bytes += index * 2;
        return (double)(uint16_t)((uint16_t)bytes[0] | ((uint16_t)bytes[1] << 8));
    case KILN_TYPED_INT32:
    case KILN_TYPED_UINT32:
    case KILN_TYPED_FLOAT32:
        bytes += index * 4;
        bits32 = (uint32_t)bytes[0] | ((uint32_t)bytes[1] << 8) | ((uint32_t)bytes[2] << 16) |
                 ((uint32_t)bytes[3] << 24);
        if (view->kind == KILN_TYPED_UINT32) return (double)bits32;
        if (view->kind == KILN_TYPED_INT32) {
            memcpy(&signed32, &bits32, sizeof signed32);
            return (double)signed32;
        }
        memcpy(&float32, &bits32, sizeof float32);
        return (double)float32;
    case KILN_TYPED_FLOAT64:
        bytes += index * 8;
        bits64 = (uint64_t)bytes[0] | ((uint64_t)bytes[1] << 8) | ((uint64_t)bytes[2] << 16) |
                 ((uint64_t)bytes[3] << 24) | ((uint64_t)bytes[4] << 32) |
                 ((uint64_t)bytes[5] << 40) | ((uint64_t)bytes[6] << 48) |
                 ((uint64_t)bytes[7] << 56);
        memcpy(&float64, &bits64, sizeof float64);
        return float64;
    }
}

/* A `DataView`: a window onto a byte buffer that has no element kind of its own.
 *
 * The buffer is *borrowed*, exactly as a typed array borrows one -- `new DataView(b).buffer === b`
 * and a write through this view is visible through every other view of the same bytes, which is
 * the whole reason a program reaches for one. Copying here would answer plausibly and wrongly.
 *
 * `byte_length` is bytes and not elements, because there is no element to count: the width comes
 * from which accessor was called, and so does the byte order. Resolved once at construction --
 * this backend has no resizable buffer, so there is no length to track. */
typedef struct kiln_data_view {
    kiln_typed_buffer *buffer;
    size_t byte_offset;
    size_t byte_length;
} kiln_data_view;


/* A nonzero callback return is propagated as KILN_C99_HOST_ERROR.  The callback owns any
 * interpretation of operation numbers and may replace *result. */
typedef int (*kiln_host_call_fn)(void *context, uint16_t operation, uint16_t argc,
                                 const kiln_value *arguments, kiln_value *result);
typedef struct kiln_c99_unit kiln_c99_unit;
typedef kiln_val *(*kiln_host_unit_values_fn)(void *module_context,
                                              const kiln_c99_unit *unit);
typedef struct kiln_host {
    kiln_host_call_fn call;
    void *context;
    /* Non-null only when this host explicitly carries generated module-instance provenance. */
    void *module_context;
    /* GC-managed owner for module-context storage, null for ordinary embedding hosts. */
    void *module_owner;
    /* Optional per-module-instance state resolver; absent on ordinary hosts. */
    kiln_host_unit_values_fn module_unit_values;
    uint64_t module_instance_id;
} kiln_host;

typedef void (*kiln_drop_opaque_fn)(void *pointer);
extern int kiln_realm_callable_store(uint32_t tag, uint64_t code, void *candidate,
                                     kiln_drop_opaque_fn drop_candidate, void **out_store);
extern int kiln_realm_value_slot(uint32_t key, void **out_slot);
#define KILN_C99_REALM_SLOT_PROCESS_EXIT_CODE UINT32_C(16)
#define KILN_C99_REALM_SLOT_ERROR_STACK_GETTER UINT32_C(17)
#define KILN_C99_REALM_SLOT_ERROR_STACK_SETTER UINT32_C(18)
#define KILN_C99_REALM_SLOT_ERROR_BRAND_TOKEN UINT32_C(19)
extern int kiln_realm_entropy_word(uint64_t seed, uint64_t *out_word);
/* The collector's seam with the runtime archive (ADR 0041). The prelude owns the heap and the
 * tracer; the archive owns the turn boundary and the roots it holds. */
typedef void (*kiln_gc_roots_fn)(void *heap, void *context, int phase);
typedef struct kiln_gc_hooks {
    uint32_t version;
    uint32_t size;
    void (*collect)(void *heap, int force);
    void (*teardown_begin)(void *heap);
    void (*teardown_end)(void *heap);
    void (*mark_value)(void *heap, const void *value);
    void (*mark_block)(void *heap, void *pointer);
    /* Offer the prelude the turn boundary the collector already sweeps at, so a pending SIGUSR1
     * dump runs where ordinary code may run. Returns KILN_C99_INTROSPECT_* as a bitmask. */
    int (*introspect)(void *heap);
} kiln_gc_hooks;
extern void *kiln_realm_gc_current_heap(void);
extern const uint64_t *kiln_realm_generation_slot(void);
/* Process-wide storage the prelude keeps its shared keys and shapes in (ADR 0071): at least
 * `size` zero-initialised bytes, the same address for every unit and every thread for the life of
 * the process, or NULL when `size` exceeds what the provider reserved. The prelude is emitted per
 * translation unit with internal linkage, so a static of its own would be one table per unit. */
extern void *kiln_realm_process_shared(uint64_t size);
/* Opaque actor graph capsules live in a provider-owned registry while both realms are claimed.
 * Rust stores only pointer bits and exact numeric identities; C owns and dereferences the
 * capsule before unlink on the sender lane or after detach on the receiver lane. */
extern int kiln_realm_graph_capsule_stage(uint64_t reservation, uint64_t receiver,
                                          uint64_t request, void *capsule);
extern int kiln_realm_graph_capsule_peek(uint64_t reservation, uint64_t sender,
                                         uint64_t request, void **out);
extern int kiln_realm_graph_capsule_detached(uint64_t reservation, uint64_t request);
extern int kiln_realm_graph_capsule_cancel(uint64_t reservation, uint64_t request);
extern int kiln_realm_graph_capsule_take(uint64_t reservation, uint64_t sender,
                                         uint64_t request, void **out);
/* Quiescent-state reclamation of that shared state (ADR 0071, realms issue 04 step 4; see
 * `KILN_C99_SLOT_*` below). `retire_tag` is asked by a writer that has just unpublished some: the
 * tag to keep beside it, or 0 when no other thread is running a realm and it may be rewritten at
 * once. `retired_safe` answers whether every thread that was running a realm at that tag has since
 * passed a turn boundary. `retire` gives up a `malloc`ed record, freed once that is so. */
extern uint64_t kiln_realm_retire_tag(void);
extern int kiln_realm_retired_safe(uint64_t tag);
extern int kiln_realm_retire(void *record);
typedef struct kiln_shared_info {
    uint32_t array;
    uint32_t null_prototype;
    uint32_t length;
    uint64_t field_count;
} kiln_shared_info;
typedef struct kiln_shared_field_info {
    uint32_t kind;
    uint32_t enumerable;
    uint64_t key_length;
    uint64_t value_length;
    uint64_t bits;
    uint32_t generation;
    uint32_t node;
} kiln_shared_field_info;
/* Mirrors kiln-shared-graph's repr(C) sealed records. The provider ABI version changes with
 * this layout; every read below checks its byte range before copying a record or text. */
typedef struct kiln_shared_node_record {
    uint32_t kind;
    uint32_t null_prototype;
    uint64_t length;
    uint64_t fields;
    uint64_t field_count;
    uint64_t index;
    uint64_t index_capacity;
} kiln_shared_node_record;
typedef struct kiln_shared_field_record {
    uint64_t key_offset;
    uint64_t key_length;
    uint32_t kind;
    uint32_t enumerable;
    uint64_t payload;
    uint64_t value_length;
} kiln_shared_field_record;
typedef struct kiln_shared_arena_view {
    const uint8_t *base;
    uint64_t length;
    uint32_t node_count;
    uint32_t layout_version;
} kiln_shared_arena_view;
_Static_assert(sizeof(kiln_shared_node_record) == 48, "sealed node layout");
_Static_assert(offsetof(kiln_shared_node_record, index_capacity) == 40, "sealed node offset");
_Static_assert(sizeof(kiln_shared_field_record) == 40, "sealed field layout");
_Static_assert(offsetof(kiln_shared_field_record, value_length) == 32, "sealed field offset");
_Static_assert(sizeof(kiln_shared_arena_view) == 24, "sealed view ABI");
_Static_assert(offsetof(kiln_shared_arena_view, layout_version) == 20, "sealed view offset");
#define KILN_SHARED_ARENA_LAYOUT_VERSION 1u
#define KILN_SHARED_ARENA_RECORD 1u
#define KILN_SHARED_ARENA_ARRAY 2u
#define KILN_SHARED_ARENA_EXTERNAL 7u
#define KILN_SHARED_FIELD_UNDEFINED 0u
#define KILN_SHARED_FIELD_NULL 1u
#define KILN_SHARED_FIELD_BOOLEAN 2u
#define KILN_SHARED_FIELD_NUMBER 3u
#define KILN_SHARED_FIELD_BIGINT 4u
#define KILN_SHARED_FIELD_STRING 5u
#define KILN_SHARED_FIELD_NODE 6u
extern uint64_t kiln_shared_graph_retain(uint32_t graph, uint32_t generation, uint32_t node);
extern int kiln_shared_graph_release(uint64_t lease);
extern int kiln_shared_graph_view(uint64_t lease, kiln_shared_arena_view *out);
extern int kiln_shared_graph_info(uint64_t lease, kiln_shared_info *out);
extern int kiln_shared_graph_find_field(uint64_t lease, const uint16_t *key,
                                        uint64_t key_length, uint64_t *out_index);
extern int kiln_shared_graph_field(uint64_t lease, uint64_t index, kiln_shared_field_info *out,
                                   uint16_t *key_out, uint64_t key_capacity,
                                   uint16_t *value_out, uint64_t value_capacity);
typedef struct kiln_shared_build_result {
    uint64_t lease;
    uint32_t graph;
    uint32_t generation;
    uint32_t node;
} kiln_shared_build_result;
typedef struct kiln_shared_source_node {
    uint32_t kind;
    uint32_t null_prototype;
    uint32_t length;
    uint64_t field_count;
} kiln_shared_source_node;
typedef struct kiln_shared_source_field {
    uint32_t key_kind;
    uint64_t key_id;
    uint64_t key_length;
    uint32_t value_kind;
    uint64_t value_bits;
    uint64_t value_length;
    uint32_t value_generation;
    uint32_t value_node;
    uint32_t enumerable;
    uint32_t accessor;
} kiln_shared_source_field;
typedef int (*kiln_shared_source_inspect_fn)(void *context, uint64_t object, uint64_t field,
                                              kiln_shared_source_node *node,
                                              kiln_shared_source_field *slot);
typedef int (*kiln_shared_source_unit_fn)(void *context, uint64_t string, uint64_t index,
                                           uint16_t *unit);
extern int kiln_shared_graph_build(uint64_t root, kiln_shared_source_inspect_fn inspect,
                                   kiln_shared_source_unit_fn unit, void *context,
                                   kiln_shared_build_result *out);
extern int kiln_shared_graph_refusal(uint32_t kind, const char **text);
extern int kiln_realm_gc_attach(const kiln_gc_hooks *hooks, void *candidate, void **out_heap);
extern int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags);
extern int kiln_realm_gc_mark_roots(void *heap);
/* Read-only provider and registered root inventory. `visit` receives a block payload (1) or a
 * pointer to a kiln_value slot (2). Zero means all roots were enumerated; one means a legacy
 * mark-only callback still has no visitor. */
typedef int (*kiln_gc_visit_root_fn)(void *context, const void *pointer, uint32_t kind);
#define KILN_GC_VISIT_BLOCK 1u
#define KILN_GC_VISIT_VALUE 2u
#define KILN_GC_VISIT_COMPACT 3u
extern int kiln_realm_gc_visit_provider_roots(void *heap, kiln_gc_visit_root_fn visit,
                                              void *context);
typedef int (*kiln_gc_visit_roots_fn)(void *context, kiln_gc_visit_root_fn visit,
                                      void *visit_context);
typedef void (*kiln_gc_release_external_context_fn)(void *context);
extern int kiln_realm_gc_has_root_visitor(void *heap, kiln_gc_visit_roots_fn visit_roots,
                                         void *context);
extern int kiln_realm_gc_register_roots_with_visitor(kiln_gc_roots_fn roots,
                                                      kiln_gc_visit_roots_fn visit_roots,
                                                      void *context, uint32_t flags);
extern int kiln_realm_is_active(void);

/* A closure is its body plus the boxes it captured.
 *
 * The body is a function pointer rather than an index into the emitting program's table,
 * because a function does not have to be called by the unit that made it: a module exports one,
 * another module imports and calls it, and an index would then be resolved against the wrong
 * table.  `proto` is kept only to name the function in text -- it is the same index the
 * interpreter uses, so both engines print `[Function 3]` for the same function. */
struct kiln_closure;
struct kiln_generator;
typedef struct kiln_call_context {
    kiln_host *host;
    kiln_value *thrown;
    struct kiln_generator *frame;
    int depth;
} kiln_call_context;

typedef int (*kiln_body_fn)(kiln_call_context *ctx, kiln_val * restrict result, kiln_val * restrict upvalues,
                            const kiln_val * restrict receiver, const kiln_val * restrict args, size_t argc);
enum kiln_c99_function_kind {
    KILN_C99_FUNCTION_ORDINARY = 0,
    KILN_C99_FUNCTION_METHOD = 1,
    KILN_C99_FUNCTION_CLASS = 2
};

typedef struct kiln_closure {
    kiln_body_fn body;
    /* Generated module closures stay bound to their graph instance across later resets. */
    kiln_host *module_host;
    void *external_owner;
    /* An arrow captures `new.target` where it was written, for the same reason it captures
     * `this`: it has none of its own. */
    int arrow;
    /* Whether this body is strict code, which is a property of the text it was written in and
     * not of the call. It decides one thing here: `OrdinaryCallBindThis`. A sloppy body called
     * with no receiver runs with the global object as `this`, which is how `var root = this ||
     * global` finds the realm; a strict one runs with the `undefined` it was passed. */
    int strict;
    int publishes_frame;
    /* Set on a closure that lives in the shared immutable region (ADR 0070 primitive B, realms
     * issue 08): made once per process by the realm that built the region, read by every realm,
     * and never written after the region is sealed. Its two mutable parts -- `own_prototype` and
     * `own_store` below -- are the realm's own and live in the realm's context instead
     * (`kiln_c99_closure_prototype`, `kiln_c99_closure_store`); read them only through those. */
    int region;
    kiln_val captured_this;
    kiln_val captured_new_target;
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
    /* Whether this function was written with method syntax. Methods have no `prototype`
     * unless they are generators, and cannot be constructed with `new`. */
    int is_method;
    int is_class;
    /* What a generator's frame has to be big enough for, known when the body is emitted. */
    uint32_t local_count;
    uint32_t stack_size;
    uint32_t handler_depth;
    /* The object `new f()` links instances to, and the store `f.x = 1` writes into. Both are
     * created on first use: an ordinary function that is never used as a constructor and never
     * carries a property allocates neither. A region closure's are the realm's, not these: every
     * read and write goes through `kiln_c99_closure_prototype{,_slot}` and
     * `kiln_c99_closure_store{,_slot}`, and the names differ from `prototype`/`properties` so that
     * a direct access elsewhere does not compile. */
    kiln_value own_prototype;
    kiln_value own_store;
    /* For the console, which prints `[Function: name]`.  NULL for an anonymous function, which
     * prints `[Function (anonymous)]` -- a distinct answer, not a missing one.  A name is a
     * canonical headed string so inferred names retain exact UTF-16 units and survive a closure
     * crossing module/GC boundaries without a byte shadow. */
    const kiln_string *name;
    /* Immutable source ownership and what `Function.prototype.toString` answers: this function's
     * own text, pointing into the
     * emitting unit's one copy of its source rather than owning a copy.  A closure outlives the
     * frame that made it and crosses unit boundaries, but the unit's static text outlives both.
     * NULL for a closure the runtime made for itself, which has no source and says so. Keeping
     * this beside the body pointer means diagnostics and future stack traces need no map,
     * allocation, or ambient current-module state. */
    const char *source;
    size_t source_len;
    /* Runtime-created functions carry their source as a canonical string value. The legacy
     * pointer/length pair remains for generated static source, whose bytes are UTF-8 literals. */
    kiln_value source_value;
    /* Immutable module ownership for diagnostics. This is deliberately distinct from `source`:
     * the latter is the function's exact source slice, not the module identifier. */
    const char *diagnostic_source;
    /* Declared parameters, which is what `f.length` answers and what `bind` subtracts from.
     * Counted where the body was compiled, because that is the only place it is known. */
    uint32_t arity;
    /* The `resume` position meaning "the parameters are bound and the body has not started".
     *
     * Binding them is part of the *call*, not of the first resumption: the language runs
     * FunctionDeclarationInstantiation and only then hands back the generator object, so
     * `function* g([{x}]) {}` called as `g([null])` throws where it is written and a parameter
     * default's side effect happens whether or not anyone ever steps the generator.
     *
     * Zero when there is nothing to run at the call: an ordinary function, a generator with no
     * pattern, default or rest to bind, and a plain async function -- whose instantiation
     * failure *rejects* rather than throwing, which is already what running the prologue inside
     * the body gives. */
    uint32_t prologue_resume;
    uint32_t proto;
    uint32_t upvalue_count;
    kiln_val inline_upvalues[4];
    kiln_val *upvalues;
} kiln_closure;

/* Reuse realm host services while routing generated module code through its captured graph. */
static inline kiln_host *kiln_c99_closure_host(kiln_host *host,
                                                const kiln_closure *closure,
                                                kiln_host *storage) {
    (void)storage;
    return closure != NULL && closure->module_host != NULL ? closure->module_host : host;
}

static inline kiln_host *kiln_c99_module_host(kiln_host *host) {
    return host != NULL && host->module_context != NULL ? host : NULL;
}

/* A JavaScript activation retained for Error.captureStackTrace. This is an explicit language
 * frame, not a C unwind frame: generated C records the module and IR instruction at each call
 * operation, and the common closure entry links the function's canonical name to it. */
typedef struct kiln_js_frame {
    struct kiln_js_frame *previous;
    const kiln_closure *closure;
    const kiln_string *method_name;
    const char *instruction_source;
    size_t instruction;
    size_t line;
    size_t column;
} kiln_js_frame;

/* Properties attached to runtime-provided callables. Method identity is its append-only numeric
 * code, so one realm-owned store per code gives `Stream.Readable`, `Buffer.constants`, and future
 * native constructor statics ordinary property behavior without changing the scalar value ABI or
 * pretending a method id is a closure pointer. */
typedef struct kiln_method_store {
    uint64_t code;
    kiln_value properties;
    struct kiln_method_store *next;
} kiln_method_store;

typedef enum kiln_native_prototype_surface {
    KILN_C99_NATIVE_EVENT_EMITTER,
    KILN_C99_NATIVE_READABLE,
    KILN_C99_NATIVE_WRITABLE,
    KILN_C99_NATIVE_SERVER_RESPONSE
} kiln_native_prototype_surface;

/* Generated from the checked Rust constructor graph: constructor identity, parentage, and surface
 * cannot drift from the compiler registry. The table is constant; each prototype is the realm's,
 * at the same index in `kiln_c99_realm_state.native_prototypes`. */
typedef struct kiln_native_constructor {
    uint64_t method;
    uint64_t parent;
    kiln_native_prototype_surface surface;
} kiln_native_constructor;

/* KILN_NATIVE_CONSTRUCTOR_TABLE */

/* A compiled unit's realm state (ADR 0070, realms issue 04 step 5): its module globals, then its
 * template sites, as one array of machine values each realm allocates the first time it enters
 * the unit and marks, and frees with its heap. `id` names the unit's row in every realm's table;
 * it is assigned once per process, the first time any realm asks. The cache is per thread and
 * trusted only under the realm generation it was filled in, exactly like `kiln_c99_gc_heap`'s. */
struct kiln_c99_unit {
    uint32_t id;
    uint32_t val_count;
};

typedef struct kiln_c99_unit_cache {
    const uint64_t *generation_slot;
    uint64_t generation;
    const void *module_context;
    uint64_t module_instance_id;
    kiln_val *vals;
} kiln_c99_unit_cache;

/* A shape describes the layout of an object's own properties (ADR 0050):
 * an ordered list of interned keys and slot indices. Objects with the same
 * sequence of added properties share the same shape. */
#define KILN_SHAPE_MAX_FAST_PROPERTIES 32

struct kiln_shape;

typedef struct kiln_shape_transition {
    const kiln_string *ikey;
    struct kiln_shape *next;
    struct kiln_shape_transition *next_transition;
} kiln_shape_transition;

typedef struct kiln_shape {
    struct kiln_shape *parent;
    const kiln_string *ikey;
    uint32_t slot_index;
    uint32_t property_count;
    uint32_t num_transitions;
    const kiln_string *keys[KILN_SHAPE_MAX_FAST_PROPERTIES];
    const kiln_string *inline_trans_ikey[2];
    struct kiln_shape *inline_trans_next[2];
    kiln_shape_transition *transitions;
    /* Whether any key on the path from the root is a symbol (`@@` spelt): the one thing a spread
     * of an object with this shape has to know before copying its slots wholesale. */
    uint32_t has_symbol_key;
    uint32_t double_slots_mask;
    /* ADR 0071. A *shared* shape is the root or a transition by a shared (process-wide) key from
     * a shared shape: it means the same layout in every realm, is never freed, and its edges are
     * published atomically because any realm's thread may add one. Every other shape is
     * realm-local: owned by the heap that made it (`owned_next` threads the heap's list of them),
     * freed when that heap is torn down, and reachable from a shared shape only through that
     * heap's own side table -- never through the shared shape's edges, which another realm reads.
     * `retained` marks a realm-local shape a teardown survivor still uses, which teardown then
     * leaks rather than frees. */
    /* Bit 0 means process-shared; bit 1 marks a complete built-in layout with no prefix
     * chain. A new property materializes this realm's dictionary. This uses the existing word
     * so the thousands of ordinary shapes do not grow for one final layout. */
    uint32_t shared;
    uint32_t retained;
    struct kiln_shape *owned_next;
    /* The transitions again, open-addressed by key pointer, once a shape has more than a few: a
     * shape every object literal starts from (the root) collects one branch per distinct first
     * key in the program *and* in the realm's bootstrap, and a list walk there is paid by every
     * literal and every constructor. NULL until `KILN_SHAPE_TABLE_FROM` branches exist; the list
     * above stays complete, so the table is only ever an index over it. A grown table replaces
     * its predecessor without freeing it (see `kiln_c99_shape_index_edge`). */
    struct kiln_shape_edge_table *edge_table;
    /* Issue 04 step 3b. Every slot's attributes (`KILN_C99_ATTR_*`), as the property record a
     * list-mode object would carry for it: zero is what an assignment creates -- a writable,
     * configurable data property, enumerable unless the key is reserved (`@@`) -- so every shape
     * built by assignment is all zeros and reads exactly as it did before shapes had attributes.
     * A transition is keyed by (key, attributes), so the attributes are part of the layout: two
     * objects of one shape agree on every slot's, and an inline cache that compared a shape
     * compared them too. The masks summarise `attrs` by slot bit for the hot tests:
     * `special_mask`, a slot that is anything but a plain assignment's property (a walk that
     * reads slots as enumerable data must leave shape mode first); `accessor_mask`, a slot that
     * holds an accessor pair (`kiln_c99_accessor_pair`), never a value; `store_mask`, a slot an
     * assignment may not simply overwrite (not writable, or an accessor). */
    uint32_t special_mask;
    uint32_t accessor_mask;
    uint32_t store_mask;
    /* Issue 04 step 3c, constant-method tracking. `code_mask` is the slots whose value this
     * shape states: each holds, in every object of this shape, a closure whose code (its
     * `body`) is recorded by the shape that added the slot -- `code`, NULL when that slot has no
     * record. A record is code, never a closure, so it means the same in every realm; it is
     * made only by a method *definition* (`kiln_c99_define_shaped`), and the code is part of the
     * transition's key with the attributes, so two definitions of one name with different code
     * are two shapes. A slot with a record is also in `store_mask`: an assignment never simply
     * overwrites it, and one that stores anything but a closure of the same code moves the
     * object to the shape without the record (`kiln_c99_shape_store_recorded`). So a shape's
     * record is true of every object that has the shape, and a call site that has matched the
     * shape may call the code without loading the closure. */
    uint32_t code_mask;
    kiln_body_fn code;
    uint8_t attrs[KILN_SHAPE_MAX_FAST_PROPERTIES];
} kiln_shape;

#define KILN_C99_SHAPE_FINAL 2u

/* The mask lives with the slots it describes, so one pointer load gives a reader both. A slot is
 * one word -- the child shape, whose `ikey` is the edge's key -- so a slot is claimed with a
 * single compare-and-swap and read with a single acquire load (ADR 0071). */
typedef struct kiln_shape_edge_table {
    uint32_t mask;
    struct kiln_shape *edges[];
} kiln_shape_edge_table;
/* A slot's attributes, as differences from what an assignment creates. */
/* enumerable is the opposite of an assignment's; for a reserved (`@@`) key, which walks over
 * string keys never list, it is the program's own answer instead: the symbol is hidden
 * (`kiln_property.symbol_hidden`) */
#define KILN_C99_ATTR_ENUM_FLIP 1u
#define KILN_C99_ATTR_READONLY 2u    /* not writable (an accessor always carries it) */
#define KILN_C99_ATTR_FIXED 4u       /* not configurable */
#define KILN_C99_ATTR_ACCESSOR 8u    /* the slot holds an accessor pair */

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
    const kiln_string *key;
    /* Whether `Object.keys`, `for...in`, spread and JSON see it. Every own property of an error
     * is readable by name and not enumerated, which is why `JSON.stringify(err)` is `{}`. */
    unsigned int enumerable : 1;
    /* A symbol key the program hid with a descriptor. This is a second bit rather than a value of
     * `enumerable` because a symbol key is stored with `enumerable = 0` unconditionally, so that
     * every walk over own *string* keys skips it without having to know about the `@@` prefix.
     * That forcing erases the program's own answer, and three separate reads wanted it back:
     * a spread carries symbol keys across and so must skip a hidden one; a descriptor reports
     * what the program asked for; and `Object.keys`/`JSON.stringify` must keep skipping it
     * whatever it asked. Overloading one bit for two questions answered all three wrongly at
     * once -- and two of them by printing the internal `@@symbol:N` spelling into program output.
     * Zero means visible, so a slot that nobody hid is correct from `calloc` alone. */
    unsigned int symbol_hidden : 1;
    /* Whether an assignment may replace the value, and whether `delete` may remove the property
     * or `defineProperty` redefine it. Both are true for everything an assignment or a literal
     * creates, which is why they went unmodelled for so long: only `Object.defineProperty` and
     * `Object.freeze` can produce a property where they are not, and only
     * `getOwnPropertyDescriptor` reports them back. Absent, a defined non-writable property took
     * writes and a non-configurable one deleted -- both silently. */
    unsigned int writable : 1;
    unsigned int configurable : 1;
    unsigned int accessor : 1;
    /* A name this realm publishes and cannot answer: reading it refuses rather than handing back
     * `undefined`. Only the node builtin namespaces install one, and only for a member the
     * shared ABI declares while the native materializer has no implementation behind it.
     *
     * The point is that the two are not the same absence. A member the language never had is
     * `undefined` and that is the right answer; a member this backend has not built is a *gap*,
     * and answering `undefined` for it silently fails the feature test a library wrote to find
     * out -- which is how a missing `path.dirname` became "a undefined is not a function" three
     * frames away in `postcss`. The refusal names the member without being told it, because the
     * instruction that read it already carries the name: `GetProperty(dirname)`.
     *
     * The same idea as `kiln_c99_builtin_backlog` one level down, and for the same reason. */
    unsigned int refused : 1;
    /* A property is either data or an accessor. The second accessor half occupies space a
     * data property never needs; keeping both representations here made every built-in data
     * property pay for three values instead of one. `accessor` selects the live union arm. */
    union {
        kiln_value value;
        struct {
            kiln_value getter;
            kiln_value setter;
        };
    };
} kiln_property;

/* A complete built-in store layout, published once per process. The entries own no realm
 * values: keys are canonical process strings, and attributes are immutable descriptor bits.
 * A realm may use this table only after its store's complete key/attribute sequence validates. */
typedef struct kiln_sealed_property {
    const kiln_string *key;
    uint8_t enumerable;
    uint8_t writable;
    uint8_t configurable;
    uint8_t symbol_hidden;
} kiln_sealed_property;

typedef struct kiln_sealed_layout {
    struct kiln_sealed_layout *next;
    uint32_t tag;
    uint64_t code;
    size_t count;
    size_t bucket_count;
    /* A uint32_t open-addressed key index follows entries[count]. Zero is empty; a filled
     * bucket stores the descriptor index plus one. Both arrays are process-owned and sealed. */
    kiln_sealed_property entries[];
} kiln_sealed_layout;

/* Realm-owned values and insertion order for a sealed layout. Value slots are followed by one
 * ordinal per descriptor and then ordinals for realm-only appended keys. Zero means absent.
 * The header stays immediately before `kiln_object.slots`, so reallocating the tail can move
 * the block without changing the object's identity. */
typedef struct kiln_sealed_values {
    uint32_t next_order;
    uint32_t extra_capacity;
    uint32_t dense_order;
    kiln_val values[];
} kiln_sealed_values;

/* The global object's stable site in the complete descriptor table. Other ordinary-object
 * sites will use compiler-assigned identities, never a realm pointer. */
#define KILN_C99_SEALED_GLOBAL_CODE UINT64_MAX
/* `process.env` is built from the host's environment. The complete first-realm layout is
 * reusable only for later realms with the same keys; an unexpected key materializes locally. */
#define KILN_C99_SEALED_ENV_CODE (UINT64_MAX - UINT64_C(1))

/* Most bootstrap objects have fewer than eight shaped properties. Keeping sixteen slots in
 * every kiln_object made the many small per-realm objects pay for unused values. Wider objects
 * already grow into a separate slot array, so this reduces their inline prefix without changing
 * their layout or the shape's slot numbering. See realms issue 08's same-binary RSS comparison. */
#define KILN_C99_INLINE_SLOTS 8

enum {
    KILN_C99_WRAPPER_NONE = 0,
    KILN_C99_WRAPPER_STRING = 1,
    KILN_C99_WRAPPER_NUMBER = 2,
    KILN_C99_WRAPPER_BOOLEAN = 3,
    KILN_C99_WRAPPER_SYMBOL = 4,
    KILN_C99_WRAPPER_BIGINT = 5
};

/* A realm-private lookup accelerator. Buckets contain ordered-vector ordinals only;
 * keys and values remain owned and traced exclusively through the property records. */
typedef struct kiln_dictionary_index {
    size_t capacity;
    size_t count;
    size_t buckets[];
} kiln_dictionary_index;

typedef struct kiln_object {
    kiln_property *properties;
    size_t count;
    size_t capacity;
    /* `Object.freeze`. A frozen object ignores every write rather than refusing one: modules are
     * strict and a real engine would throw, but the interpreter is silent here and the two have
     * to agree before either can change. */
    int frozen;
    /* `Object.preventExtensions`. Separate from `frozen`, because it is strictly weaker: a
     * non-extensible object still takes writes to the properties it already has, and only
     * refuses to grow. `Object.isFrozen` on one is false, which is exactly the distinction a
     * library checks before deciding an object is safe to mutate in place. */
    int not_extensible;
    /* Module namespace objects are non-extensible and reject [[Set]] even though their exported
     * data properties report writable=true. This is deliberately separate from `frozen`: a
     * namespace is not an ordinary frozen object, and reflection must preserve that distinction. */
    int module_namespace;
    void *external_owner;
    /* The `arguments` object of a call. An internal slot rather than a `Symbol.toStringTag`
     * property, because the tag is not a property: neither `getOwnPropertyNames` nor
     * `getOwnPropertySymbols` may report it, and a program must not be able to delete it. */
    int arguments_brand;
    /* An error, by the same rule as `arguments_brand`: the brand is not a property. */
    int error_brand;
    /* `%JSON.rawJSON%`'s unforgeable raw-text carrier brand. The source text itself is the
     * immutable enumerable `rawJSON` property, so the collector already traces it with slots. */
    int raw_json_brand;
    /* Error stack activation records are engine metadata, not own properties. */
    kiln_value error_stack_frames;
    /* How many of `properties` currently have `accessor` set. `kiln_c99_put_mode`'s chain walk
     * exists to find an inherited setter; when a prototype level's count is zero, it has none,
     * and the level costs one pointer follow instead of a scan of its whole property list.
     *
     * A level with `accessor_count == 0` can still hold a non-writable *data* property under the
     * same key, which must refuse the shadowing write; `read_only_seen` below covers that, so
     * the two together make the skip exact rather than a bet on what the Node surface defines.
     *
     * Kept exact by every site that flips a property's `accessor` bit, so it is never a scan in
     * disguise. */
    size_t accessor_count;
    /* Set once a data property of this object has been made non-writable, and never cleared:
     * with `accessor_count`, it is what lets `kiln_c99_put_mode` skip a prototype level's scan
     * exactly. A level that never held an accessor or a read-only data property has nothing
     * that could intercept a write from a descendant. */
    int read_only_seen;
    /* The next object a property read falls through to, or undefined for none.  Undefined and
     * null are different answers here, exactly as they are to `Object.getPrototypeOf`: one means
     * nothing was recorded and the other means nothing was wanted.
     *
     * Placed directly before `shape` (issue 04 step 3c): a method call through the prototype
     * reads the receiver's shape, its prototype link and then its slots, and with the link here
     * the three share the receiver's cache lines instead of the link costing one of its own. */
    kiln_value prototype;
    /* Shape-based layout (ADR 0050). When non-NULL, properties are stored in
     * the slots array indexed by shape->keys. Appended to preserve member offsets. */
    kiln_shape *shape;
    kiln_val *slots;
    size_t slot_capacity;
    kiln_val inline_slots[KILN_C99_INLINE_SLOTS];
    /* Dense elements vector for integer keys (ADR 0050 Milestone 5) */
    kiln_val *elements;
    size_t element_count;
    size_t element_capacity;
    /* ADR 0071. The layout token of an object in list (dictionary) mode that some inline cache
     * relies on: such an object has no shape to name its key set by, so the first cache that
     * walks through it issues it a process-unique number (`kiln_c99_pic_name_level`), and any
     * mutation of it -- an own property written, defined or deleted, a prototype link changed
     * (`kiln_c99_prototype_touched`) -- clears it. A cache entry that names the token is valid
     * while the object still carries it. Tokens are never reused, so an entry can never match
     * another realm's object, or a later object at a freed one's address. Zero means none; a
     * shaped object never has one. Since shapes carry attributes (issue 04 step 3b) a class
     * prototype is shaped and needs none: a token names only a dictionary-mode level -- an object
     * past the shape limits, or one a deletion, `defineProperty`, a freeze or a reconfiguration
     * moved out of shape mode. Occupies what was padding after the last member. */
    uint32_t layout_token;
    /* NULL for ordinary shape/list storage. A non-NULL process descriptor changes only storage:
     * this full object and its prototype remain owned by the realm. Reads project the descriptor
     * and this object's slots; mutation can materialize the ordinary list in place. */
    const kiln_sealed_layout *sealed;
    /* A dictionary property buffer allocated inside an actor candidate carries its owner
     * and allocation provenance immediately before the records. Ordinary stores do not. */
    uint32_t candidate_properties;
    /* Provenance for a shaped object's separately allocated value slots. */
    uint32_t candidate_slots;
    /* Provenance for a shaped object's separately allocated numeric elements. */
    uint32_t candidate_elements;
    /* Boxed primitive for String/Number/Boolean/Symbol/BigInt wrapper objects. The wrapper is
     * still an ordinary object for identity, properties and `typeof`; only the matching
     * prototype methods and ToPrimitive inspect this slot. */
    uint8_t primitive_wrapper_kind;
    kiln_value boxed_primitive;
    kiln_dictionary_index *dictionary_index;
} kiln_object;

typedef struct kiln_c99_candidate_object_properties {
    uint64_t candidate_id;
    kiln_object *owner;
    size_t capacity;
    uint64_t generation;
} kiln_c99_candidate_object_properties;

static inline kiln_c99_candidate_object_properties *kiln_c99_object_candidate_header(
    const kiln_object *object) {
    return ((kiln_c99_candidate_object_properties *)object->properties) - 1;
}

typedef struct kiln_c99_candidate_object_slots {
    uint64_t candidate_id;
    kiln_object *owner;
    size_t capacity;
    uint64_t generation;
} kiln_c99_candidate_object_slots;

static inline kiln_c99_candidate_object_slots *kiln_c99_object_candidate_slots_header(
    const kiln_object *object) {
    return ((kiln_c99_candidate_object_slots *)object->slots) - 1;
}

typedef struct kiln_c99_candidate_object_elements {
    uint64_t candidate_id;
    kiln_object *owner;
    size_t capacity;
    uint64_t generation;
} kiln_c99_candidate_object_elements;

static inline kiln_c99_candidate_object_elements *kiln_c99_object_candidate_elements_header(
    const kiln_object *object) {
    return ((kiln_c99_candidate_object_elements *)object->elements) - 1;
}

/* A prepared graph can keep a dictionary's fixed records in the same movable GC block.
 * Ordinary dictionaries own a separate malloc buffer. */
static inline int kiln_c99_object_properties_in_block(const kiln_object *object) {
    return object->properties == (const kiln_property *)(object + 1);
}

static inline void kiln_c99_dictionary_index_reset(kiln_object *object) {
    free(object->dictionary_index);
    object->dictionary_index = NULL;
}

static inline void kiln_c99_object_properties_free(kiln_object *object) {
    kiln_c99_dictionary_index_reset(object);
    if (!kiln_c99_object_properties_in_block(object))
        free(object->candidate_properties ? (void *)kiln_c99_object_candidate_header(object)
                                          : (void *)object->properties);
    object->candidate_properties = 0;
}

/* A shaped object's slots can be its fixed prefix, a tail reserved in the same GC block by
 * `kiln_c99_new_object_with_capacity`, or a separately malloc'd array. Only the last one may
 * be handed to realloc/free. Sealed-layout values have their own owner and are handled before
 * this helper is used. */
static inline int kiln_c99_object_slots_owned(const kiln_object *object) {
    return object->slots != NULL && object->slots != object->inline_slots &&
           object->slots != (const kiln_val *)(object + 1);
}

static inline void kiln_c99_object_slots_free(kiln_object *object) {
    if (kiln_c99_object_slots_owned(object))
        free(object->candidate_slots ? (void *)kiln_c99_object_candidate_slots_header(object)
                                     : (void *)object->slots);
    object->candidate_slots = 0;
}

static inline void kiln_c99_object_elements_free(kiln_object *object) {
    free(object->candidate_elements ? (void *)kiln_c99_object_candidate_elements_header(object)
                                    : (void *)object->elements);
    object->candidate_elements = 0;
}

static kiln_val *kiln_c99_object_slots_resize(kiln_object *object, size_t capacity);

static inline kiln_sealed_values *kiln_c99_sealed_values_of(const kiln_object *object) {
    return (kiln_sealed_values *)((char *)object->slots -
                                  offsetof(kiln_sealed_values, values));
}

/* The only realm-heap state for a sealed graph node. Its lease retains the graph in the Rust
 * registry; the opaque ID is copied into message tokens, never interpreted as a pointer. */
typedef struct kiln_shared_facade {
    uint64_t lease;
    kiln_shared_arena_view arena;
    uint32_t graph;
    uint32_t generation;
    uint32_t node;
    /* Weak, realm-local identity chain and hash bucket. The collector traces neither. */
    void *owner;
    struct kiln_shared_facade *previous;
    struct kiln_shared_facade *next;
    struct kiln_shared_facade *hash_next;
} kiln_shared_facade;

/* Nonzero once any realm in this process has written an own property onto some built-in's store
 * (`kiln_c99_callable_store` with `create`). It is only a process-wide summary: which built-ins
 * a realm has modified is that realm's own record (`kiln_c99_realm_builtin_modified`, on the
 * heap), and a cache entry never holds a value of it. While it is zero no realm has modified any
 * built-in, which is what the emitted built-in speculations ask; once it is not, a cached
 * built-in member asks the current realm's record instead.
 *
 * One per program: the prelude unit defines it and exports it, and the copy of this header a
 * decoupled module unit compiles declares it `extern` instead (`runtime_prelude_header`), since
 * a definition in every unit would give each a copy of its own that never moves. Hidden in that
 * declaration, so a read is PC-relative rather than a load through the GOT.
 *
 * Realms on several threads move it with atomic adds; the emitted speculations read it as a plain
 * word (`kiln_c99_builtins_unmodified`), which is sound because the only change that matters to a
 * realm's reading is its own store, made earlier on the same thread -- another realm's add only
 * sends it to its own record.
 * The stores a realm's bootstrap makes (installing TypeScript built-ins) are part of what the
 * built-ins *are*, not a modification of them: each realm takes its own back when its bootstrap
 * ends (`kiln_c99_builtin_bootstrap_done`), and everything else it added when it is torn down. So
 * the word is the number of stores live realms have made to built-ins outside their bootstraps,
 * and zero whenever none has -- which the launcher used to arrange by zeroing it after each
 * bootstrap, erasing whatever another realm running at the time had done. */
static uint64_t kiln_c99_builtin_store_epoch;

/* Whether the built-ins are the current realm's pristine ones, which is what every emitted
 * built-in speculation asks (an inlined `Math.sqrt`, an `arr.map` loop): at once while no live
 * realm has stored onto a built-in outside its bootstrap, and otherwise by asking the current
 * realm's own record -- another realm's bootstrap or modification is not this realm's, and a
 * typed body that took the summary alone would refuse whenever any realm in the process did. */
/* Pure: it writes nothing a caller can see (at most it fills this thread's heap cache), so a loop
 * that asks it can still hoist the summary's load. Cold, and on clang for x86-64 it saves every
 * register it uses itself, vector registers included: an ordinary call on the unlikely side made
 * the hot side keep a typed body's doubles on the stack across it (1.5 instructions per call on
 * a class-poly loop, measured). */
#if defined(__clang__) && defined(__x86_64__)
#define KILN_C99_RARE_CALL __attribute__((cold, preserve_all))
#else
#define KILN_C99_RARE_CALL __attribute__((cold))
#endif
#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_realm_builtins_unmodified(void) __attribute__((pure)) KILN_C99_RARE_CALL;
#else
static int kiln_c99_realm_builtins_unmodified(void) __attribute__((pure)) KILN_C99_RARE_CALL;
#endif
static KILN_ALWAYS_INLINE inline int kiln_c99_builtins_unmodified(void) {
    return KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) || kiln_c99_realm_builtins_unmodified();
}

/* The narrow forms (issue 20): a speculation names what it relies on, and a store onto some other
 * built-in leaves it alone. The hot side is the same one compare; the cold side asks the current
 * realm's program record. `kiln_c99_builtins_unmodified_mask` is object-granular (an array
 * method's fast path relies on Array and Array.prototype as a whole); the member form is for a
 * speculation on one own member of one built-in (`Math.sqrt`), so `Math.foo = 1` leaves it fast. */
#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_realm_builtins_untouched(uint64_t mask0, uint64_t mask1, int beyond)
    __attribute__((pure)) KILN_C99_RARE_CALL;
extern int kiln_c99_realm_builtin_member_untouched(uint32_t builtin, const char *member, size_t len)
    __attribute__((pure)) KILN_C99_RARE_CALL;
extern int kiln_c99_realm_cached_builtin_member_untouched(uint32_t builtin, const char *member,
                                                          size_t len) KILN_C99_RARE_CALL;
#else
static int kiln_c99_realm_builtins_untouched(uint64_t mask0, uint64_t mask1, int beyond)
    __attribute__((pure)) KILN_C99_RARE_CALL;
static int kiln_c99_realm_builtin_member_untouched(uint32_t builtin, const char *member, size_t len)
    __attribute__((pure)) KILN_C99_RARE_CALL;
static int kiln_c99_realm_cached_builtin_member_untouched(uint32_t builtin, const char *member,
                                                          size_t len) KILN_C99_RARE_CALL;
#endif
/* Bit `id` of the bitmap's word `word`: a compile-time constant for a named built-in. */
#define KILN_C99_BIT_OF_BUILTIN(id, word) \
    (((uint32_t)(id) >> 6) == (uint32_t)(word) ? (UINT64_C(1) << ((uint32_t)(id) & 63u)) : UINT64_C(0))
static KILN_ALWAYS_INLINE inline int kiln_c99_builtins_unmodified_mask(uint64_t mask0, uint64_t mask1,
                                                                        int beyond) {
    return KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) ||
           kiln_c99_realm_builtins_untouched(mask0, mask1, beyond);
}
/* Two built-ins, object-granular: an array method relies on Array (its species) and
 * Array.prototype (the method itself), a string method on String and String.prototype. */
#define kiln_c99_builtin_pair_unmodified(a, b)                                                     \
    kiln_c99_builtins_unmodified_mask(KILN_C99_BIT_OF_BUILTIN(a, 0) | KILN_C99_BIT_OF_BUILTIN(b, 0),     \
                                      KILN_C99_BIT_OF_BUILTIN(a, 1) | KILN_C99_BIT_OF_BUILTIN(b, 1),     \
                                      (a) >= 128 || (b) >= 128)
/* A typed entry whose body reaches nothing relies on its callers to have asked about the members
 * it inlines (the emitter's `CallPlan::typed_guards`), and does not ask again. Under
 * `KILN_IC_CHECK` it asks, and traps when a call site entered it without checking. */
#ifdef KILN_IC_CHECK
#define KILN_C99_TYPED_RELIES_CHECK(guard)                                                        \
    if (KILN_UNLIKELY(!(guard))) {                                                                \
        fputs("kiln: typed-entry oracle: entered with a built-in it relies on modified; a call "  \
              "site skipped CallPlan::typed_guards\n",                                             \
              stderr);                                                                             \
        __builtin_trap();                                                                          \
    }
#else
#define KILN_C99_TYPED_RELIES_CHECK(guard)
#endif
/* One own member of one built-in, spelt by a string literal. */
#define kiln_c99_builtin_member_unmodified(id, member)                                             \
    (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) ||                                              \
     kiln_c99_realm_builtin_member_untouched((uint32_t)(id), (member), sizeof(member) - 1))
static KILN_ALWAYS_INLINE inline int kiln_c99_builtin_member_unmodified_n(uint32_t id,
                                                                           const char *member,
                                                                           size_t length) {
    return KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) ||
           kiln_c99_realm_builtin_member_untouched(id, member, length);
}

#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_realm_builtin_modified(uint32_t builtin);
#else
static int kiln_c99_realm_builtin_modified(uint32_t builtin);
#endif

/* Issue 30: a method a site remembered for a receiver kind (`kiln_c99_cached_method_matches`) is
 * the kind's method table's, which a member of that name the program wrote onto the kind's
 * prototype (or onto Object.prototype) shadows. The hot side is the summary's one compare. */
#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_realm_tag_method_pristine(uint8_t tag, uint64_t code) __attribute__((pure))
    KILN_C99_RARE_CALL;
#else
static int kiln_c99_realm_tag_method_pristine(uint8_t tag, uint64_t code) __attribute__((pure))
    KILN_C99_RARE_CALL;
#endif
static KILN_ALWAYS_INLINE inline int kiln_c99_tag_method_pristine(uint8_t tag, uint64_t code) {
    return KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) || kiln_c99_realm_tag_method_pristine(tag, code);
}

/* Issue 30: an emitted method fast path (`s.trim()`, `a.join(",")`, `m.get(k)`) answers by name
 * for whichever kind the receiver is, so it relies on that name being the table's on every kind
 * prototype: none of them holds a member of that name the program wrote. Hot side: one compare. */
#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_realm_kind_member_untouched(const char *member, size_t len)
    __attribute__((pure)) KILN_C99_RARE_CALL;
#else
static int kiln_c99_realm_kind_member_untouched(const char *member, size_t len)
    __attribute__((pure)) KILN_C99_RARE_CALL;
#endif
#define kiln_c99_kind_member_unmodified(member)                                                    \
    (KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) ||                                              \
     kiln_c99_realm_kind_member_untouched((member), sizeof(member) - 1))

/* Issue 30: whether the program has written onto `builtin` in this realm, for a read that is
 * about to answer from `builtin`'s method table and must let a written member shadow it. */
#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_realm_program_wrote(uint32_t builtin);
#else
static int kiln_c99_realm_program_wrote(uint32_t builtin);
#endif
static KILN_ALWAYS_INLINE inline int kiln_c99_program_wrote_builtin(uint32_t builtin) {
    return KILN_UNLIKELY(kiln_c99_builtin_store_epoch != 0) && kiln_c99_realm_program_wrote(builtin);
}

/* Every mutation of an object some inline cache might read a property through calls this: an own
 * property written, defined or deleted, a prototype link changed, a conversion to dictionary
 * mode. A shaped object's key set is its shape, which such a mutation changes when it matters;
 * a list-mode object's is its layout token, which this clears. An object nobody caches through
 * costs one compare. */
static inline void kiln_c99_prototype_touched(const kiln_object *object) {
    if (KILN_UNLIKELY(object->layout_token != 0)) ((kiln_object *)object)->layout_token = 0;
}



/* A prototype chain is built by the program, so its length is the program's business -- but the
 * walk is on the C stack, where running out aborts rather than reports. Bounded, like the
 * interpreter's. */
#define KILN_C99_MAX_PROTOTYPE_DEPTH 10000

/* A compiled named-access site's memory of where its key was last found (ADR 0047): a slot on
 * the receiver, or a slot on the receiver's immediate prototype. Objects have no shape, so a
 * hit is validated by the key itself: keys are unique within an object and canonical across
 * the realm, so `properties[own].key == key` proves the slot *is* the property, on any object
 * whatsoever -- and objects built by the same code put their keys at the same indices, which is
 * what makes one guess right for every instance of a class that does not exist. One of these is
 * a block-scope static at each GetProperty, SetProperty, SetPropertyStrict and CallMethod. */
#define KILN_C99_PIC_SIZE 8

/* ---- Publishing shared cache state (ADR 0071, realms issue 04 step 4) ----------------------
 *
 * Every cache below is one process-wide static that the realms of the process share, and those
 * realms may run on several threads at once. An answer a cache holds is a *slot*: a few words
 * named by one key word, the receiver shape the answer is for. The key word is the protocol:
 *
 *   - A reader loads it with acquire (`kiln_c99_slot_key`) and compares it with its object's
 *     shape. It reads the other words only after a match, and they are complete by then.
 *   - A writer claims a slot by moving its key word to CLAIMED with a compare-and-swap, writes
 *     the other words, and publishes them with a release store of the shape
 *     (`kiln_c99_slot_claim`, `kiln_c99_slot_publish`). A writer that finds a slot claimed by
 *     another does not learn this time; it never waits.
 *   - A writer that replaces an answer claims the published slot the same way. It may rewrite it
 *     at once only when no other thread is running a realm (`kiln_realm_retire_tag` answers 0),
 *     which is every single-realm process. Otherwise the slot is RETIRED with that tag in its key
 *     word, and is claimed again only once the tag is safe: every thread that could have been
 *     reading the old answer has since passed a turn boundary (quiescent-state reclamation, the
 *     turn as its quantum).
 *
 * So a reader can never pair one answer's key with another answer's words, and a slot's words
 * are never rewritten under a reader -- with no lock, no allocation, and on the hit path nothing
 * but the acquire load, which on x86-64 is the plain load it replaced. No state below is a shape
 * any object has (shapes are at least 4-byte aligned, and an object's shape is one or NULL), so a
 * reader needs no test beyond its compare. */
/* The loads the hit paths make of shared cache words. An acquire load on x86-64 is a plain `mov`
 * -- the machine never reorders a load before an older load -- so all it asks of the compiler is
 * not to move later loads above it. Spelled as an atomic load, it also stops the compiler folding
 * the load into the compare that uses it (one more instruction on every hit); spelled as a
 * volatile load followed by a compiler-only acquire fence, it folds and keeps its order, which is
 * the same machine code an acquire load has minus that instruction. Elsewhere it is the atomic
 * acquire load. `KILN_C99_LOAD_ONCE` is the relaxed kind, for a word that is an answer by itself
 * (a hint, an index checked where it is used): one untorn load the compiler neither splits nor
 * repeats. */
#if defined(__x86_64__) || defined(__i386__)
#define KILN_C99_LOAD_ACQUIRE(pointer)                                                    \
    __extension__({                                                                    \
        __typeof__(*(pointer)) kiln_c99_loaded_ = *(volatile __typeof__(*(pointer)) *)(pointer); \
        __atomic_signal_fence(__ATOMIC_ACQUIRE);                                        \
        kiln_c99_loaded_;                                                              \
    })
#else
#define KILN_C99_LOAD_ACQUIRE(pointer) __atomic_load_n((pointer), __ATOMIC_ACQUIRE)
#endif
#define KILN_C99_LOAD_ONCE(pointer) (*(volatile __typeof__(*(pointer)) *)(pointer))

#define KILN_C99_SLOT_UNLEARNED ((const kiln_shape *)(uintptr_t)1)
#define KILN_C99_SLOT_CLAIMED ((const kiln_shape *)(uintptr_t)2)
#define KILN_C99_SLOT_RETIRED(tag) ((const kiln_shape *)(uintptr_t)(((uint64_t)(tag) << 2) | 3u))
#define KILN_C99_SLOT_IS_RETIRED(key) (((uintptr_t)(key) & 3u) == 3u)
#define KILN_C99_SLOT_RETIRED_TAG(key) ((uint64_t)(uintptr_t)(key) >> 2)
/* A slot's key word names an answer: a shape, not NULL (never written) or a state above. */
#define KILN_C99_SLOT_IS_ANSWER(key) ((key) != NULL && ((uintptr_t)(key) & 3u) == 0u)

static KILN_ALWAYS_INLINE inline const kiln_shape *kiln_c99_slot_key(const kiln_shape *const *key) {
    return KILN_C99_LOAD_ACQUIRE(key);
}

static inline void kiln_c99_slot_publish(const kiln_shape **key, const kiln_shape *shape) {
    __atomic_store_n(key, shape, __ATOMIC_RELEASE);
}

/* Claim the slot named by `key` for writing. A free slot (never written, or UNLEARNED) is taken;
 * a retired one only once its tag is safe; a published answer only when `replace` says the caller
 * means to replace it -- and then, when another thread may be reading it, it is retired instead
 * and the claim fails. 1: the slot is CLAIMED and the caller must publish it (or retire it). */
static __attribute__((noinline, unused)) int kiln_c99_slot_claim(const kiln_shape **key,
                                                                   int replace) {
    const kiln_shape *seen = __atomic_load_n(key, __ATOMIC_ACQUIRE);
    uint64_t tag;
    if (seen == KILN_C99_SLOT_CLAIMED) return 0;
    if (KILN_C99_SLOT_IS_RETIRED(seen) && !kiln_realm_retired_safe(KILN_C99_SLOT_RETIRED_TAG(seen)))
        return 0;
    if (KILN_C99_SLOT_IS_ANSWER(seen) && !replace) return 0;
    /* Sequentially consistent: unpublishing an answer must be ordered before the count of online
     * threads `kiln_realm_retire_tag` reads (a thread coming online announces itself, then
     * reads). */
    if (!__atomic_compare_exchange_n(key, &seen, KILN_C99_SLOT_CLAIMED, 0, __ATOMIC_SEQ_CST,
                                     __ATOMIC_RELAXED))
        return 0;
    if (!KILN_C99_SLOT_IS_ANSWER(seen)) return 1;
    tag = kiln_realm_retire_tag();
    if (tag == 0) return 1;
    __atomic_store_n(key, KILN_C99_SLOT_RETIRED(tag), __ATOMIC_RELEASE);
    return 0;
}

/* Unpublish the answer `key` names, if it still names `shape`: RETIRED, reusable once safe. */
static __attribute__((noinline, unused)) void kiln_c99_slot_retire(const kiln_shape **key,
                                                                    const kiln_shape *shape) {
    const kiln_shape *seen = shape;
    if (!KILN_C99_SLOT_IS_ANSWER(shape) ||
        !__atomic_compare_exchange_n(key, &seen, KILN_C99_SLOT_CLAIMED, 0, __ATOMIC_SEQ_CST,
                                     __ATOMIC_RELAXED))
        return;
    __atomic_store_n(key, KILN_C99_SLOT_RETIRED(kiln_realm_retire_tag()), __ATOMIC_RELEASE);
}

/* One entry of a site's polymorphic cache (ADR 0071): an answer stated in realm-independent
 * terms -- layouts, paths and slots, never an object -- so one process-wide site serves every
 * realm, and a new realm starts with the caches earlier ones warmed.
 *
 * `shape` is the receiver's. An own data property is its `slot` there (`transition_to` set when
 * a store moves the receiver to a new shape). Anything else is a *path*: `depth` prototype steps
 * from the receiver, each level named by its shape or, for a level in list mode, which has none,
 * by its layout token (`kiln_object.layout_token`) -- or, for the list-mode level holding the
 * member, by the key its record carries. A hit follows the receiver's live prototype links
 * `depth` steps, checks each level is the one named, and reads the answer live:
 *
 *   - a member found at `depth` (1 or 2) is `proto_slot` there -- a slot of a shaped holder, or the
 *     index into a list-mode holder's property list, valid while the record there still carries
 *     `key` (a dictionary-mode prototype: past the shape limits, or reconfigured). A class
 *     prototype is shaped -- shapes carry attributes (issue 04 step 3b) -- so its methods are
 *     slots, and the common entry, one step to a shaped holder, is marked `one_step`. So a
 *     reassigned method is seen (the value is read, not remembered), a shadowing property added
 *     on the way changes a shape or clears a token and misses, and a receiver whose prototype was
 *     swapped reaches a different level and misses.
 *   - `absent` says the key is on none of the receiver and levels 1..`depth`, and the last one's
 *     prototype ends the chain (null, or the implicit Object.prototype while this realm has not
 *     modified it). A read answers undefined; a transitioning store knows nothing intercepts it.
 *
 * A shape is a key path and says nothing about the prototype, so two classes with one field
 * order share a receiver shape; only the live walk tells their methods apart. A member deeper
 * than two steps is not remembered.
 *
 * An entry is a slot (see `KILN_C99_SLOT_*` above) keyed by `shape`: published whole, never
 * changed while published, replaced only by the slot protocol (`kiln_c99_site_remember`). */
typedef struct kiln_c99_pic_entry {
    /* The key word: read with `kiln_c99_slot_key`, written only by the slot protocol. */
    const kiln_shape *shape;
    const kiln_shape *transition_to;
    uint32_t slot;
    uint32_t proto_slot;
    uint8_t depth;
    uint8_t absent;
    /* Set by `kiln_c99_site_remember` when the entry is the common prototype member: found one
     * step up, in a shaped holder named by `level_shape[0]` (a class prototype's method, since
     * shapes carry attributes). The hit is then the prototype's shape check and one slot load,
     * with no walk. Derived from the fields above, never learned on its own. */
    uint8_t one_step;
    /* Set on an entry a computed site learned for one key of many (a symbol-keyed access,
     * `kiln_c99_val_read_symbol`): it answers only for a receiver of `shape` read under `key`,
     * which every hit compares. Such a site is found by (shape, key), so one receiver shape read
     * under two symbols -- or under each realm's own `Symbol()` -- keeps two entries. A named
     * site's key is its constant and it never sets this. */
    uint8_t keyed;
    uint32_t level_token[2];
    const kiln_shape *level_shape[2];
    /* The canonical key the path was learned for. A list-mode holder is not named by a token:
     * the record at `proto_slot` must carry this key -- a shared key means the same name in every
     * realm, so such an entry serves them all. */
    const kiln_string *key;
} kiln_c99_pic_entry;

typedef struct kiln_c99_site {
    /* Dictionary indices are biased by one: zero is the all-zero, not-yet-learned state and a
     * learned property at index n is stored as n + 1.  Shape/PIC entries use their own
     * `pic_count` state and are not part of this encoding.  Keep the representation in sync with
     * the generated site's zero initializer and the decode at the two dictionary consumers. */
    uint32_t own;
    uint32_t proto;
    /* Slow-path answers that taught the site nothing: a name answered from a builtin table or a
     * deeper prototype. After `KILN_C99_SITE_GIVE_UP` of them the site stops scanning to learn.
     * A count, so a lost increment between two realms' threads only delays giving up. */
    uint32_t misses;
    /* How many entries have ever been claimed (`kiln_c99_site_count`); the ones past it were never
     * written. Only grows: a forgotten entry is retired in place, not removed. */
    uint32_t pic_count;
    kiln_c99_pic_entry entries[KILN_C99_PIC_SIZE];
    uint8_t cached_tag;
    uint8_t has_cached_method;
    uint64_t cached_method;
    /* Exact ASCII property key for cached builtin-method answers. Native method ids can have
     * aliases, so bootstrap facade checks must be tied to the spelling this site read. */
    uint8_t cached_member_len;
    char cached_member[32];
    /* A builtin receiver's method is cached by which builtin it was, and nothing else: it holds
     * while the current realm has not modified that built-in (`kiln_c99_realm_builtin_modified`),
     * a record every realm keeps for itself. */
    uint32_t cached_builtin;
    /* The serial of the last heap this site was registered with, because an entry names one of
     * that heap's realm-local shapes or keys; the heap forgets such entries when it is torn down,
     * before it frees them (`kiln_c99_gc_forget_realm_local`). Zero: never registered. */
    uint64_t registered;
} kiln_c99_site;

/* The one-word fields of a site -- the dictionary indices `own` and `proto`, `misses`, the
 * cached method's flag -- are each an answer on its own, checked where it is used (a dictionary
 * index by the key its record must carry), so they are read and written as single relaxed words;
 * only the cached method's other words are published behind its flag
 * (`kiln_c99_site_cache_method`). */

static KILN_ALWAYS_INLINE inline uint32_t kiln_c99_site_count(const kiln_c99_site *site) {
    return KILN_C99_LOAD_ACQUIRE(&site->pic_count);
}

/* The published entry that answers for `shape`, or NULL. */
static KILN_ALWAYS_INLINE inline const kiln_c99_pic_entry *kiln_c99_site_entry(
    const kiln_c99_site *site, const kiln_shape *shape) {
    uint32_t p, count = kiln_c99_site_count(site);
    for (p = 0; p < count; ++p) {
        if (kiln_c99_slot_key(&site->entries[p].shape) == shape) return &site->entries[p];
    }
    return NULL;
}

/* The published entry of a keyed site (`kiln_c99_pic_entry.keyed`) that answers for `shape` read
 * under `key`, or NULL. The key is compared only after the key word matched, when the entry's
 * other words are complete. */
static KILN_ALWAYS_INLINE inline const kiln_c99_pic_entry *kiln_c99_site_entry_keyed(
    const kiln_c99_site *site, const kiln_shape *shape, const kiln_string *key) {
    uint32_t p, count = kiln_c99_site_count(site);
    for (p = 0; p < count; ++p) {
        if (kiln_c99_slot_key(&site->entries[p].shape) == shape && site->entries[p].key == key)
            return &site->entries[p];
    }
    return NULL;
}

/* Whether the site should learn an answer for `shape`: it has one (to be replaced) or has room. */
static inline int kiln_c99_site_may_learn(const kiln_c99_site *site, const kiln_shape *shape) {
    return kiln_c99_site_count(site) < KILN_C99_PIC_SIZE || kiln_c99_site_entry(site, shape) != NULL;
}

static inline uint32_t kiln_c99_site_own(const kiln_c99_site *site) {
    return KILN_C99_LOAD_ONCE(&site->own);
}

static inline uint32_t kiln_c99_site_proto(const kiln_c99_site *site) {
    return KILN_C99_LOAD_ONCE(&site->proto);
}

static inline void kiln_c99_site_set_dictionary(kiln_c99_site *site, uint32_t own, uint32_t proto) {
    __atomic_store_n(&site->own, own, __ATOMIC_RELAXED);
    __atomic_store_n(&site->proto, proto, __ATOMIC_RELAXED);
}

/* A site's cached method -- a method-call or property-read site whose receiver is a built-in
 * kind -- is three words behind `has_cached_method`, a one-byte slot: 0 none, 2 being written, 1
 * published with release. A published answer and its property-key bytes remain immutable;
 * another receiver kind or key takes the slow path. */
#define KILN_C99_SITE_METHOD_WRITING 2u

static KILN_ALWAYS_INLINE inline int kiln_c99_site_has_method(const kiln_c99_site *site) {
    return KILN_C99_LOAD_ACQUIRE(&site->has_cached_method) == 1u;
}

static __attribute__((noinline, unused)) void kiln_c99_site_cache_method(kiln_c99_site *site,
                                                                          uint8_t tag,
                                                                          uint64_t method,
                                                                          uint32_t builtin,
                                                                          const char *member,
                                                                          size_t member_len) {
    uint8_t seen = __atomic_load_n(&site->has_cached_method, __ATOMIC_ACQUIRE);
    if (seen == 1u && site->cached_tag == tag && site->cached_method == method &&
        site->cached_builtin == builtin && site->cached_member_len == member_len &&
        (member_len == 0 || (member_len < sizeof site->cached_member &&
                             memcmp(site->cached_member, member, member_len) == 0)))
        return;
    /* Published sites are immutable. A call site can be shared by realm threads; replacing its
     * key buffer after publication would race readers and invalidate realm member-answer caches. */
    if (seen == 1u) return;
    if (seen == KILN_C99_SITE_METHOD_WRITING ||
        !__atomic_compare_exchange_n(&site->has_cached_method, &seen,
                                     (uint8_t)KILN_C99_SITE_METHOD_WRITING, 0, __ATOMIC_SEQ_CST,
                                     __ATOMIC_RELAXED))
        return;
    site->cached_tag = tag;
    site->cached_method = method;
    site->cached_builtin = builtin;
    if (tag == KILN_VALUE_BUILTIN) {
        size_t index;
        if (member == NULL || member_len >= sizeof site->cached_member) {
            site->cached_member_len = UINT8_MAX;
        } else {
            for (index = 0; index < member_len; ++index) {
                if ((unsigned char)member[index] >= 0x80u) break;
            }
            if (index != member_len) {
                site->cached_member_len = UINT8_MAX;
            } else {
                memcpy(site->cached_member, member, member_len);
                site->cached_member_len = (uint8_t)member_len;
            }
        }
    } else {
        site->cached_member_len = 0;
    }
    __atomic_store_n(&site->has_cached_method, (uint8_t)1u, __ATOMIC_RELEASE);
}

/* A `length` site's receiver-kind hint -- the kind it last saw -- kept in the same two bytes a
 * method site uses for its method; a site is only ever one of the two. A hint is never trusted: it
 * picks which check to make first, and the receiver's own kind confirms it (an array and a typed
 * array share a boxing prefix, and another realm's thread may have just written the hint). */
static inline uint8_t kiln_c99_site_hint(const kiln_c99_site *site) {
    return KILN_C99_LOAD_ONCE(&site->cached_tag);
}

/* The hint of a site that has never cached a method (a method site's tag names its method's
 * receiver kind, not a `length`), or 0. */
static KILN_ALWAYS_INLINE inline uint8_t kiln_c99_site_length_hint(const kiln_c99_site *site) {
    return KILN_C99_LOAD_ONCE(&site->has_cached_method) == 0u ? kiln_c99_site_hint(site)
                                                                              : 0u;
}

/* Hints are written only when they change: a site is a line every realm's thread reads, and a
 * store of the value already there would still take the line from all of them. */
static inline void kiln_c99_site_set_hint(kiln_c99_site *site, uint8_t tag) {
    if (KILN_C99_LOAD_ONCE(&site->cached_tag) != tag)
        __atomic_store_n(&site->cached_tag, tag, __ATOMIC_RELAXED);
    if (KILN_C99_LOAD_ONCE(&site->has_cached_method) != 0u)
        __atomic_store_n(&site->has_cached_method, (uint8_t)0u, __ATOMIC_RELAXED);
}


/* The compact emission's inline check reads only a site's first entry, and a later one answers
 * through a call. When the same later entry has answered `KILN_C99_SITE_PROMOTE_AFTER` times
 * running -- the site has changed phase -- it moves to the front (`kiln_c99_site_promote`). A site
 * whose receivers alternate never promotes, so it never pays a swap per access; and a streak whose
 * promotion was declined (another thread runs a realm) stays full.
 *
 * The streak is this thread's own: which later entry of which site answered it last, and how many
 * times running, in a small direct-mapped table. Counting on the site itself would write a
 * process-wide line on every such hit, which every other realm's thread reading the site would
 * then miss on; a collision in the table only restarts a streak. */
#define KILN_C99_SITE_PROMOTE_AFTER 8u
#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_site_promote(kiln_c99_site *site, uint32_t hot);
#else
static int kiln_c99_site_promote(kiln_c99_site *site, uint32_t hot);
#endif

typedef struct kiln_site_streak {
    const kiln_c99_site *site;
    uint32_t entry;
    uint32_t length;
} kiln_site_streak;
static _Thread_local kiln_site_streak kiln_c99_streaks[64];

static inline void kiln_c99_site_answered(kiln_c99_site *site, uint32_t hot) {
    kiln_site_streak *streak = &kiln_c99_streaks[((uintptr_t)site >> 6) & 63u];
    if (streak->site != site || streak->entry != hot) {
        streak->site = site;
        streak->entry = hot;
        streak->length = 0;
    } else if (streak->length >= KILN_C99_SITE_PROMOTE_AFTER) {
        return;
    }
    /* Promoted, the entry at `hot` is now the one that was first: its streak starts over. */
    if (++streak->length == KILN_C99_SITE_PROMOTE_AFTER && kiln_c99_site_promote(site, hot))
        streak->length = 0;
}

static inline void kiln_c99_site_missed(kiln_c99_site *site) {
    __atomic_fetch_add(&site->misses, 1u, __ATOMIC_RELAXED);
}

static inline uint32_t kiln_c99_site_misses(const kiln_c99_site *site) {
    return KILN_C99_LOAD_ONCE(&site->misses);
}

/* A computed site's symbol-keyed cache (`kiln_c99_val_read_symbol`): a pointer to a site
 * allocated the first time the site learns about a symbol, zero until then. */
typedef kiln_c99_site *kiln_c99_symbol_cell;

/* A private-field site: one slot (`KILN_C99_SLOT_*`) keyed by the receiver's shape. A realm-local
 * shape -- an object that also carries a `Symbol()` key, as every `Event` does (realms issue 17) --
 * is learned like a shared one, and the site then registers with its realm's heap, which retires
 * the entry before it frees the shape at teardown (`kiln_c99_gc_register_private_site`), as a
 * property site's are. `registered` is that heap's serial, a hint against registering twice. */
typedef struct kiln_c99_private_site {
    const struct kiln_shape *shape;
    uint32_t brand_slot;
    uint32_t member_slot;
    uint64_t registered;
} kiln_c99_private_site;

/* Whether a literal site may remember `shape` and hand it to objects of any realm without the
 * transitions that would have produced it (ADR 0071). Only a *shared* shape qualifies: it is a
 * layout of shared (process-wide) keys, never freed, and a spelling is never a shared key and a
 * live realm's own key at once, so building the same literal by transitions in any realm reaches
 * exactly this shape. A realm-local shape names keys of the realm that made it and is freed with
 * its heap; remembering one would hand a later realm freed memory or foreign keys. */
#define KILN_C99_SHAPE_IS_PORTABLE(shape) ((shape)->shared != 0)


/* The slot a property-read site remembers for `shape`, or UINT32_MAX when it remembers none
 * or remembers a path instead. */
static inline uint32_t kiln_c99_site_slot_for(const kiln_c99_site *site, const kiln_shape *shape) {
    const kiln_c99_pic_entry *entry = kiln_c99_site_entry(site, shape);
    if (entry != NULL && entry->depth == 0 && !entry->absent && entry->transition_to == NULL)
        return entry->slot;
    return UINT32_MAX;
}

/* The level an entry's path ends at, reached from `obj` by its live prototype links, or NULL when
 * a level on the way is not the one the entry names. `obj` itself when the path is empty. */
static KILN_ALWAYS_INLINE inline const kiln_object *kiln_c99_pic_path(const kiln_c99_pic_entry *entry,
                                                   const kiln_object *obj) {
    const kiln_object *level = obj;
    uint32_t d;
    for (d = 0; d < entry->depth; ++d) {
        const kiln_shape *named = entry->level_shape[d];
        if (level->prototype.tag != KILN_VALUE_OBJECT) return NULL;
        level = (const kiln_object *)(uintptr_t)level->prototype.bits;
        if (level->shape != named) return NULL;
        /* A list-mode level is named by its token -- except a holder named by its key, whose
         * token is zero and whose record the caller checks. */
        if (named == NULL && entry->level_token[d] != 0 &&
            level->layout_token != entry->level_token[d])
            return NULL;
    }
    return level;
}

/* Whether the implicit Object.prototype -- where an object whose prototype link is undefined
 * finds its inherited members -- is still the built-in one in the current realm. */
static inline int kiln_c99_object_prototype_unmodified(void) {
    return KILN_LIKELY(kiln_c99_builtin_store_epoch == 0) ||
           !kiln_c99_realm_builtin_modified(18u /* KILN_C99_BUILTIN_OBJECT_PROTOTYPE */);
}

/* Whether the current realm's Object.prototype own-property store lacks `key` (a canonical
 * pointer), scanned live. The bootstrap itself writes that store (the Annex B `__defineGetter__`
 * family, `annexb.ts`), so in practice every realm has "modified" Object.prototype before the
 * program's first line, and the test above alone would never let an absent path that ends there
 * hit -- which is every miss on a plain object. What the store holds is what a read can find
 * there, so a key it lacks is still absent; the method table beside it is fixed, and a key it
 * answers was never learned as absent. Consulted, never copied into an entry (ADR 0071). */
#ifdef KILN_SEPARATE_PRELUDE
extern int kiln_c99_object_prototype_lacks(const kiln_string *key);
#else
static int kiln_c99_object_prototype_lacks(const kiln_string *key);
#endif

#ifdef KILN_IC_CHECK
/* The oracle for an absent answer: the slow path, asked again, must find nothing. */
#ifdef KILN_SEPARATE_PRELUDE
extern void kiln_c99_pic_absent_oracle(const kiln_object *obj, const kiln_string *key);
#else
static void kiln_c99_pic_absent_oracle(const kiln_object *obj, const kiln_string *key);
#endif
#endif

#ifdef KILN_IC_CHECK
/* The oracle for a mutation path that forgot `kiln_c99_prototype_touched`: every list-mode level
 * a hit trusted by its layout token must still lack the key. A build with `KILN_IC_CHECK` in its
 * environment checks every such hit the long way and traps on a disagreement. */
static __attribute__((noinline)) void kiln_c99_pic_oracle(const kiln_c99_pic_entry *entry,
                                                          const kiln_object *obj) {
    const kiln_object *level = obj;
    uint32_t d;
    for (d = 0; d < entry->depth && entry->key != NULL; ++d) {
        size_t i;
        level = (const kiln_object *)(uintptr_t)level->prototype.bits;
        if (level->shape != NULL || entry->level_token[d] == 0) continue;
        for (i = 0; i < level->count && level->properties != NULL; ++i) {
            if (level->properties[i].key == entry->key ||
                kiln_string_equal(level->properties[i].key, entry->key)) {
                fputs("kiln: inline-cache oracle: a list-mode level gained a key under an unchanged layout token; a mutation path forgot kiln_c99_prototype_touched\n", stderr);
                __builtin_trap();
            }
        }
    }
}
#endif

/* The answer an entry that is not an own slot gives for `obj`, whose shape it has matched: the
 * member at the end of its path, read live, or undefined for a key absent along it. 0 when the
 * entry is an own slot, or the path from `obj` is not the one remembered. */
/* A dictionary-mode holder's record at `proto_slot`, when it still carries the entry's key as a
 * data property. Out of line: since shapes carry attributes (issue 04 step 3b) a class prototype
 * is shaped, and a list-mode holder is the uncommon case this keeps off every inlined site. */
static __attribute__((noinline, unused)) int kiln_c99_pic_list_holder(const kiln_c99_pic_entry *entry,
                                                                     const kiln_object *level,
                                                                     kiln_val *out) {
    const kiln_property *record;
    if (KILN_UNLIKELY(entry->proto_slot >= level->count)) return 0;
    record = &level->properties[entry->proto_slot];
    if (KILN_UNLIKELY(record->key != entry->key || record->accessor)) return 0;
    *out = record->value.tag == KILN_VALUE_FUNCTION
               ? KILN_VAL_TAG_PTR(KILN_TAG_CLOSURE_PREFIX, (const void *)(uintptr_t)record->value.bits)
               : kiln_val_from_boundary(record->value);
    return 1;
}

static KILN_ALWAYS_INLINE inline int kiln_c99_pic_inherited(const kiln_c99_pic_entry *entry, const kiln_object *obj,
                                         kiln_val *out) {
    const kiln_object *level;
    if (KILN_LIKELY(entry->one_step)) {
        if (KILN_UNLIKELY(obj->prototype.tag != KILN_VALUE_OBJECT)) return 0;
        level = (const kiln_object *)(uintptr_t)obj->prototype.bits;
        if (KILN_UNLIKELY(level->shape != entry->level_shape[0])) return 0;
        *out = level->slots[entry->proto_slot];
        return 1;
    }
    if (entry->depth == 0 && !entry->absent) return 0;
    level = kiln_c99_pic_path(entry, obj);
    if (level == NULL) return 0;
#ifdef KILN_IC_CHECK
    kiln_c99_pic_oracle(entry, obj);
#endif
    if (entry->absent) {
        /* The receiver's shape is its whole key set only with no list beside it, and an error,
         * an `arguments` object and a namespace answer some names by kind rather than by key. */
        if (KILN_UNLIKELY(obj->properties != NULL || obj->error_brand || obj->arguments_brand ||
                          obj->module_namespace)) {
            return 0;
        }
        if (level->prototype.tag == KILN_VALUE_NULL ||
            (level->prototype.tag == KILN_VALUE_UNDEFINED &&
             (kiln_c99_object_prototype_unmodified() ||
              (entry->key != NULL && kiln_c99_object_prototype_lacks(entry->key))))) {
#ifdef KILN_IC_CHECK
            kiln_c99_pic_absent_oracle(obj, entry->key);
#endif
            *out = KILN_VAL_UNDEFINED;
            return 1;
        }
        return 0;
    }
    if (level->shape != NULL) {
        *out = level->slots[entry->proto_slot];
        return 1;
    }
    return kiln_c99_pic_list_holder(entry, level, out);
}

/* The callee a method-call site remembers for an object of `obj`'s shape, or 0. One that is the
 * object's own property is remembered by slot and read from *this* object, because two objects
 * of one shape hold their own values -- caching the value for an own property is what called
 * the first subscriber's callback for every later one. One found on the prototype chain is read
 * live at the end of the entry's path, after the path from this object is checked level by
 * level. */
static KILN_ALWAYS_INLINE inline kiln_val kiln_c99_site_cached_callee(const kiln_c99_site *site, const kiln_object *obj) {
    const kiln_c99_pic_entry *entry = kiln_c99_site_entry(site, obj->shape);
    kiln_val answer;
    if (entry == NULL) return 0;
    if (entry->slot < obj->slot_capacity) return obj->slots[entry->slot];
    if (kiln_c99_pic_inherited(entry, obj, &answer)) return answer;
    return 0;
}
/* Issue 04 step 3c. The code `shape` records for `slot`, or NULL: the record lives on the shape
 * that added the slot, so this walks up to it. Not a hot path -- call sites learn from it once. */
static inline kiln_body_fn kiln_c99_shape_slot_code(const kiln_shape *shape, uint32_t slot) {
    if (shape == NULL || slot >= shape->property_count || (shape->code_mask & (1u << slot)) == 0)
        return NULL;
    while (shape->property_count > slot + 1) shape = shape->parent;
    return shape->code;
}

/* A typed method-call site's memory of one candidate's path (ADR 0071, issue 04 steps 3b and
 * 3c): the receiver's shape, its prototype's shape, and the slot there whose code the
 * prototype's shape records as the candidate's. Layouts only, and both shapes shared -- never
 * freed, the same in every realm -- so the static serves every realm. A hit is the receiver's
 * shape and the prototype's shape, and then the candidate's code is called with no closure
 * loaded: the receiver's shape is the proof the name is not an own property, and the
 * prototype's shape that the slot holds a closure of exactly that code (a shape's code record
 * is true of every object that has it; see `kiln_shape.code_mask`). The slot is read only for
 * what the call takes from the closure itself, its upvalues. `row` is the candidate's receiver
 * row for `recv` (`kiln_c99_recv_row`), learned with it, for a candidate that takes one; a path
 * is only learned once the row is known, so a hit passes it without another shape check.
 *
 * A path is one slot (`KILN_C99_SLOT_*`) keyed by `recv`, which a hit reads with
 * `kiln_c99_slot_key` before anything else. */
typedef struct kiln_c99_method_path {
    const kiln_shape *recv;
    const kiln_shape *proto;
    const uint32_t *row;
    uint32_t slot;
} kiln_c99_method_path;

/* A path nothing has taught: its receiver shape is a slot state no shape is, so a hit needs no
 * separate test that the receiver is shaped (a dictionary-mode receiver's NULL never matches). */
#define KILN_C99_UNLEARNED_METHOD_PATH { KILN_C99_SLOT_UNLEARNED, NULL, NULL, 0 }

/* Whether the prototype at the end of `path` for `obj`, whose shape the caller has matched to
 * `path->recv`, has the path's shape; `*level` is that prototype. */
static KILN_ALWAYS_INLINE inline int kiln_c99_method_path_hit(const kiln_c99_method_path *path,
                                                             const kiln_object *obj,
                                                             const kiln_object **level) {
    if (KILN_UNLIKELY(obj->prototype.tag != KILN_VALUE_OBJECT)) return 0;
    *level = (const kiln_object *)(uintptr_t)obj->prototype.bits;
    return KILN_LIKELY((*level)->shape == path->proto);
}

/* Teach `path` from the site entry that just answered for `obj` with a closure of `code`: only
 * an entry that is a shaped prototype one step up, only shared shapes, since the path is a
 * process-wide static, and only a prototype shape that records `code` for the slot. A candidate
 * that takes a receiver row (`need_row`) learns only with `row`, the row for `obj`'s shape. */
static inline void kiln_c99_method_path_learn(kiln_c99_method_path *path, const kiln_c99_site *site,
                                              const kiln_object *obj, kiln_body_fn code,
                                              const uint32_t *row, int need_row) {
    const kiln_shape *shape = obj->shape;
    const kiln_c99_pic_entry *entry;
    if (shape == NULL || !shape->shared || (need_row && row == NULL)) return;
    entry = kiln_c99_site_entry(site, shape);
    if (entry == NULL || !entry->one_step || !entry->level_shape[0]->shared ||
        kiln_c99_shape_slot_code(entry->level_shape[0], entry->proto_slot) != code)
        return;
    /* Already this answer: nothing to publish (and nothing for another realm to lose). */
    if (kiln_c99_slot_key(&path->recv) == shape && path->proto == entry->level_shape[0] &&
        path->slot == entry->proto_slot && path->row == row)
        return;
    if (!kiln_c99_slot_claim(&path->recv, 1)) return;
    path->proto = entry->level_shape[0];
    path->slot = entry->proto_slot;
    path->row = row;
    kiln_c99_slot_publish(&path->recv, shape);
}

/* The slow path of integer arithmetic -- a result outside the safe range, computed as a
 * double -- passes through here so that it stays a branch. Inlined beside the fast path, the
 * compiler folds both into a select, and the loop-carried integer then waits on the double
 * conversions every iteration; a call it cannot speculate keeps the cold path cold. */
__attribute__((noinline, cold)) static double kiln_c99_int_slow(double value) {
    /* Opaque to the optimiser: an identity it could see through would be deleted, and the
     * conversions would be folded back beside the fast path. */
    #if defined(__aarch64__)
    __asm__ volatile("" : "+w"(value));
    #elif defined(__x86_64__) || defined(_M_X64)
    __asm__ volatile("" : "+x"(value));
    #else
    __asm__ volatile("" : "+m"(value));
    #endif
    return value;
}

#define KILN_C99_SITE_NONE 0xFFFFFFFFu
#define KILN_C99_SITE_GIVE_UP 8u

static inline uint32_t kiln_c99_site_encode_dictionary_index(size_t index) {
    return index < (size_t)KILN_C99_SITE_NONE ? (uint32_t)index + UINT32_C(1) : 0;
}

static inline uint32_t kiln_c99_site_decode_dictionary_index(uint32_t encoded) {
    /* Zero is deliberately decoded as UINT32_MAX: existing bounds checks then reject the empty
     * state without a second branch. */
    return encoded - UINT32_C(1);
}

/* A proxy's two halves.
 *
 * Both are held as ordinary values because both are: a handler is a plain object whose
 * properties are the traps, and a target is whatever was proxied -- including another proxy,
 * which is why neither is narrowed to a kind here. */
typedef struct kiln_proxy {
    kiln_value target;
    kiln_value handler;
    /* `Proxy.revocable`'s second half was called. A revoked proxy is not an empty one: every
     * operation on it is a TypeError, which is the entire point -- the capability it stood for
     * has been withdrawn, and a program holding a stale reference has to find out rather than
     * see a plausible answer. */
    int revoked;
} kiln_proxy;

typedef enum kiln_gc_kind {
    KILN_GC_NONE = 0,
    KILN_GC_STRING,
    KILN_GC_SLICED_STRING,
    KILN_GC_BYTES,
    KILN_GC_OBJECT,
    KILN_GC_CLOSURE,
    KILN_GC_CELL,
    KILN_GC_ARRAY,
    KILN_GC_SHARED_FACADE,
    KILN_GC_GENERATOR,
    KILN_GC_PROMISE,
    KILN_GC_SETTLER,
    KILN_GC_BOUND,
    KILN_GC_COLLECTION,
    KILN_GC_PROXY,
    KILN_GC_DATE,
    KILN_GC_REGEXP,
    KILN_GC_SYMBOL,
    KILN_GC_TYPED_BUFFER,
    KILN_GC_TYPED_ARRAY,
    /* A DataView holds one edge -- the buffer it borrows -- and owns no growth buffer of its own,
     * so it is traced and never finalised. The bytes belong to KILN_GC_TYPED_BUFFER, which is the
     * one place they are released. */
    KILN_GC_DATA_VIEW,
    KILN_GC_COMMONJS_REQUIRE,
    KILN_GC_METHOD_STORE,
    KILN_GC_NODE_SERVER,
    KILN_GC_NODE_SOCKET,
    KILN_GC_NODE_CLIENT_REQUEST,
    KILN_GC_NODE_RESPONSE,
    KILN_GC_NODE_COMPLETION,
    KILN_GC_NODE_FS_COMPLETION,
    KILN_GC_NODE_PROCESS_COMPLETION,
    KILN_GC_NEXT_TICK,
    KILN_GC_TIMER,
    /* GC owner for external native state. Never cloned into shared heaps. */
    KILN_GC_EXTERNAL_OWNER,
    KILN_GC_KIND_COUNT
} kiln_gc_kind;

typedef struct kiln_gc_external_owner {
    void *context;
    kiln_gc_visit_roots_fn visit_roots;
    kiln_gc_release_external_context_fn release_context;
} kiln_gc_external_owner;

typedef struct kiln_gc_cell {
    /* First member preserves existing generated `kiln_val *` cell access. */
    kiln_val value;
    void *external_owner;
} kiln_gc_cell;

extern void *kiln_c99_external_owner_create(void *context,
                                          kiln_gc_visit_roots_fn visit_roots,
                                          kiln_gc_release_external_context_fn release_context);
extern void kiln_c99_external_owner_detach(void *owner);
extern int kiln_c99_object_set_external_owner(kiln_value object_value, void *owner);
extern void kiln_c99_closure_set_external_owner(kiln_closure *closure, void *owner);
extern int kiln_c99_val_make_cell_owned(kiln_val *slot, void *owner);

/* The kind of a heap payload, read from the block header in front of it. The header is exactly
 * one `kiln_gc_block` (`heap.c`) before the payload whatever its origin -- nursery, slab or
 * malloc -- so this is pointer arithmetic and one load, inline wherever a boxed pointer's kind is
 * asked on a hit path (an element site telling an array from a typed array, which share a boxing
 * prefix). `heap.c` asserts the offset and mask against the struct. */
#define KILN_C99_BLOCK_KIND_OFFSET 8u
#define KILN_C99_BLOCK_KIND_MASK ((uint32_t)0x03FFFFFFu)
static KILN_ALWAYS_INLINE inline uint32_t kiln_c99_gc_payload_kind(const void *payload) {
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
    /* Every kind is below 256 and the flags live in the top bits, so the kind is the word's low
     * byte: one byte compare, where masking the word costs a load, a constant and an `and`. */
    return *((const uint8_t *)payload - KILN_C99_BLOCK_KIND_OFFSET);
#else
    uint32_t kind;
    memcpy(&kind, (const char *)payload - KILN_C99_BLOCK_KIND_OFFSET, sizeof kind);
    return kind & KILN_C99_BLOCK_KIND_MASK;
#endif
}


/* A function's receiver-shape cache: four rows of shapes seen as `this`, and for each a slot
 * per name the body reads of `this`. The row for `shape`, or NULL: a read through a row is one
 * indexed load, with no absent-slot test and no capacity test, because a row is only ever
 * claimed whole (below) and an object holds every slot its shape names. */
/* A function's receiver rows are write-once slots (`KILN_C99_SLOT_*`): row `j` is claimed, filled
 * and published by a release store of `shapes[j]`, and never written again -- a row is a pure
 * function of its shape, so it never needs replacing, and nothing is ever retired. */
static inline const uint32_t *kiln_c99_recv_row(const kiln_shape **shapes, const uint32_t *slots,
                                                size_t columns, const kiln_shape *shape) {
    size_t j;
    /* No shape, no row. An unclaimed row's key is NULL too, and its columns are not slots: a
     * receiver whose shape word reads NULL -- a `Date`, whose payload is not a shaped object --
     * matched the first empty row and read `slots[UINT32_MAX]` (`Date.prototype[Symbol.
     * toPrimitive]` reading `this.valueOf`, parallel-builtins issue 18). */
    if (shape == NULL) return NULL;
    for (j = 0; j < 4; ++j) {
        const kiln_shape *row_shape = kiln_c99_slot_key(&shapes[j]);
        if (row_shape == shape) return slots + j * columns;
        if (row_shape == NULL) break;
    }
    return NULL;
}

/* Claims a row for `obj`'s shape when one is free and every name is an own data slot of the
 * shape -- the shape knows its keys in slot order, so the whole row is filled from it here and
 * needs no slow path to learn from. A shape with a name that is not its own -- a getter, a
 * prototype's -- gets no row and is remembered as refused, so the attempt is not repeated;
 * those reads take the site cache. The caller has checked the kind of the object first: a
 * proxy is boxed like an object and whatever lies at its shape offset is not a shape to read
 * keys from. `keys` are the realm's interned pointers, as a shape's are. */
/* Whether a claim for `shape` was refused before, so it is not tried again. */
static inline int kiln_c99_recv_refused(const kiln_shape *const *refused, const kiln_shape *shape) {
    size_t j;
    for (j = 0; j < 4; ++j) {
        if (KILN_C99_LOAD_ONCE(&refused[j]) == shape) {
            return 1;
        }
    }
    return 0;
}

static inline const uint32_t *kiln_c99_recv_claim(const kiln_shape **shapes,
                                                  const kiln_shape **refused, uint32_t *slots,
                                                  size_t columns, const kiln_string *const *keys,
                                                  const kiln_object *obj,
                                                  uint32_t double_mask) {
    const kiln_shape *shape = obj->shape;
    size_t j, c;
    /* Rows are process-wide statics, so only a shared shape gets one (ADR 0071): a realm-local
     * shape is freed with its realm, and a later shape at its address must not find its row. It
     * is not refused either -- the site cache serves it -- because the same claim is tried again
     * only by an object of the same local shape. */
    if (shape == NULL || !shape->shared) {
        return NULL;
    }
    /* Rows are claimed in order, one claim in flight per table: a claimer that meets a row
     * another thread is filling waits the few instructions that takes and then reads the row
     * again, so realms starting together fill one row per shape exactly as one realm alone
     * would -- racing claimers must not each take a row for the same shape and leave none for
     * the next. */
    for (j = 0; j < 4; ++j) {
        const kiln_shape *seen = kiln_c99_slot_key(&shapes[j]);
        const kiln_shape *empty = NULL;
        while (seen == KILN_C99_SLOT_CLAIMED) seen = kiln_c99_slot_key(&shapes[j]);
        if (seen == shape) return slots + j * columns;
        if (seen != NULL) continue;
        if (__atomic_compare_exchange_n(&shapes[j], &empty, KILN_C99_SLOT_CLAIMED, 0,
                                        __ATOMIC_ACQUIRE, __ATOMIC_RELAXED))
            break;
        --j; /* Another claim won this row: read it again. */
    }
    if (j == 4) {
        return NULL;
    }
    for (c = 0; c < columns; ++c) {
        uint32_t i;
        uint32_t found = UINT32_MAX;
        for (i = 0; i < shape->property_count; ++i) {
            if (shape->keys[i] == keys[c]) {
                found = i;
                break;
            }
        }
        /* An accessor's slot holds its pair, not the value a read of the name answers. */
        if (found == UINT32_MAX || (shape->accessor_mask & (1u << found)) != 0 ||
            ((double_mask & (1u << c)) && !(shape->double_slots_mask & (1u << found)))) {
            size_t r;
            for (r = 0; r < 4; ++r) {
                const kiln_shape *empty = NULL;
                if (__atomic_compare_exchange_n(&refused[r], &empty, shape, 0, __ATOMIC_RELAXED,
                                                __ATOMIC_RELAXED))
                    break;
            }
            /* The claimed row goes back unwritten: nothing read it, and it is free again. */
            __atomic_store_n(&shapes[j], (const kiln_shape *)NULL, __ATOMIC_RELEASE);
            return NULL;
        }
        slots[j * columns + c] = found;
    }
    kiln_c99_slot_publish(&shapes[j], shape);
    return slots + j * columns;
}


/* An array's elements are dense and its `length` is derived, not stored: writing past the end
 * grows it and the gap is filled with holes, which read as undefined and are not the same thing --
 * `1 in [1, , 3]` is false and `[1, , 3].map(f)` does not call `f` for the middle. */
typedef struct kiln_array {
    kiln_val *elements;
    size_t count;
    size_t capacity;
    /* Properties that are not elements: `a.total = 1`, and the `index`, `input` and `groups` a
     * match result carries. Undefined until the first one is written, so an ordinary array is one
     * pointer smaller than it looks and costs nothing. */
    kiln_value properties;
    /* `Object.freeze`. An element write is ignored, like a property write on a frozen object;
     * a method that would change the length throws, because there is no way to do half of it. */
    int frozen;
    /* `Object.preventExtensions`. An existing index still takes a write; a new one does not. */
    int not_extensible;
    /* Index descriptors that cannot live in the dense element vector are stored in the array's
     * ordinary property store. This bit keeps dense readers on their fast path unless an index
     * has an accessor or non-default attributes. */
    int has_index_descriptors;
    int64_t match_index;
    kiln_value match_input;
    /* The `groups` object of a match with named captures, built when the match is; undefined
     * for a pattern with none, which is what `m.groups` answers then. */
    kiln_value match_groups;
    kiln_val inline_elements[8];
    /* Whether this array is proven to contain only numbers (dense double backing, no holes). */
    int is_numeric;
    /* A separately allocated element run made in an actor candidate carries its provenance
     * immediately before the elements. Other arrays use the ordinary malloc layout. */
    uint32_t candidate_elements;
} kiln_array;

typedef struct kiln_c99_candidate_array_elements {
    uint64_t candidate_id;
    kiln_array *owner;
    size_t capacity;
    uint64_t generation;
} kiln_c99_candidate_array_elements;

static inline kiln_c99_candidate_array_elements *kiln_c99_array_candidate_header(
    const kiln_array *array) {
    return ((kiln_c99_candidate_array_elements *)array->elements) - 1;
}

/* Elements the array did not take from `malloc`, and so must never hand back to `free` or to
 * `realloc`.
 *
 * There are two such runs and they are the same mistake. `inline_elements` is the array's own
 * first eight, and the run past the end of the block is the room
 * `kiln_c99_new_array_with_capacity` takes when the count is known and larger than eight -- a
 * split into nine parts used to pay a `malloc` and a `free` for the ninth. Neither pointer came
 * out of the allocator, so growing past either is a fresh `malloc` and a copy, exactly as it was
 * for `inline_elements` alone; `realloc`ing one of them is the crash a bundled `ajv` produced the
 * day small arrays were first given inline storage.
 *
 * The run is inside the array's own block, so the tracer reaches it through `elements` like any
 * other and the collector frees it with the array. */
static inline int kiln_c99_array_elements_owned(const kiln_array *array) {
    return array->elements != NULL && array->elements != array->inline_elements &&
           array->elements != (const kiln_val *)(array + 1);
}

/* ---- Tier 2 of the parallel built-ins: callbacks that read the heap (ADR 0069, issue 07) ----
 *
 * A tier-2 callback (`parallel::heap_verdict`) reads arrays, typed arrays, the own and inherited
 * data properties of ordinary objects, and strings, and writes nothing. Its clone (`_h`, emitted
 * by `parallel_heap.rs`) runs on the pool's threads while the realm thread is blocked in the
 * call, so nothing can change the heap; and nothing collects, because nothing allocates -- the
 * proof admits no allocation, and every read below is a load. Each worker also reads in no-write
 * mode (`kiln_c99_par_enter`, which is `kiln_c99_no_write_enter`), so the few paths that would
 * write while reading -- a string's ASCII answer, a key lookup -- compute without storing.
 *
 * Every read here answers exactly what the sequential read answers, or declines with 0: a getter
 * anywhere on the path, a proxy, an exotic object (`arguments`, an error, a namespace), a name an
 * `Object.prototype` member might answer, an element that would need a string allocated. A
 * decline is not an error. The worker sets the call's failure flag, and the call site then runs
 * the method's sequential lowering from the first element. That is sound because nothing the
 * pool did is observable: a tier-2 callback writes nothing, and a declined read ran no user code,
 * so the sequential run performs every effect -- the getter's included -- once, in order. */
typedef struct kiln_c99_par_env {
    /* Offset 0, as in tier 1: a worker that declines sets it, and every worker stops early. */
    int failed;
    /* A `parallelMap` over an array wrote something other than a number (a hole, a boolean):
     * the result is not a numeric array. */
    int mixed;
    /* The frozen realm's heap, which the workers read in no-write mode. */
    struct kiln_gc_heap *heap;
    /* The callback's captured values (bindings written once), read on the realm thread. */
    const kiln_val *caps;
    /* The property names the callback reads, as the unit interned them. */
    const kiln_string *const *keys;
    /* The string literals it compares with. */
    const kiln_val *strs;
    /* Tier 3 (parallel issue 08): the worker arenas of this call, one per thread that ran a chunk
     * (`kiln_c99_par_arena_claim`), opened on the realm thread before the pool starts
     * (`kiln_c99_par_arenas_open`) and joined to the heap after it returns
     * (`kiln_c99_par_arenas_join`). NULL for a site whose workers must not allocate -- a typed
     * array's, `parallelReduce`'s, a key comparator's -- and every allocation there declines. */
    struct kiln_c99_par_arena *arenas;
    uint32_t arena_capacity;
    uint32_t arena_count;
    /* The interned keys the callback defines (object literal members, `{ ...r, k: v }`), in
     * `HeapShape::defines` order, interned on the realm thread before the pool starts. */
    const kiln_string *const *defines;
    /* Tier 4 (parallel issue 09): the bodies of the functions the callback calls through a
     * closure value, in `HeapShape::bodies` order, so a clone checks per call that the closure
     * it found is the one the proof is about. */
    const kiln_body_fn *bodies;
} kiln_c99_par_env;

/* How deep a pool thread's clones call each other (tier 4) before the call declines and runs
 * sequentially -- where a recursion that deep is the sequential run's to finish, or overflow. */
#define KILN_C99_PAR_CALL_DEPTH 128u

/* A worker's own cache for one named read of the clone: the shape it last met there and the own
 * data slot the name has in it. It lives in the chunk worker's frame, so nothing shared is taught
 * (ADR 0071's caches are consulted by nothing here, and written by nothing here). */
typedef struct kiln_c99_par_site {
    const kiln_shape *shape;
    uint32_t slot;
    /* Tier 3: at a definition site, whether the value last defined was a number (the edge's
     * double bit), and the shape the definition moved `shape` to. */
    uint32_t number;
    const kiln_shape *to;
} kiln_c99_par_site;

/* A worker's arena (tier 3, ADR 0069): private chunks of the kind the nursery uses. Opaque here;
 * `heap.c` owns the layout. */
typedef struct kiln_c99_par_arena kiln_c99_par_arena;

/* Where `kiln_c99_par_enter` keeps what `kiln_c99_par_leave` restores (a no-write frame). */
typedef struct kiln_c99_par_frame {
    uint64_t words[4];
} kiln_c99_par_frame;

#ifdef KILN_SEPARATE_PRELUDE
extern void kiln_c99_par_enter(const kiln_c99_par_env *env, kiln_c99_par_frame *frame);
extern void kiln_c99_par_leave(kiln_c99_par_frame *frame);
extern int kiln_c99_par_get_slow(const kiln_c99_par_env *env, kiln_c99_par_site *site,
                                 kiln_val object, const kiln_string *ikey, kiln_val *out);
extern int kiln_c99_par_index_slow(const kiln_c99_par_env *env, kiln_val object, kiln_val key,
                                   kiln_val *out);
extern struct kiln_gc_heap *kiln_c99_par_prepare(void);
extern void kiln_c99_par_section(int frozen);
extern int kiln_c99_par_arenas_open(kiln_c99_par_env *env, size_t threads);
extern int kiln_c99_par_arenas_join(kiln_c99_par_env *env, int keep);
extern kiln_c99_par_arena *kiln_c99_par_arena_claim(kiln_c99_par_env *env);
extern const kiln_string *kiln_c99_par_intern(const kiln_string *key);
extern int kiln_c99_par_object(kiln_c99_par_arena *arena, kiln_val *out);
extern int kiln_c99_par_define(kiln_c99_par_arena *arena, kiln_c99_par_site *site, kiln_val target,
                               const kiln_string *ikey, kiln_val value);
extern int kiln_c99_par_define_computed(kiln_c99_par_arena *arena, kiln_val target, kiln_val key,
                                        kiln_val value);
extern int kiln_c99_par_spread(kiln_c99_par_arena *arena, kiln_val target, kiln_val source);
extern int kiln_c99_par_array(kiln_c99_par_arena *arena, const kiln_val *elements, size_t count,
                              kiln_val *out);
extern int kiln_c99_par_to_string(kiln_c99_par_arena *arena, kiln_val value, kiln_val *out);
extern int kiln_c99_par_concat(kiln_c99_par_arena *arena, kiln_val a, kiln_val b, kiln_val *out);
extern int kiln_c99_par_flatten(kiln_val mapped, size_t count, kiln_val *out);
/* Tier 4 (parallel issue 09): the pure built-ins a pool thread runs (`boxed.c`). */
extern int kiln_c99_par_plain_array(kiln_val value, const kiln_array **out);
extern int kiln_c99_par_array_sized(kiln_c99_par_arena *arena, size_t count, kiln_val *out,
                                    kiln_array **array);
extern void kiln_c99_par_array_seal(kiln_array *array);
extern int kiln_c99_par_f_Number(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_Boolean(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_String(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_parseInt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_parseFloat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_Number_isInteger(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_Number_isSafeInteger(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_Number_isNaN(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_Number_isFinite(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_Number_parseInt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_Number_parseFloat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_f_JSON_stringify(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_trim(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_trimStart(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_trimEnd(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_toUpperCase(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_toLowerCase(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_at(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_charAt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_codePointAt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_substring(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_slice(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_indexOf(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_includes(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_lastIndexOf(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_startsWith(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_endsWith(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_padStart(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_padEnd(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_repeat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_split(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_toString(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_toFixed(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
extern int kiln_c99_par_m_join(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
#else
static void kiln_c99_par_enter(const kiln_c99_par_env *env, kiln_c99_par_frame *frame);
static void kiln_c99_par_leave(kiln_c99_par_frame *frame);
static int kiln_c99_par_get_slow(const kiln_c99_par_env *env, kiln_c99_par_site *site,
                                 kiln_val object, const kiln_string *ikey, kiln_val *out);
static int kiln_c99_par_index_slow(const kiln_c99_par_env *env, kiln_val object, kiln_val key,
                                   kiln_val *out);
static struct kiln_gc_heap *kiln_c99_par_prepare(void);
static void kiln_c99_par_section(int frozen);
static int kiln_c99_par_arenas_open(kiln_c99_par_env *env, size_t threads);
static int kiln_c99_par_arenas_join(kiln_c99_par_env *env, int keep);
static kiln_c99_par_arena *kiln_c99_par_arena_claim(kiln_c99_par_env *env);
static const kiln_string *kiln_c99_par_intern(const kiln_string *key);
static int kiln_c99_par_object(kiln_c99_par_arena *arena, kiln_val *out);
static int kiln_c99_par_define(kiln_c99_par_arena *arena, kiln_c99_par_site *site, kiln_val target,
                               const kiln_string *ikey, kiln_val value);
static int kiln_c99_par_define_computed(kiln_c99_par_arena *arena, kiln_val target, kiln_val key,
                                        kiln_val value);
static int kiln_c99_par_spread(kiln_c99_par_arena *arena, kiln_val target, kiln_val source);
static int kiln_c99_par_array(kiln_c99_par_arena *arena, const kiln_val *elements, size_t count,
                              kiln_val *out);
static int kiln_c99_par_to_string(kiln_c99_par_arena *arena, kiln_val value, kiln_val *out);
static int kiln_c99_par_concat(kiln_c99_par_arena *arena, kiln_val a, kiln_val b, kiln_val *out);
static int kiln_c99_par_flatten(kiln_val mapped, size_t count, kiln_val *out);
static int kiln_c99_par_plain_array(kiln_val value, const kiln_array **out);
static int kiln_c99_par_array_sized(kiln_c99_par_arena *arena, size_t count, kiln_val *out,
                                    kiln_array **array);
static void kiln_c99_par_array_seal(kiln_array *array);
static int kiln_c99_par_f_Number(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_Boolean(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_String(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_parseInt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_parseFloat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_Number_isInteger(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_Number_isSafeInteger(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_Number_isNaN(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_Number_isFinite(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_Number_parseInt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_Number_parseFloat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_f_JSON_stringify(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, size_t argc,
           const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_trim(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_trimStart(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_trimEnd(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_toUpperCase(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_toLowerCase(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_at(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_charAt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_codePointAt(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_substring(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_slice(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_indexOf(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_includes(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_lastIndexOf(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_startsWith(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_endsWith(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_padStart(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_padEnd(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_repeat(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_split(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_toString(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_toFixed(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
static int kiln_c99_par_m_join(const kiln_c99_par_env *env, kiln_c99_par_arena *arena, kiln_val self,
           size_t argc, const kiln_val *args, kiln_val *out);
#endif



/* `ToNumber` of a value whose conversion runs no code and allocates nothing: a number, undefined
 * (NaN), null and false (0), true (1). A string would parse and an object would call `valueOf`,
 * so both decline. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_number(kiln_val v, double *out) {
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(v))) {
        memcpy(out, &v, sizeof *out);
        return 1;
    }
    if (v == KILN_VAL_UNDEFINED) {
        *out = NAN;
        return 1;
    }
    if (v == KILN_VAL_NULL || v == KILN_VAL_FALSE) {
        *out = 0.0;
        return 1;
    }
    if (v == KILN_VAL_TRUE) {
        *out = 1.0;
        return 1;
    }
    return 0;
}

/* `object[ikey]` for a name that is not an index: an ordinary object's own data slot, from the
 * worker's cache when the shape is the one it last met there, else the whole walk. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_get(const kiln_c99_par_env *env,
                                                      kiln_c99_par_site *site, kiln_val object,
                                                      const kiln_string *ikey, kiln_val *out) {
    if (KILN_LIKELY(KILN_VAL_IS_OBJECT(object))) {
        const kiln_object *obj = (const kiln_object *)KILN_VAL_PTR(object);
        if (KILN_LIKELY(site->shape != NULL &&
                        kiln_c99_gc_payload_kind(obj) == KILN_GC_OBJECT &&
                        obj->shape == site->shape && site->slot < obj->slot_capacity &&
                        !obj->arguments_brand && !obj->error_brand && !obj->module_namespace)) {
            *out = obj->slots[site->slot];
            return 1;
        }
    }
    return kiln_c99_par_get_slow(env, site, object, ikey, out);
}

/* `object.length`: an array's count, a view's length, a string's code units; an ordinary
 * object's own or inherited `length` property through the walk. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_length(const kiln_c99_par_env *env,
                                                         kiln_c99_par_site *site, kiln_val object,
                                                         const kiln_string *ikey, kiln_val *out) {
    if ((object & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX) {
        const void *payload = KILN_VAL_PTR(object);
        uint32_t kind = kiln_c99_gc_payload_kind(payload);
        if (KILN_LIKELY(kind == KILN_GC_ARRAY)) {
            *out = kiln_c99_val_from_double((double)((const kiln_array *)payload)->count);
            return 1;
        }
        if (kind == KILN_GC_TYPED_ARRAY && !((const kiln_typed_array *)payload)->buffer->detached) {
            *out = kiln_c99_val_from_double((double)((const kiln_typed_array *)payload)->length);
            return 1;
        }
        return 0;
    }
    if (KILN_VAL_IS_STRING(object)) {
        *out = kiln_c99_val_from_double(
            (double)((const kiln_string *)KILN_VAL_PTR(object))->unit_len);
        return 1;
    }
    return kiln_c99_par_get(env, site, object, ikey, out);
}

/* `object[key]`: an element of an array (a hole declines to the slow path, which answers it) or
 * of a Number-kind view, inline; everything else -- strings, objects by name -- out of line. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_index(const kiln_c99_par_env *env,
                                                        kiln_val object, kiln_val key,
                                                        kiln_val *out) {
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(key) &&
                    (object & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX)) {
        const void *payload = KILN_VAL_PTR(object);
        uint32_t kind = kiln_c99_gc_payload_kind(payload);
        double d;
        memcpy(&d, &key, sizeof d);
        if (KILN_LIKELY(kind == KILN_GC_ARRAY)) {
            const kiln_array *arr = (const kiln_array *)payload;
            if (d >= 0.0 && d < (double)arr->count) {
                size_t i = (size_t)d;
                if ((double)i == d && arr->elements[i] != KILN_VAL_HOLE) {
                    *out = arr->elements[i];
                    return 1;
                }
            }
        } else if (kind == KILN_GC_TYPED_ARRAY) {
            const kiln_typed_array *view = (const kiln_typed_array *)payload;
            if (d >= 0.0 && d < (double)view->length && view->kind <= KILN_TYPED_FLOAT64 &&
                !view->buffer->detached) {
                size_t i = (size_t)d;
                if ((double)i == d) {
                    *out = kiln_c99_val_from_double(kiln_typed_load(view, i));
                    return 1;
                }
            }
        }
    }
    return kiln_c99_par_index_slow(env, object, key, out);
}

/* `a < b` (9), `<=` (10), `>` (11), `>=` (12) where neither side runs code to convert: two
 * strings compare by code units, anything `kiln_c99_par_number` converts compares as numbers
 * (NaN answering false). 1 or 0, or -1 to decline. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_relational(int op, kiln_val a, kiln_val b) {
    double x, y;
    if (KILN_UNLIKELY(KILN_VAL_IS_STRING(a) && KILN_VAL_IS_STRING(b))) {
        int c = kiln_string_compare_units((const kiln_string *)KILN_VAL_PTR(a),
                                          (const kiln_string *)KILN_VAL_PTR(b));
        switch (op) {
        case 9: return c < 0;
        case 10: return c <= 0;
        case 11: return c > 0;
        default: return c >= 0;
        }
    }
    if (!kiln_c99_par_number(a, &x) || !kiln_c99_par_number(b, &y)) return -1;
    switch (op) {
    case 9: return x < y;
    case 10: return x <= y;
    case 11: return x > y;
    default: return x >= y;
    }
}

/* `a === b`: numbers by value (NaN unequal, the zeros equal), strings by code units, every other
 * value by identity; a BigInt, which compares by value, declines. 1 or 0, or -1. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_strict_equal(kiln_val a, kiln_val b) {
    if (KILN_VAL_IS_NUMBER(a) || KILN_VAL_IS_NUMBER(b)) {
        double x, y;
        if (!(KILN_VAL_IS_NUMBER(a) && KILN_VAL_IS_NUMBER(b))) return 0;
        memcpy(&x, &a, sizeof x);
        memcpy(&y, &b, sizeof y);
        return x == y;
    }
    if (KILN_VAL_IS_STRING(a) && KILN_VAL_IS_STRING(b)) {
        return a == b || kiln_string_equal((const kiln_string *)KILN_VAL_PTR(a),
                                           (const kiln_string *)KILN_VAL_PTR(b));
    }
    if ((a & UINT64_C(0xFFFF800000000000)) == KILN_TAG_BIGINT_PREFIX ||
        (b & UINT64_C(0xFFFF800000000000)) == KILN_TAG_BIGINT_PREFIX) {
        return -1;
    }
    return a == b;
}

/* `a == b` where no conversion runs code: null and undefined equal each other and nothing else;
 * numbers and booleans compare as numbers; two strings by code units; two objects (or two
 * symbols) by identity. A string against a number parses and an object against a primitive calls
 * `valueOf`, so those, and BigInts, decline. 1 or 0, or -1. */
static inline int kiln_c99_par_loose_equal(kiln_val a, kiln_val b) {
    double x, y;
    int a_prim, b_prim;
    if (KILN_VAL_IS_NULLISH(a) || KILN_VAL_IS_NULLISH(b)) {
        return KILN_VAL_IS_NULLISH(a) && KILN_VAL_IS_NULLISH(b);
    }
    if ((KILN_VAL_IS_NUMBER(a) || KILN_VAL_IS_BOOLEAN(a)) &&
        (KILN_VAL_IS_NUMBER(b) || KILN_VAL_IS_BOOLEAN(b))) {
        (void)kiln_c99_par_number(a, &x);
        (void)kiln_c99_par_number(b, &y);
        return x == y;
    }
    if (KILN_VAL_IS_STRING(a) && KILN_VAL_IS_STRING(b)) return kiln_c99_par_strict_equal(a, b);
    a_prim = KILN_VAL_IS_NUMBER(a) || KILN_VAL_IS_BOOLEAN(a) || KILN_VAL_IS_STRING(a) ||
             (a & UINT64_C(0xFFFF800000000000)) == KILN_TAG_BIGINT_PREFIX;
    b_prim = KILN_VAL_IS_NUMBER(b) || KILN_VAL_IS_BOOLEAN(b) || KILN_VAL_IS_STRING(b) ||
             (b & UINT64_C(0xFFFF800000000000)) == KILN_TAG_BIGINT_PREFIX;
    if (a_prim || b_prim || KILN_VAL_IS_SYMBOL(a) != KILN_VAL_IS_SYMBOL(b)) return -1;
    return a == b;
}

/* `s.charCodeAt(pos)` for a string `s` (anything else declines: an object could own the name):
 * the code unit at `ToIntegerOrInfinity(pos)`, or NaN past either end. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_char_code_at(kiln_val s, kiln_val pos,
                                                               kiln_val *out) {
    const kiln_string *str;
    double d;
    if (!KILN_VAL_IS_STRING(s) || !kiln_c99_par_number(pos, &d)) return 0;
    str = (const kiln_string *)KILN_VAL_PTR(s);
    d = isnan(d) ? 0.0 : trunc(d);
    if (d < 0.0 || d >= (double)str->unit_len) {
        *out = KILN_QNAN_BASE;
        return 1;
    }
    *out = kiln_c99_val_from_double((double)kiln_string_unit_at(str, (size_t)d));
    return 1;
}

/* `a + b` on a pool thread (tier 3): two numbers (or what converts to one silently) add; when
 * either side is a string, the other is converted as `ToString` converts a primitive and the two
 * are joined in the worker's arena (`kiln_c99_par_concat`); an object, a symbol or a BigInt
 * declines -- converting one could run code. With no arena (a site whose workers must not
 * allocate), a string declines. */
static KILN_ALWAYS_INLINE inline int kiln_c99_par_add(kiln_c99_par_arena *arena, kiln_val a,
                                                      kiln_val b, kiln_val *out) {
    double x, y;
    if (KILN_LIKELY(KILN_VAL_IS_NUMBER(a) && KILN_VAL_IS_NUMBER(b))) {
        memcpy(&x, &a, sizeof x);
        memcpy(&y, &b, sizeof y);
        *out = kiln_c99_val_from_double(x + y);
        return 1;
    }
    if (KILN_VAL_IS_STRING(a) || KILN_VAL_IS_STRING(b)) {
        return arena != NULL && kiln_c99_par_concat(arena, a, b, out);
    }
    if (!kiln_c99_par_number(a, &x) || !kiln_c99_par_number(b, &y)) return 0;
    *out = kiln_c99_val_from_double(x + y);
    return 1;
}

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
    kiln_value properties;
    /* The GetPrototypeFromConstructor result fixed when this iterator is created. */
    kiln_value prototype;
    kiln_value function;
    kiln_value receiver;
    /* An async function is compiled as a generator -- `await` suspends exactly as `yield` does --
     * and this is the promise its eventual return value settles. Undefined for a plain one. */
    kiln_value promise;
    int is_async;
    kiln_val *locals;
    kiln_val *stack;
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
    /* Whether this entry is the parameter binding rather than the body. Set only for the call
     * that builds the generator, and read at the seam between the two halves: the emitted body
     * is one flat instruction sequence, so the only thing that can stop it there is a test. */
    int binding;
    int started;
    int done;
    /* Whether the body is on the C stack right now.
     *
     * A generator has one frame, and `next` resumes *that* frame -- so a `next` from inside the
     * body would resume a frame that is already running, overwriting the locals and operand stack
     * the running body is standing on. The language answers "Generator is already running"
     * instead, and this is what lets the answer be given: before it, the re-entrant call clobbered
     * the frame and the process ended with an exit code and no words. */
    int running;
    kiln_value *queue_promises;
    kiln_value *queue_sent;
    size_t queue_count;
    size_t queue_capacity;
    int queue_executing;
    kiln_value delegate;
    size_t delegate_exit;
} kiln_generator;

/* The parts a civil date is made of. The realm has one zone and it is UTC (ADR 0015), so there
 * is no local variant of any of these to keep in step. */
/* Timestamp stays first so existing Date arithmetic keeps its scalar load. Named/private
 * properties and subclass prototypes are ordinary object state owned by the Date allocation. */
typedef struct kiln_date {
    double time;
    kiln_value properties;
} kiln_date;

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
    uint32_t *hash_table;
    size_t hash_capacity;
    int is_map;
    /* Weak collections use ephemeron tracing: a value is marked only once its key is live. */
    int is_weak;
    struct kiln_collection *weak_previous;
    struct kiln_collection *weak_next;
    struct kiln_gc_heap *weak_owner;
    /* Where `class Registry extends Map {}` puts the prototype its instances inherit from, and
     * where a write that is not an entry goes. Undefined until something needs one, so an
     * ordinary Map costs nothing for it. */
    kiln_value properties;
} kiln_collection;

/* A collection's tagged backing store is not enough to answer a method call. A subclass keeps
 * its ordinary prototype (and any own properties) in `properties`, and a program can replace a
 * member on Map.prototype or Set.prototype. Emitted collection fast paths may use the backing
 * table only while lookup is still the untouched built-in method. */
static inline int kiln_c99_collection_intrinsic_method_allowed(const kiln_collection *collection,
                                                               const char *member,
                                                               size_t member_len) {
    uint32_t prototype;
    if (collection == NULL || member == NULL || collection->is_weak ||
        collection->properties.tag != KILN_VALUE_UNDEFINED) return 0;
    if ((strcmp(member, "get") == 0 || strcmp(member, "set") == 0) && !collection->is_map)
        return 0;
    if (strcmp(member, "add") == 0 && collection->is_map) return 0;
    prototype = collection->is_map ? KILN_C99_BUILTIN_MAP_PROTOTYPE
                                   : KILN_C99_BUILTIN_SET_PROTOTYPE;
    return kiln_c99_builtin_member_unmodified_n(prototype, member, member_len);
}

static inline int kiln_c99_collection_key_matches_method(const kiln_string *key,
                                                         const char *method) {
    size_t length;
    if (key == NULL || method == NULL) return 0;
    length = strlen(method);
    if (key->unit_len != length ||
        (key->flags & (KILN_STRING_SYMBOL_KEY | KILN_STRING_PRIVATE_KEY)) !=
            (kiln_c99_fixed_key_flags(method, length) &
             (KILN_STRING_SYMBOL_KEY | KILN_STRING_PRIVATE_KEY))) return 0;
    for (size_t index = 0; index < length; ++index) {
        if (kiln_string_unit_at(key, (uint32_t)index) != (unsigned char)method[index]) return 0;
    }
    return 1;
}

static inline int kiln_c99_collection_cached_method_allowed(const kiln_collection *collection,
                                                            const kiln_string *key,
                                                            uint64_t method_code,
                                                            const char *method) {
    if (collection == NULL || collection->properties.tag != KILN_VALUE_UNDEFINED ||
        !kiln_c99_collection_key_matches_method(key, method)) return 0;
    switch (method_code) {
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
    case KILN_C99_METHOD_COLLECTION_SELF:
        return kiln_c99_collection_intrinsic_method_allowed(collection, method, strlen(method));
    default:
        /* Object.prototype methods are cacheable only while the collection has no own or
         * subclass overlay. Their prototype mutations are covered by the tag-pristine guard. */
        return 1;
    }
}

/* A promise: a value that arrives once, later, and everything waiting for it.
 *
 * `state` is pending, fulfilled or rejected, and only the first settlement counts -- a promise is
 * handed to the program as two functions and the program may call either twice. */
typedef struct kiln_reaction {
    kiln_value on_fulfilled;
    kiln_value on_rejected;
    /* The promise `then` returned, settled with whatever the handler produces. */
    kiln_value derived;
    /* Set when both handlers are the suspended frame an `await` left, to be stepped rather than
     * called: a frame is not callable, and a reaction merely carrying one would otherwise read
     * as a `then` with no handler. */
    uint8_t continuation: 2;
} kiln_reaction;

typedef struct kiln_promise {
    int state;
    /* [[PromiseIsHandled]], and where a rejection nothing handled stands in node's tracking
     * (`KILN_C99_REJECTION_*`, promises.c). Both fit in the padding before `value`. */
    unsigned char handled;
    unsigned char rejection;
    kiln_value value;
    /* `inline_reaction` while at most one reaction waits -- the case for nearly every promise,
     * which one `await` or one `then` watches -- and a malloc'd array only past that. A pending
     * promise used to cost a 224-byte array the moment anything waited on it; a hundred thousand
     * sleeping tasks each held one. `kiln_c99_promise_add_reaction` is the one place that grows
     * it, and only a pointer other than `inline_reaction` is ever freed. */
    kiln_reaction *reactions;
    size_t count;
    size_t capacity;
    kiln_value properties;
    kiln_reaction inline_reaction;
} kiln_promise;

/* `kiln_promise.rejection`: never rejected unhandled (or handled in time); rejected with nothing
 * attached and waiting for the end of the checkpoint; handed to node's tracking, so a handler
 * attached now is `rejectionHandled`. */
#define KILN_C99_REJECTION_NONE 0
#define KILN_C99_REJECTION_WAITING 1
#define KILN_C99_REJECTION_REPORTED 2

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
    int (*matcher)(const uint16_t *units, size_t count, size_t start,
                   int64_t *ranges, size_t capacity, size_t *groups,
                   int *matched);
    int (*matcher_ascii)(const uint8_t *bytes, size_t count, size_t start,
                         int64_t *ranges, size_t capacity, size_t *groups,
                         int *matched);
    /* These are canonical string headers. They must outlive the regexp object: generated
     * literals use static headers and runtime-created patterns use GC-owned string headers. */
    const kiln_string *source;
    const kiln_string *flags;
    int global;
    int sticky;
    double last_index;
    int last_index_writable;
    const char *cached_subject_ptr;
    size_t cached_subject_len;
    int cached_is_ascii;
    size_t cached_group_count;
    /* Whether the pattern has named capture groups: 0 not yet asked, 1 none, 2 some. Asked of
     * the provider once, after the first match, so an unnamed pattern pays one call. */
    int named_groups;
    int64_t cached_ranges[64];
    kiln_val cached_elements[32];
    /* User-defined own properties (apart from the intrinsic, non-configurable lastIndex). */
    kiln_value properties;
} kiln_regexp;

/* A symbol: a property key with an identity of its own.
 *
 * `key` is the reserved string the symbol names a property under, so the object model stays
 * string-keyed and nothing else has to learn about symbols. A well-known symbol gets a fixed key,
 * so two mentions of `Symbol.iterator` reach the same property; a unique one gets a serial, so two
 * symbols with the same description do not collide and a program cannot spell either by writing a
 * string. The description is for display and is never part of the key. */
typedef struct kiln_symbol {
    const kiln_string *key;
    const char *description;
    size_t description_len;
    /* Runtime descriptions are canonical strings so UTF-16 units, including lone surrogates,
     * survive Symbol(description) and reflection. The byte pair remains only for immortal
     * built-in symbols whose descriptions are compiled ASCII. */
    kiln_value description_value;
    /* Non-zero only for a symbol from the cross-realm registry. It lived in the value's payload
     * word until that word was shared with the pointer; a registered symbol is the one value that
     * wanted both, and the id belongs with the symbol rather than beside it. */
    uint64_t registry_id;
} kiln_symbol;

typedef struct kiln_bound {
    kiln_value target;
    kiln_value receiver;
    /* `f.bind(o, 1)` fixes the 1 as well as the `o`: the bound arguments come first and the
     * call's own follow them. Owned here, because the call that supplies the rest happens
     * arbitrarily later. */
    kiln_value *arguments;
    size_t count;
    /* Bound functions are ordinary callable objects: libraries attach metadata to wrappers
     * returned by `bind`. Keep that own store on the value, not on its target, so two bindings
     * of one function do not share statics. */
    kiln_value properties;
} kiln_bound;

typedef int (*kiln_commonjs_require_fn)(void *context, kiln_host *host, kiln_value specifier,
                                        kiln_value *result, kiln_value *thrown);
typedef int (*kiln_commonjs_resolve_fn)(void *context, kiln_host *host, kiln_value specifier,
                                        kiln_value *result, kiln_value *thrown);
typedef struct kiln_commonjs_require {
    kiln_commonjs_require_fn call;
    kiln_commonjs_resolve_fn resolve;
    void *context;
    /* CommonJS require is a function object with a module-local own-property store. */
    kiln_value properties;
    /* The launcher's registered module-root inventory, with its stable realm-owned anchor.
     * Opaque contexts without this authority remain unproven for graph transfer. */
    kiln_gc_visit_roots_fn visit_context;
    void *root_context;
} kiln_commonjs_require;

/* One job of the promise lane: run `handler` against `argument` and settle `derived` with what
 * it produced.  A handler that is not callable passes the value through unchanged, which is what
 * `p.then(undefined, f)` and `p.catch(f)` rely on.  These are values in an array the prelude
 * owns (`kiln_gc_heap.jobs`), not cells: see the head of `promises.c`. */
typedef struct kiln_microtask {
    kiln_value handler;
    kiln_value argument;
    kiln_value derived;
    uint8_t rejected: 1;
    uint8_t continuation: 2;
} kiln_microtask;

/* One process.nextTick callback. The Realm Runtime owns lane ordering; this payload contains
 * only the language values its opaque work callback understands. */
typedef struct kiln_next_tick {
    kiln_value callback;
    kiln_value *args;
    size_t argc;
    /* Still counted in the heap's `next_ticks_queued`: cleared when the tick starts to run (a
     * running tick is no longer queued), or when it is disposed of unrun. */
    int counted;
} kiln_next_tick;

/* One generated callback payload. The Realm Runtime owns its timer identity, deadline, sequence,
 * repeat policy and cancellation; this allocation contains only the language state its opaque
 * work callback understands. */
typedef struct kiln_timer {
    kiln_value callback;
    kiln_value *args;
    size_t argc;
    /* The handle's own properties, an ordinary object whose prototype is `Timeout.prototype`:
     * undefined until a program touches the handle (`kiln_c99_timer_materialize`). */
    kiln_value properties;
    /* The delay it was armed with, in milliseconds as the program gave it, which is what a
     * `refresh()` of a timeout that already ran arms again. */
    double delay;
    /* The realm's identity for it; the same one for the life of the record, a refresh included. */
    uint64_t id;
    /* How many entries in the realm's queue name this record: one, and two for the moment a
     * timeout re-arms itself from inside its own callback. */
    uint32_t owners;
    uint8_t kind;
    /* The timeout or immediate ran and has not been armed since. */
    uint8_t ran;
    /* `unref()`, which survives the timer's end because `hasRef` still answers. */
    uint8_t unreferenced;
    /* Cleared: its callback and arguments are dropped and `refresh()` does nothing. */
    uint8_t cleared;
} kiln_timer;
enum { KILN_C99_TIMER_TIMEOUT = 0, KILN_C99_TIMER_INTERVAL = 1, KILN_C99_TIMER_IMMEDIATE = 2,
       KILN_C99_TIMER_INTERNAL = 3 };
#define kiln_c99_timer_of(value) ((kiln_timer *)(uintptr_t)(value).bits)
typedef struct kiln_node_server kiln_node_server;
typedef struct kiln_node_socket kiln_node_socket;

struct kiln_node_server {
    kiln_host *host;
    kiln_value object;
    kiln_value connection_listener;
    uint64_t listener;
    uint64_t listen_request;
    uint64_t accept_request;
    char *address;
    size_t address_len;
    uint32_t port;
    uint64_t timeout_milliseconds;
    int protocol_http;
    /* Raw net servers default to half-close disabled; HTTP sockets expose true independently
     * of the HTTP parser's `httpAllowHalfOpen` option. */
    int allow_half_open;
    /* A fetch-style (`serve`) server: requests arrive whole and reach the listener as strings
     * and bytes, with no IncomingMessage or ServerResponse built (`kiln_c99_node_fetch_request`). */
    int fetch_mode;
    int closing;
    int closed;
    kiln_node_socket *sockets;
};

struct kiln_node_socket {
    kiln_host *host;
    kiln_value object;
    kiln_value readable_state;
    kiln_node_socket *next;
    uint64_t resource;
    uint64_t connect_request;
    uint64_t read_request;
    uint64_t write_request;
    size_t active_write_len;
    uint64_t shutdown_request;
    uint64_t close_request;
    uint64_t upgrade_request_id;
    uint64_t shutdown_after_upgrade_fence;
    char *pending_write_bytes;
    size_t pending_write_offset;
    size_t pending_write_len;
    size_t pending_write_capacity;
    /* Writes made between cork/uncork are retained as one ordered byte sequence. */
    char *corked_write_bytes;
    size_t corked_write_len;
    size_t corked_write_capacity;
    unsigned int cork_depth;
    /* FIFO of [byte length, callback batch] entries for writes not yet submitted. */
    kiln_value write_queue;
    size_t write_queue_head;
    /* The currently submitted batch remains rooted here until its host result arrives. */
    kiln_value active_write_callbacks;
    kiln_value write_callbacks;
    kiln_value end_callbacks;
    /* Upgrade callback batches are keyed by HTTP engine append-completion fences. */
    kiln_value upgrade_write_callbacks;
    int http_upgrade_pending;
    int http_upgraded;
    int http_client_owned;
    int connecting;
    int pending_http_attach;
    int pending_read_after_write;
    int read_paused;
    size_t read_demand;
    int read_demand_active;
    int paused_read_eof;
    int read_end_scheduled;
    uint8_t *paused_read_bytes;
    size_t paused_read_len;
    size_t paused_read_capacity;
    kiln_node_server *server;
    struct kiln_node_client_request *client_requests;
    uint64_t next_client_request_id;
    struct kiln_node_response *responses;
    int close_after_write;
    int close_after_responses;
    int destroy_after_write;
    int shutdown_after_write;
    int utf8;
    int read_closed;
    int write_closed;
    int closing;
    int closed;
};

typedef struct kiln_node_client_request {
    kiln_host *host;
    kiln_value object;
    kiln_value response;
    kiln_node_socket *socket;
    char *host_name;
    size_t host_name_len;
    char *path;
    size_t path_len;
    char *method;
    size_t method_len;
    char *headers;
    size_t headers_len;
    char *pending_body;
    size_t pending_body_len;
    size_t pending_body_capacity;
    uint16_t port;
    uint64_t connect_request;
    uint64_t request_id;
    int ended;
    int sent;
    int response_started;
    int response_ended;
    int response_released;
    int response_reusable;
    int socket_published;
    int utf8;
    struct kiln_node_client_request *next;
} kiln_node_client_request;

static int kiln_c99_node_client_try_start(kiln_node_client_request *request);
static int kiln_c99_node_socket_object(kiln_node_socket *socket, kiln_value *thrown);
static int kiln_c99_node_socket_ready_state(kiln_node_socket *socket,
                                            const char *state, size_t state_len);

static kiln_node_client_request *kiln_c99_node_client_for_id(kiln_node_socket *socket,
                                                              uint64_t id) {
    kiln_node_client_request *request;
    if (socket == NULL || id == 0) return NULL;
    for (request = socket->client_requests; request != NULL; request = request->next) {
        if (request->request_id == id) return request;
    }
    return NULL;
}

typedef struct kiln_node_response {
    kiln_node_socket *socket;
    uint64_t request_id;
    kiln_value request;
    kiln_value object;
    /* The request's headers as the runtime flattened them: NUL-separated name/value pairs.
     * While the head event is being dispatched this borrows the event's buffer
     * (`request_headers_owned == 0`); the dispatch copies it before returning if `req.headers`
     * was not read, because the runtime reuses that buffer for the next request on the
     * connection. A malloc'd inner buffer of this block once owned, released by its finaliser
     * (ADR 0041) or as soon as `req.headers` is read and the object it describes exists instead. */
    char *request_headers;
    size_t request_headers_len;
    int request_headers_owned;
    int request_utf8;
    uint16_t status;
    int started;
    int ended;
    struct kiln_node_response *next;
    /* The response head the program is building, already in the private ABI's flattened form
     * (`name NUL value NUL`, UTF-8, names as the program spelt them): `setHeader`, `writeHead`
     * and friends edit it in place and the first `write`/`end` hands it to the runtime as is.
     * A malloc'd inner buffer of this block, released by its finaliser. */
    char *out_headers;
    size_t out_headers_len;
    size_t out_headers_capacity;
    /* Step B.3: Specialized URL construction and request target tracking */
    kiln_value url_object;
    kiln_value search_params_object;
    int url_used;
    const char *target_ptr;
    size_t target_len;
    kiln_value pathname_val;
    size_t pathname_cap;
    kiln_value query_val;
    size_t query_cap;
    kiln_value param_val;
    size_t param_cap;
} kiln_node_response;

static void kiln_c99_node_unlink_response(kiln_node_response *response) {
    kiln_node_response **cursor;
    if (response == NULL || response->socket == NULL) return;
    cursor = &response->socket->responses;
    while (*cursor != NULL) {
        if (*cursor == response) {
            *cursor = response->next;
            response->next = NULL;
            return;
        }
        cursor = &(*cursor)->next;
    }
}

/* A server keeps its accepted sockets in a list so that `server.close()` can drain them and so
 * that the last one to close is what emits the server's own `close`. A socket that has closed
 * answers neither question, and leaving it linked would make the server retain every connection
 * it ever accepted -- one live block per request, which is exactly the growth ADR 0041 exists to
 * remove. Unlinking is what "all remaining sockets are open" means, so the walks above read the
 * same either way. */
static void kiln_c99_node_unlink_socket(kiln_node_socket *socket) {
    kiln_node_socket **cursor;
    if (socket == NULL || socket->server == NULL) return;
    cursor = &socket->server->sockets;
    while (*cursor != NULL) {
        if (*cursor == socket) {
            *cursor = socket->next;
            socket->next = NULL;
            return;
        }
        cursor = &(*cursor)->next;
    }
}

static int kiln_c99_node_has_unended_response(const kiln_node_socket *socket) {
    const kiln_node_response *response;
    if (socket == NULL) return 0;
    for (response = socket->responses; response != NULL; response = response->next) {
        if (!response->ended) return 1;
    }
    return 0;
}

/* A response head the script already framed. Node leaves a script-supplied Content-Length or
 * Transfer-Encoding alone; header names arrive from setHeader lowercased and from writeHead as
 * written, so the comparison is case-insensitive. */
static int kiln_c99_node_response_is_framed(const kiln_object *headers) {
    static const char *const framing[2] = {"content-length", "transfer-encoding"};
    size_t index;
    size_t which;
    if (headers == NULL) return 0;
    if (headers->shape != NULL) {
        for (index = 0; index < headers->shape->property_count; ++index) {
            const kiln_string *key = headers->shape->keys[index];
            size_t len = key == NULL ? 0 : key->unit_len;
            for (which = 0; which < 2; ++which) {
                size_t flen = strlen(framing[which]);
                size_t byte;
                if (len != flen) continue;
                for (byte = 0; byte < flen; ++byte) {
                    if (kiln_string_unit_at(key, byte) > 0x7f ||
                        (char)tolower((unsigned char)kiln_string_unit_at(key, byte)) != framing[which][byte])
                        break;
                }
                if (byte == flen) return 1;
            }
        }
    }
    for (index = 0; index < headers->count; ++index) {
        const kiln_property *property = &headers->properties[index];
        for (which = 0; which < 2; ++which) {
            size_t len = strlen(framing[which]);
            size_t byte;
            if (property->key->unit_len != len) continue;
            for (byte = 0; byte < len; ++byte) {
                if (kiln_string_unit_at(property->key, byte) > 0x7f ||
                    (char)tolower((unsigned char)kiln_string_unit_at(property->key, byte)) != framing[which][byte])
                    break;
            }
            if (byte == len) return 1;
        }
    }
    return 0;
}

typedef struct kiln_node_completion {
    uint32_t kind;
    kiln_node_server *server;
    kiln_node_socket *socket;
    kiln_node_client_request *client_request;
    kiln_value write_callbacks;
} kiln_node_completion;

/* One Node fs/promises request. The Promise and conversion mode stay in generated C; the Realm
 * owns this allocation from submission through completion/cancellation and invokes the adapter
 * only on the realm task. */
typedef struct kiln_node_fs_completion {
    kiln_value promise;
    uint32_t operation;
    int utf8;
} kiln_node_fs_completion;

typedef struct kiln_node_process_completion {
    kiln_value promise;
    kiln_value result;
    kiln_value stdout_value;
    kiln_value stderr_value;
    uint8_t port_callback;
} kiln_node_process_completion;

/* A cursor over JSON source, moved forward as text is consumed. */
/* One entry of the tape `kiln_json_parse` answers with -- the same struct as `kiln_json::Node` in
 * the provider archive. `len` is a string's byte length or a container's child count; `value` is
 * a string's offset into the decoded text or the bits of a number's double. */
typedef struct kiln_json_node {
    uint32_t tag;
    uint32_t len;
    uint64_t value;
} kiln_json_node;

#define KILN_JSON_NULL 0
#define KILN_JSON_FALSE 1
#define KILN_JSON_TRUE 2
#define KILN_JSON_NUMBER 3
#define KILN_JSON_STRING 4
#define KILN_JSON_ARRAY 5
#define KILN_JSON_OBJECT 6

/* A growable byte buffer, for the operations that build a string rather than slice one. */
typedef struct kiln_text {
    char *bytes;
    size_t len;
    size_t capacity;
    /* Set while `bytes` is a buffer the builder does not own -- a caller's stack array. The
     * first growth past it moves into memory of the builder's own; `kiln_c99_text_value` copies
     * from it without freeing it. Only a builder that started that way ever has it set. */
    int borrowed;
} kiln_text;

static int kiln_c99_text_append_grow(kiln_text *text, const char *bytes, size_t len);

/* Explicit host codec for APIs whose wire contract is UTF-8.  JavaScript strings can contain
 * lone UTF-16 surrogates, which have no UTF-8 scalar representation; this operation uses the
 * host's documented replacement semantics for those units and is deliberately named so callers
 * cannot mistake it for a unit-preserving string operation.  The returned buffer is malloc'd and
 * belongs to the caller. */
static int kiln_c99_string_to_utf8_lossy(kiln_value value, uint8_t **out, size_t *out_len);
static int kiln_c99_text_append_string(kiln_text *text, kiln_value value);

/* Append `len` bytes. A piece that fits is copied here, with a byte loop for the short ones a
 * JSON writer appends by the thousand -- a quote, a comma, a key -- where a `memmove` call cost
 * more than the copy; a piece that does not fit takes the growth path out of line. */
static inline int kiln_c99_text_append(kiln_text *text, const char *bytes, size_t len) {
    if (KILN_LIKELY(text->len + len <= text->capacity)) {
        char *at = text->bytes + text->len;
        if (len <= 16) {
            size_t i;
            for (i = 0; i < len; ++i) at[i] = bytes[i];
        } else {
            memcpy(at, bytes, len);
        }
        text->len += len;
        return KILN_C99_OK;
    }
    if (len == 0) return KILN_C99_OK;
    return kiln_c99_text_append_grow(text, bytes, len);
}

/* Internal link-time runtime contract. The final executable/shared library supplies these symbols
 * from the Rust provider archive; they are not part of the public embedding ABI.
 *
 * The offsets `kiln_regexp_search` reports are UTF-16 code units, because that is what the
 * language counts in (ADR 0022) and matching over UTF-16 makes them so without a conversion. */
int kiln_number_to_js_string(double value, char *buffer, size_t capacity);
double kiln_string_to_number(const char *bytes, size_t len);
/* `%TypedArray%.prototype.sort`'s default order, in place on the calling thread (the order
 * `kiln_number_core::typed_order` owns for both engines); `kind` is a `kiln_typed_kind`. */
int32_t kiln_typed_array_sort(uint8_t *data, size_t count, int32_t kind);
int kiln_intl_segment(const uint8_t *bytes, size_t len, uint32_t granularity,
                      const uint32_t **out_records, size_t *out_count);
int kiln_intl_string_case(const uint16_t *units, uint64_t count, const uint8_t *locale,
                          uint64_t locale_len, uint32_t uppercase, uint16_t *out,
                          uint64_t capacity, uint64_t *out_len);
int kiln_intl_datetime_format(const uint8_t *spec, uint64_t spec_len, double epoch_milliseconds,
                              uint32_t operation, const uint8_t **out, uint64_t *out_len);
int kiln_crypto_key_import(const uint8_t *data, size_t length, uint32_t kind,
                           uint64_t *out_handle);
int kiln_crypto_key_public(uint64_t handle, uint64_t *out_handle);
int kiln_crypto_key_info(uint64_t handle, uint32_t *out_type, uint32_t *out_bits);
int kiln_crypto_key_export(uint64_t handle, uint32_t format, uint32_t encoding,
                           uint8_t *output, size_t capacity, size_t *out_length);
int kiln_crypto_key_free(uint64_t handle);
typedef int (*kiln_realm_work_fn)(void *work, void *host, void *thrown);
typedef void (*kiln_realm_drop_fn)(void *work);
/* The promise job lane the prelude owns (Phase 6.3). Promise reactions and await resumptions
 * are queued in the prelude, not handed to the runtime one FFI call and one GC cell at a time;
 * the realm calls back into the lane at the point in its drain where the promise jobs belong,
 * and `pending` is what tells it there is anything there. `context` is the heap the ring lives
 * in, so a realm never drains another realm's lane. */
typedef int (*kiln_realm_lane_run_fn)(void *context, void *host, void *thrown);
typedef int (*kiln_realm_lane_pending_fn)(void *context);
typedef int (*kiln_realm_entry_fn)(void *context);
#define KILN_REALM_ASYNC_ABI_VERSION 6
#define KILN_REALM_ASYNC_OK 0
#define KILN_REALM_ASYNC_ERROR 1
#define KILN_REALM_ASYNC_NOT_FOUND 1
#define KILN_REALM_ASYNC_UNDEFINED 0
#define KILN_REALM_ASYNC_BYTES 1
#define KILN_REALM_ASYNC_FILE_STAT 2
#define KILN_REALM_ASYNC_RESOURCE 3
#define KILN_REALM_ASYNC_RESOURCE_STATE 4
#define KILN_REALM_ASYNC_ACCEPTED_STREAM 5
#define KILN_REALM_ASYNC_PROCESS_OUTPUT 6
#define KILN_REALM_OP_READ_FILE 1
#define KILN_REALM_OP_WRITE_FILE 2
#define KILN_REALM_OP_STAT 3
#define KILN_REALM_OP_CLOSE_RESOURCE 4
#define KILN_REALM_OP_TCP_LISTEN 7
#define KILN_REALM_OP_TCP_ACCEPT 8
#define KILN_REALM_OP_TCP_CONNECT 9
#define KILN_REALM_OP_TCP_READ 10
#define KILN_REALM_OP_TCP_WRITE 11
#define KILN_REALM_OP_TCP_SHUTDOWN 12
#define KILN_REALM_OP_STDIN_READ 13
#define KILN_REALM_OP_MKDIR 14
#define KILN_REALM_OP_PROCESS_RUN 15
/* Done once a cross-realm port (`resource`) has something for this realm (realms issue 06). */
#define KILN_REALM_OP_PORT_WAIT 16
#define KILN_REALM_OP_ATOMICS_WAIT 17
#define KILN_REALM_HTTP_CLOSE_GRACEFUL 0
#define KILN_REALM_HTTP_CLOSE_ABORT 1
/* One element of a message between realms, as the runtime's ports carry it
 * (`kiln_runtime::workers`, realms issue 06): the shared TypeScript flattens a structured clone
 * into these, and the provider turns them into its tokens and back. A moved block is `malloc`
 * memory a detached `ArrayBuffer` gave up; a shared store is a `SharedArrayBuffer`'s. Whoever
 * adopts a `handle` clears it in the token. */
typedef struct kiln_worker_token {
    uint32_t kind;
    uint32_t flag;
    double number;
    const void *data;
    uint64_t len;
    void *handle;
} kiln_worker_token;
typedef struct kiln_worker_plain_node {
    uint32_t kind;
    double number;
    const kiln_string *text;
    size_t identity;
    size_t length;
} kiln_worker_plain_node;
typedef struct kiln_worker_plain_child {
    const kiln_string *key;
    uint64_t value;
    uint32_t hole;
} kiln_worker_plain_child;
typedef int (*kiln_worker_plain_describe)(void *context, uint64_t handle,
                                           kiln_worker_plain_node *out);
typedef int (*kiln_worker_plain_field)(void *context, uint64_t handle, size_t index,
                                        kiln_worker_plain_child *out);
#define KILN_WORKER_TOKEN_UNDEFINED 0u
#define KILN_WORKER_TOKEN_NULL 1u
#define KILN_WORKER_TOKEN_BOOL 2u
#define KILN_WORKER_TOKEN_NUMBER 3u
#define KILN_WORKER_TOKEN_STRING 4u
#define KILN_WORKER_TOKEN_MOVED 5u
#define KILN_WORKER_TOKEN_BYTES 6u
#define KILN_WORKER_TOKEN_SHARED 7u
#define KILN_WORKER_TOKEN_GRAPH 8u
typedef struct kiln_async_host_op {
    uint32_t operation;
    uint32_t flags;
    uint64_t resource;
    uint64_t argument;
    const uint8_t *text;
    uint64_t text_len;
    const uint8_t *bytes;
    uint64_t bytes_len;
} kiln_async_host_op;
typedef struct kiln_async_completion {
    uint64_t request;
    uint64_t sequence;
    int32_t status;
    uint32_t value_kind;
    const uint8_t *bytes;
    uint64_t bytes_len;
    const uint8_t *stderr_bytes;
    uint64_t stderr_len;
    int32_t exit_code;
    int32_t signal;
    uint64_t value_u64;
    uint64_t resource;
    uint32_t resource_kind;
    uint32_t resource_port;
    uint32_t resource_flags;
    const uint8_t *address;
    uint64_t address_len;
    const uint8_t *peer_address;
    uint64_t peer_address_len;
    uint32_t peer_port;
    const uint8_t *error_code;
    uint64_t error_code_len;
    const uint8_t *error_syscall;
    uint64_t error_syscall_len;
    const uint8_t *error_path;
    uint64_t error_path_len;
    const uint8_t *error_message;
    uint64_t error_message_len;
} kiln_async_completion;
typedef int (*kiln_realm_completion_fn)(void *work, const kiln_async_completion *completion,
                                       void *host, void *thrown);
/* The command line the launcher's `main` was handed, recorded on the realm before it was
 * entered. A realm nobody launched -- an embedded module, a unit test -- reports zero, which is
 * how `process.argv` stays empty instead of naming an executable that does not exist. */
/* The same virtual clock, in whole epoch nanoseconds split across two halves because C99 has no
 * portable 128-bit integer. `Temporal` measures instants in nanoseconds, and rounding the realm's
 * clock through the millisecond accessor above would make the two engines disagree below the
 * millisecond rather than refuse. */
/* ADR 0031 civil time. Deterministic realms use the virtual accessor above; host realms call
 * HostOps::ClockNow through the explicit host argument. */
static int kiln_c99_wall_clock_milliseconds(kiln_host *host, double *out_milliseconds);
static int kiln_c99_wall_clock_nanoseconds(kiln_host *host, int64_t *out_high, uint64_t *out_low);
typedef struct kiln_realm_http_event {
    uint32_t kind;
    uint32_t flags;
    uint64_t id;
    uint16_t status;
    uint8_t version_minor;
    uint8_t reserved;
    const uint8_t *method;
    uint64_t method_len;
    const uint8_t *target;
    uint64_t target_len;
    const uint8_t *reason;
    uint64_t reason_len;
    const uint8_t *headers;
    uint64_t headers_len;
    const uint8_t *data;
    uint64_t data_len;
    uint64_t write_fence;
} kiln_realm_http_event;
typedef struct kiln_realm_http_request {
    const uint8_t *method;
    uint64_t method_len;
    const uint8_t *target;
    uint64_t target_len;
    const uint8_t *headers;
    uint64_t headers_len;
    const uint8_t *body;
    uint64_t body_len;
} kiln_realm_http_request;
typedef struct kiln_realm_http_response {
    uint16_t status;
    uint16_t reserved;
    const uint8_t *headers;
    uint64_t headers_len;
} kiln_realm_http_response;
typedef int (*kiln_realm_http_event_fn)(void *work, const kiln_realm_http_event *event,
                                       void *host, void *thrown);
