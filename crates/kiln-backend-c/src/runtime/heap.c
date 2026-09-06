
/* ---------------------------------------------------------------------------------------------
 * The collector (ADR 0041).
 *
 * Every allocation a JavaScript value can point at, or that holds one, comes from
 * `kiln_c99_gc_alloc`. It carries a header naming its kind, its mark epoch and its size, is
 * linked into one heap shared by every translation unit of the program, and is entered in a hash
 * set of block addresses. Marking is precise about edges and paranoid about targets: a candidate
 * pointer is marked only when the set knows it, so a string that points at a constant table, a
 * symbol that points at the well-known table, or an allocation site still on plain `malloc` is
 * ignored rather than freed.
 *
 * Collection runs in two places now. At the top of the realm's drive loop, no generated frame is
 * on the stack (ADR 0041 says why that was enough for a server and not yet enough for a
 * CPU-bound body); there the runtime archive calls `collect` through the hooks below, never
 * reading a C layout, and enumerates the roots it owns by handing pointers and value slots back
 * to `mark_block` and `mark_value`, with each translation unit marking its own statics through a
 * registered root function. A CPU-bound body is also covered: `kiln_c99_gc_alloc` can call
 * `kiln_c99_gc_collect_mid_turn` from inside a turn, once allocation since the last collection
 * has grown enough to be worth the pass, and that path has no root list to hand back -- generated
 * frames are on the stack -- so `kiln_c99_gc_scan_stack` treats the stack conservatively instead,
 * scanning both the spilled callee-saved registers (`setjmp`) and every word from there down to
 * the realm's recorded stack base, marking anything that resolves to a live block or interior
 * pointer and leaving everything else alone.
 *
 * The heap struct's first member is the hooks pointer. The runtime reads exactly that one word
 * through the otherwise opaque heap pointer, so a root function outside the prelude (the graph
 * launcher) can mark through `kiln_realm_gc_mark_value`.
 * ------------------------------------------------------------------------------------------- */

#define KILN_GC_PHASE_MARK 0
#define KILN_GC_PHASE_TEARDOWN 1
#define KILN_GC_PHASE_RESET 2
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_HOOKS_VERSION 2u
#define KILN_GC_DEFAULT_FLOOR ((size_t)256 * 1024)
#define KILN_GC_MID_TURN_FLOOR ((size_t)4 * 1024 * 1024)
/* What the `introspect` hook reports back, as a bitmask. `ARMED` is what tells the drive to keep
 * its park short enough to notice a signal; `DUMPED` says a request was pending and was served. */
#define KILN_C99_INTROSPECT_ARMED 1
#define KILN_C99_INTROSPECT_DUMPED 2

/* `kind` carries `kiln_gc_kind` in its low bits and, for a `KILN_GC_STRING` block only, a cached
 * answer to "is this string all ASCII" (ADR 0022: ASCII is exactly the case where a code-unit
 * index equals a byte offset, which is what every position-taking string operation walks UTF-8 to
 * find) in its top two. `KILN_GC_KIND_COUNT` is nowhere near the mask's ~30 low bits, so the two
 * high bits are free; every reader of `block->kind` as a `kiln_gc_kind` must mask with
 * `KILN_GC_KIND_MASK` first. */
#define KILN_GC_KIND_MASK ((uint32_t)0x0FFFFFFFu)
#define KILN_GC_FLAG_ASCII_KNOWN ((uint32_t)0x80000000u)
#define KILN_GC_FLAG_ASCII ((uint32_t)0x40000000u)
#define KILN_GC_FLAG_NURSERY ((uint32_t)0x20000000u)
#define KILN_GC_FLAG_DEAD ((uint32_t)0x10000000u)

typedef struct kiln_gc_block {
    struct kiln_gc_block *prev;
    struct kiln_gc_block *next;
    size_t size;
    uint32_t kind;
    uint32_t mark;
} kiln_gc_block;

/* Small-object slabs (perf/gc-slab-allocator). Most turns allocate a dozen blocks under 128
 * bytes; routing those through `malloc`/`free` plus a hash-table entry each was the allocator's
 * whole cost. A slab is one size-aligned allocation holding many same-size slots, so `kiln_c99_gc_lookup`
 * can answer membership by masking to the slab's base and reading its header instead of probing a
 * hash table sized by the live set. The block header above is unchanged and is what every slot
 * holds, so the tracer and finaliser never know which path a block came from. Alignment makes
 * "is this address in one of my slabs" a value comparison (`kiln_gc_ptrset_contains` on the slab
 * base) before anything derived from an untrusted candidate pointer is dereferenced -- marking
 * still never trusts a pointer (ADR 0041); the value check is the fail-safe the hash table gave
 * per block, done once per slab instead. */
#define KILN_GC_SLAB_SIZE ((size_t)64 * 1024)
#define KILN_GC_SLAB_CLASS_MIN ((size_t)48) /* smallest total slot (header + 1 byte payload) */
#define KILN_GC_SLAB_CLASS_MAX ((size_t)256)
#define KILN_GC_SLAB_CLASS_STEP ((size_t)16)
#define KILN_GC_SLOT_FREE ((size_t)-1) /* block->size sentinel: this slot is on the free list */

typedef struct kiln_gc_slab {
    struct kiln_gc_slab *all_prev;  /* heap-wide list of every slab, for sweep and teardown */
    struct kiln_gc_slab *all_next;
    struct kiln_gc_slab *free_prev; /* per-class list of slabs with >=1 free slot */
    struct kiln_gc_slab *free_next;
    kiln_gc_block *free_head;       /* free list threaded through freed slots' `next` field */
    uint32_t class_index;
    uint32_t slot_size;             /* bytes per slot: header + this class's payload, rounded */
    uint32_t slot_count;
    uint32_t free_count;
    uint32_t first_slot_offset;     /* bytes from the slab base to slot 0 */
} kiln_gc_slab;

/* A tiny open-addressed set of pointer values, shared by the large-block table (keys are block
 * headers) and the slab table (keys are slab base addresses). Lookup on a hit only ever compares
 * pointer values already in the set; nothing derived from an untrusted candidate is read until a
 * match confirms the memory is ours. */
typedef struct kiln_gc_ptrset {
    void **table;
    size_t capacity;
    size_t count;
} kiln_gc_ptrset;

/* Interned property keys (ADR 0047). One entry per distinct key spelling the realm has ever
 * stored or registered, mapping the bytes to the one pointer every property with that name
 * carries -- so `property->key == key` is the whole of a key comparison and a linear scan never
 * touches key bytes. The table is the heap's because the heap is the one struct every
 * translation unit shares: two units each keeping their own would have two canonical pointers
 * for one name, which is exactly the situation this exists to end.
 *
 * An entry is *pinned* when its bytes are immortal (a unit's constant table, a prelude literal,
 * a symbol's reserved key) or when the table itself has undertaken to keep a block alive because
 * a unit registered the same spelling after a computed key had already claimed it. Every other
 * entry is weak: its bytes are a string block that the properties holding the key keep marked,
 * and the collection that finds the block unmarked drops the entry before the sweep frees it. */
typedef struct kiln_key_entry {
    const char *bytes; /* NULL when the slot is empty */
    size_t len;
    uint64_t hash;
    int pinned;
} kiln_key_entry;

typedef struct kiln_key_table {
    kiln_key_entry *entries;
    size_t capacity; /* zero or a power of two */
    size_t count;
} kiln_key_table;

/* The literal keys one request costs, every time: the request and response objects, and the
 * `URL` a router builds from `req.url`.
 *
 * Interning is a hash of the bytes and a probe. These are the same twenty-odd strings on every
 * request, so ADR 0047's own "next step" applies: intern each once per heap and let the per-request
 * cost become an array index. The X-macro keeps the spelling and its length in one place, because
 * a key whose length disagrees with its text is a bug that hashes cleanly and looks up nothing.
 */
#define KILN_C99_HOT_KEYS(X)                     \
    X(METHOD, "method")                           \
    X(URL, "url")                                 \
    X(HTTP_VERSION, "httpVersion")                \
    X(HTTP_VERSION_MAJOR, "httpVersionMajor")     \
    X(HTTP_VERSION_MINOR, "httpVersionMinor")     \
    X(ABORTED, "aborted")                         \
    X(COMPLETE, "complete")                       \
    X(DESTROYED, "destroyed")                     \
    X(SOCKET, "socket")                           \
    X(CONNECTION, "connection")                   \
    X(HEADERS, "headers")                         \
    X(STATUS_CODE, "statusCode")                  \
    X(PATHNAME, "pathname")                       \
    X(GET, "get")                                 \
    X(SEARCH_PARAMS, "searchParams")

#define KILN_C99_HOT_KEY_ENUM(id, text) KILN_C99_HOT_KEY_##id,
enum kiln_c99_http_key {
    KILN_C99_HOT_KEYS(KILN_C99_HOT_KEY_ENUM) KILN_C99_HOT_KEY_COUNT
};
#undef KILN_C99_HOT_KEY_ENUM

#define KILN_C99_HOT_KEY_TEXT(id, text) text,
static const char *const kiln_c99_hot_key_text[KILN_C99_HOT_KEY_COUNT] = {
    KILN_C99_HOT_KEYS(KILN_C99_HOT_KEY_TEXT)};
#undef KILN_C99_HOT_KEY_TEXT

#define KILN_C99_HOT_KEY_LEN(id, text) sizeof(text) - 1,
static const size_t kiln_c99_hot_key_len[KILN_C99_HOT_KEY_COUNT] = {
    KILN_C99_HOT_KEYS(KILN_C99_HOT_KEY_LEN)};
#undef KILN_C99_HOT_KEY_LEN

typedef struct kiln_nursery_chunk {
    char *base;
    char *limit;
    uint64_t *bitmap;
    size_t bitmap_words;
    size_t live_count;
    struct kiln_nursery_chunk *next;
} kiln_nursery_chunk;

typedef struct kiln_gc_nursery {
    char *base;
    char *bump;
    char *limit;
    size_t capacity;
    uint64_t *bitmap;
    size_t bitmap_words;
    int active;
    uint64_t count;
    uint64_t bytes;
    uint64_t promotions;
    uint64_t resets;
    kiln_nursery_chunk *promoted;
} kiln_gc_nursery;

typedef struct kiln_gc_heap {
    const kiln_gc_hooks *hooks;
    kiln_gc_block *head; /* large blocks only; slab blocks are found by walking slabs */
    size_t count;
    size_t bytes;
    size_t since_collection;
    size_t survived;
    size_t floor;
    uint32_t epoch;
    int enabled;
    int stress;
    int trace;
    int collecting;
    int mark_failed;
    int fallback;
    kiln_gc_ptrset large_table;
    kiln_gc_ptrset slab_table;
    kiln_gc_slab *all_slabs;
    kiln_gc_slab *class_free_head[(KILN_GC_SLAB_CLASS_MAX - KILN_GC_SLAB_CLASS_MIN) /
                                       KILN_GC_SLAB_CLASS_STEP +
                                   1];
    kiln_gc_nursery nursery;
    kiln_gc_block **stack;
    size_t stack_len;
    size_t stack_capacity;
    uint64_t collections;
    uint64_t freed_blocks;
    uint64_t freed_bytes;
    kiln_key_table keys;
    /* Canonical pointers for the keys the HTTP path writes on every request and response. See
     * KILN_C99_HOT_KEYS. NULL means "not interned in this heap yet"; the table is per-heap
     * because a canonical pointer from one realm means nothing in another. */
    const char *hot_keys[KILN_C99_HOT_KEY_COUNT];
    /* Mid-turn collection (perf/gc-mid-turn). The collector used to run only between turns,
     * when no frame of the program is live and the realm's roots are the whole root set. A
     * synchronous loop then never had its garbage reclaimed: it spilled out of the nursery into
     * slabs and grew until the turn ended. An allocation that finds `since_collection` past
     * the threshold now collects on the spot, with the program's frames found the way
     * JavaScriptCore finds them: every word of the machine stack, and of the callee-saved
     * registers spilled onto it, that decodes to an address inside a live block is a root.
     * Objects never move, so a word that only looks like a pointer keeps one block alive for
     * one collection and costs nothing else. */
    void *stack_base;
    int mid_turn;
    size_t stress_every;
    size_t stress_countdown;
    /* Call-site caches that remember a heap object -- the holder a method lives on -- are
     * registered on first fill and cleared after any collection that frees what they
     * remember, so a stale entry can never match an object that later reuses the address. */
    kiln_c99_site **sites;
    size_t site_len;
    size_t site_cap;
} kiln_gc_heap;

static const kiln_gc_hooks kiln_c99_gc_hooks;
/* The heap a unit uses when no realm is active: the prelude's own tests. It never collects. */
static kiln_gc_heap kiln_c99_gc_fallback;

static void kiln_c99_nursery_init(kiln_gc_nursery *nursery) {
    const char *setting;
    memset(nursery, 0, sizeof *nursery);
    nursery->capacity = (size_t)2 * 1024 * 1024;
    setting = getenv("KILN_NURSERY_KIB");
    if (setting != NULL && setting[0] >= '0' && setting[0] <= '9') {
        size_t kib = (size_t)strtoul(setting, NULL, 10);
        if (kib > 0 && kib <= 64 * 1024) nursery->capacity = kib * 1024;
    }
    nursery->base = (char *)malloc(nursery->capacity);
    if (nursery->base != NULL) {
        nursery->bump = nursery->base;
        nursery->limit = nursery->base + nursery->capacity;
        nursery->bitmap_words = (nursery->capacity / 16 + 63) / 64;
        nursery->bitmap = (uint64_t *)calloc(nursery->bitmap_words, sizeof(uint64_t));
        if (nursery->bitmap != NULL) {
            nursery->active = 1;
        } else {
            free(nursery->base);
            nursery->base = NULL;
            nursery->active = 0;
        }
    }
    setting = getenv("KILN_NURSERY");
    if (setting != NULL && setting[0] == '0' && setting[1] == '\0') {
        nursery->active = 0;
    }
}

static void kiln_c99_nursery_destroy(kiln_gc_nursery *nursery) {
    if (nursery->base != NULL) {
        free(nursery->base);
        nursery->base = NULL;
    }
    if (nursery->bitmap != NULL) {
        free(nursery->bitmap);
        nursery->bitmap = NULL;
    }
    while (nursery->promoted != NULL) {
        kiln_nursery_chunk *next = nursery->promoted->next;
        free(nursery->promoted->base);
        free(nursery->promoted->bitmap);
        free(nursery->promoted);
        nursery->promoted = next;
    }
    nursery->active = 0;
}

static void kiln_c99_gc_heap_init(kiln_gc_heap *heap) {
    const char *setting;
    memset(heap, 0, sizeof *heap);
    heap->hooks = &kiln_c99_gc_hooks;
    heap->floor = KILN_GC_DEFAULT_FLOOR;
    heap->enabled = 1;
    setting = getenv("KILN_GC");
    if (setting != NULL && setting[0] == '0' && setting[1] == '\0') heap->enabled = 0;
    setting = getenv("KILN_GC_STRESS");
    heap->stress = setting != NULL && setting[0] != '\0' && setting[0] != '0';
    /* `KILN_GC_STRESS=N` also collects mid-turn every N allocations, `1` on every one: the
     * conservative scan's correctness test, since a root it misses shows up as a freed block
     * under a live reference within a few allocations. */
    if (heap->stress) {
        size_t every = 0;
        const char *digit = setting;
        while (*digit >= '0' && *digit <= '9' && every < ((size_t)-1) / 10) {
            every = every * 10 + (size_t)(*digit - '0');
            ++digit;
        }
        heap->stress_every = every == 0 ? 1 : every;
        heap->stress_countdown = heap->stress_every;
    }
    setting = getenv("KILN_GC_TRACE");
    heap->trace = setting != NULL && setting[0] != '\0' && setting[0] != '0';
    setting = getenv("KILN_GC_FLOOR_KIB");
    if (setting != NULL && setting[0] >= '0' && setting[0] <= '9') {
        size_t kib = 0;
        while (*setting >= '0' && *setting <= '9' && kib < ((size_t)-1) / 10240) {
            kib = kib * 10 + (size_t)(*setting - '0');
            ++setting;
        }
        heap->floor = kib * 1024;
    }
    kiln_c99_nursery_init(&heap->nursery);
}

/* Diagnostics stay off the locale-sensitive printf family (decision recorded beside
 * kiln_number_to_js_string): a count is spelled here by hand. */
static void kiln_c99_gc_trace_text(const char *text) { fputs(text, stderr); }

static void kiln_c99_gc_trace_count(uint64_t count) {
    char digits[24];
    size_t at = sizeof digits;
    digits[--at] = '\0';
    do {
        digits[--at] = (char)('0' + (int)(count % 10));
        count /= 10;
    } while (count != 0 && at != 0);
    fputs(&digits[at], stderr);
}

/* ---- SIGUSR1 runtime introspection ------------------------------------------------------
 *
 * A compiled server has no REPL, no inspector and no admin port, so `kill -USR1` is how a
 * production process is asked what it is doing. It answers with three lines on stderr and keeps
 * serving.
 *
 * The handler does the one thing a handler may do -- assign to a `volatile sig_atomic_t` -- and
 * the dump runs at the next turn boundary, the safepoint ADR 0041 already collects at, where no
 * generated frame is on the stack. Deferring is not merely stylish here: the kernel runs a
 * process-directed signal on whichever thread has not blocked it, Tokio's blocking pool included,
 * so the handler may be on a thread that has never touched this heap. Setting a process-global
 * flag is sound from there; reading the heap would not be. One realm per process (KILN_WORKERS
 * forks rather than threads) is what makes the flag unambiguous, but it does not make the
 * delivering thread predictable.
 *
 * The formatter below obeys the async-signal-safe rules anyway -- a stack buffer, hand-rolled
 * base-ten spelling, one write(2) per line, no malloc and no stdio. Only the `heap` line could
 * actually be produced from the handler; `runtime` and `connections` cross into the archive. The
 * discipline is kept because stdio is the wrong tool here for the locale reason recorded beside
 * kiln_number_to_js_string, and because one write per line means a dump racing another writer on
 * fd 2 interleaves lines rather than characters. */

/* Fields kiln_realm_runtime_report fills, in its documented order. */
#define KILN_C99_RUNTIME_REPORT_FIELDS 9

#if defined(__unix__) || defined(__APPLE__)

/* Set by the handler on an arbitrary thread, read and cleared at the turn boundary. Process
 * state, not realm state: it names a request that has arrived, and it outlives any one realm. */
static volatile sig_atomic_t kiln_c99_introspect_pending;
static int kiln_c99_introspect_armed;

static void kiln_c99_introspect_handler(int signal_number) {
    (void)signal_number;
    kiln_c99_introspect_pending = 1;
}

/* SA_RESTART so an interrupted accept or read resumes: a diagnostic must not be able to fail the
 * server it is diagnosing with EINTR. */
static int kiln_c99_introspect_arm(void) {
    struct sigaction action;
    const char *setting = getenv("KILN_INTROSPECT");
    if (setting != NULL && setting[0] == '0' && setting[1] == '\0') return 0;
    memset(&action, 0, sizeof action);
    action.sa_handler = kiln_c99_introspect_handler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART;
    return sigaction(SIGUSR1, &action, NULL) == 0;
}

typedef struct kiln_c99_introspect_line {
    char bytes[512];
    size_t len;
} kiln_c99_introspect_line;

static void kiln_c99_introspect_text(kiln_c99_introspect_line *line, const char *text) {
    while (*text != '\0' && line->len + 1 < sizeof line->bytes) line->bytes[line->len++] = *text++;
}

static void kiln_c99_introspect_number(kiln_c99_introspect_line *line, uint64_t value) {
    char digits[24];
    size_t at = sizeof digits;
    do {
        digits[--at] = (char)('0' + (int)(value % 10));
        value /= 10;
    } while (value != 0 && at != 0);
    while (at < sizeof digits && line->len + 1 < sizeof line->bytes)
        line->bytes[line->len++] = digits[at++];
}

static void kiln_c99_introspect_field(kiln_c99_introspect_line *line, const char *name,
                                      uint64_t value) {
    kiln_c99_introspect_text(line, " ");
    kiln_c99_introspect_text(line, name);
    kiln_c99_introspect_text(line, "=");
    kiln_c99_introspect_number(line, value);
}

/* The pid leads every line because KILN_WORKERS forks: a signal sent to the process group
 * produces one three-line group per worker, and they have to be separable. */
static void kiln_c99_introspect_begin(kiln_c99_introspect_line *line, const char *section) {
    line->len = 0;
    kiln_c99_introspect_text(line, "kiln introspect pid=");
    kiln_c99_introspect_number(line, (uint64_t)getpid());
    kiln_c99_introspect_text(line, " ");
    kiln_c99_introspect_text(line, section);
}

/* One write per line. A short write is retried a bounded number of times rather than by reading
 * errno, which a handler would have to save and restore. */
static void kiln_c99_introspect_flush(kiln_c99_introspect_line *line) {
    size_t at = 0;
    int retries = 0;
    if (line->len < sizeof line->bytes) line->bytes[line->len++] = '\n';
    else line->bytes[sizeof line->bytes - 1] = '\n';
    while (at < line->len) {
        ssize_t written = write(2, line->bytes + at, line->len - at);
        if (written < 0) {
            if (++retries > 8) break;
            continue;
        }
        if (written == 0) break;
        at += (size_t)written;
    }
}

static void kiln_c99_introspect_dump(kiln_gc_heap *heap) {
    kiln_c99_introspect_line line;
    uint64_t fields[KILN_C99_RUNTIME_REPORT_FIELDS];
    uint64_t reported = 0;
    size_t index;

    kiln_c99_introspect_begin(&line, "heap");
    kiln_c99_introspect_field(&line, "bytes", (uint64_t)heap->bytes);
    kiln_c99_introspect_field(&line, "count", (uint64_t)heap->count);
    kiln_c99_introspect_field(&line, "since_collection", (uint64_t)heap->since_collection);
    kiln_c99_introspect_field(&line, "survived", (uint64_t)heap->survived);
    kiln_c99_introspect_field(&line, "floor", (uint64_t)heap->floor);
    kiln_c99_introspect_field(&line, "epoch", (uint64_t)heap->epoch);
    kiln_c99_introspect_field(&line, "collections", heap->collections);
    kiln_c99_introspect_field(&line, "freed_bytes", heap->freed_bytes);
    kiln_c99_introspect_field(&line, "nursery_promotions", heap->nursery.promotions);
    kiln_c99_introspect_field(&line, "nursery_resets", heap->nursery.resets);
    kiln_c99_introspect_field(&line, "enabled", (uint64_t)(heap->enabled != 0));
    kiln_c99_introspect_field(&line, "collecting", (uint64_t)(heap->collecting != 0));
    kiln_c99_introspect_flush(&line);

    for (index = 0; index < KILN_C99_RUNTIME_REPORT_FIELDS; ++index) fields[index] = 0;
    if (kiln_realm_runtime_report(fields, (uint64_t)KILN_C99_RUNTIME_REPORT_FIELDS, &reported) !=
        KILN_C99_OK) {
        return;
    }
    /* Fields 0-5 are interval counters covering the time since the previous dump; 6-8 are
     * instantaneous. A shorter report than this prelude knows about prints the prefix it got. */
    if (reported >= 6) {
        kiln_c99_introspect_begin(&line, "runtime");
        kiln_c99_introspect_field(&line, "live_tasks", fields[0]);
        kiln_c99_introspect_field(&line, "park_count", fields[1]);
        kiln_c99_introspect_field(&line, "busy_us", fields[2]);
        kiln_c99_introspect_field(&line, "elapsed_us", fields[3]);
        kiln_c99_introspect_field(&line, "queue_depth", fields[4]);
        kiln_c99_introspect_field(&line, "workers", fields[5]);
        kiln_c99_introspect_flush(&line);
    }
    if (reported >= 9) {
        kiln_c99_introspect_begin(&line, "connections");
        kiln_c99_introspect_field(&line, "server", fields[6]);
        kiln_c99_introspect_field(&line, "client", fields[7]);
        kiln_c99_introspect_field(&line, "requests_in_flight", fields[8]);
        kiln_c99_introspect_flush(&line);
    }
}

static int kiln_c99_gc_hook_introspect(void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    int status;
    if (heap == NULL || heap->fallback) return 0;
    status = kiln_c99_introspect_armed ? KILN_C99_INTROSPECT_ARMED : 0;
    if (kiln_c99_introspect_pending) {
        kiln_c99_introspect_pending = 0;
        kiln_c99_introspect_dump(heap);
        status |= KILN_C99_INTROSPECT_DUMPED;
    }
    return status;
}

#else

/* A target without POSIX signals compiles the same prelude with the dump inert; the two statics
 * stay declared so the ownership guard sees one shape on every platform. */
static volatile int kiln_c99_introspect_pending;
static int kiln_c99_introspect_armed;

static int kiln_c99_introspect_arm(void) { return 0; }

static int kiln_c99_gc_hook_introspect(void *opaque) {
    (void)opaque;
    (void)kiln_c99_introspect_pending;
    return 0;
}

#endif

static kiln_gc_heap *kiln_c99_gc_heap(void) {
    void *attached = kiln_realm_gc_current_heap();
    kiln_gc_heap *candidate;
    int status;
    if (attached != NULL) return (kiln_gc_heap *)attached;
    status = kiln_realm_gc_attach(&kiln_c99_gc_hooks, NULL, &attached);
    if (status == KILN_C99_OK && attached != NULL) return (kiln_gc_heap *)attached;
    if (status != KILN_C99_OK) {
        if (kiln_c99_gc_fallback.hooks == NULL) {
            kiln_c99_gc_heap_init(&kiln_c99_gc_fallback);
            kiln_c99_gc_fallback.enabled = 0;
            kiln_c99_gc_fallback.fallback = 1;
        }
        return &kiln_c99_gc_fallback;
    }
    candidate = (kiln_gc_heap *)malloc(sizeof *candidate);
    if (candidate == NULL) return NULL;
    kiln_c99_gc_heap_init(candidate);
    status = kiln_realm_gc_attach(&kiln_c99_gc_hooks, candidate, &attached);
    if (status != KILN_C99_OK || attached == NULL) {
        kiln_c99_nursery_destroy(&candidate->nursery);
        free(candidate);
        return NULL;
    }
    if (attached != candidate) {
        kiln_c99_nursery_destroy(&candidate->nursery);
        free(candidate);
    }
    /* Arm here, in the unit whose candidate won: the heap now carries *this* unit's static
     * kiln_c99_gc_hooks, so the `introspect` hook the archive calls reads the same copy of
     * kiln_c99_introspect_pending that this unit's handler writes. The prelude is emitted per
     * unit with internal linkage, so arming anywhere else would arm one flag and poll another. */
    else kiln_c99_introspect_armed = kiln_c99_introspect_arm();
    return (kiln_gc_heap *)attached;
}

static size_t kiln_c99_gc_hash(const void *pointer, size_t mask) {
    uint64_t bits = (uint64_t)(uintptr_t)pointer;
    bits ^= bits >> 29;
    bits *= UINT64_C(0xBF58476D1CE4E5B9);
    bits ^= bits >> 32;
    return (size_t)bits & mask;
}

static int kiln_c99_gc_ptrset_grow(kiln_gc_ptrset *set, size_t wanted) {
    void **grown;
    void **old = set->table;
    size_t old_capacity = set->capacity;
    size_t index;
    size_t capacity = 64;
    while (capacity < wanted * 2) {
        if (capacity > ((size_t)-1) / 4) return KILN_C99_OUT_OF_MEMORY;
        capacity *= 2;
    }
    grown = (void **)calloc(capacity, sizeof *grown);
    if (grown == NULL) return KILN_C99_OUT_OF_MEMORY;
    set->table = grown;
    set->capacity = capacity;
    set->count = 0;
    for (index = 0; index < old_capacity; ++index) {
        void *entry = old[index];
        if (entry != NULL) {
            size_t slot = kiln_c99_gc_hash(entry, capacity - 1);
            while (grown[slot] != NULL) slot = (slot + 1) & (capacity - 1);
            grown[slot] = entry;
            ++set->count;
        }
    }
    free(old);
    return KILN_C99_OK;
}

static int kiln_c99_gc_ptrset_insert(kiln_gc_ptrset *set, void *pointer) {
    size_t slot;
    size_t mask;
    if ((set->count + 1) * 2 > set->capacity) {
        int status = kiln_c99_gc_ptrset_grow(set, set->count + 1);
        if (status != KILN_C99_OK) return status;
    }
    mask = set->capacity - 1;
    slot = kiln_c99_gc_hash(pointer, mask);
    while (set->table[slot] != NULL) slot = (slot + 1) & mask;
    set->table[slot] = pointer;
    ++set->count;
    return KILN_C99_OK;
}

/* Linear probing with backward-shift deletion, so a release leaves no tombstone behind. */
static void kiln_c99_gc_ptrset_remove(kiln_gc_ptrset *set, const void *pointer) {
    size_t mask;
    size_t slot;
    size_t hole;
    if (set->capacity == 0) return;
    mask = set->capacity - 1;
    slot = kiln_c99_gc_hash(pointer, mask);
    while (set->table[slot] != pointer) {
        if (set->table[slot] == NULL) return;
        slot = (slot + 1) & mask;
    }
    hole = slot;
    for (;;) {
        size_t home;
        slot = (slot + 1) & mask;
        if (set->table[slot] == NULL) break;
        home = kiln_c99_gc_hash(set->table[slot], mask);
        /* Move the entry back into the hole when its home sits at or before the hole in probe
         * order, otherwise a lookup that started at its home would stop at the hole. */
        if ((slot > hole && (home <= hole || home > slot)) ||
            (slot < hole && home <= hole && home > slot)) {
            set->table[hole] = set->table[slot];
            hole = slot;
        }
    }
    set->table[hole] = NULL;
    --set->count;
}

/* Value-only membership: never dereferences `pointer`, only compares it against addresses this
 * heap itself inserted. This is what makes masking an untrusted candidate down to a slab base
 * safe to then dereference -- the slab header is read only after this returns true. */
static int kiln_c99_gc_ptrset_contains(const kiln_gc_ptrset *set, const void *pointer) {
    size_t mask;
    size_t slot;
    if (pointer == NULL || set->capacity == 0) return 0;
    mask = set->capacity - 1;
    slot = kiln_c99_gc_hash(pointer, mask);
    while (set->table[slot] != NULL) {
        if (set->table[slot] == pointer) return 1;
        slot = (slot + 1) & mask;
    }
    return 0;
}

/* Portable aligned allocation for slabs: only <stdlib.h> `malloc`/`free` are assumed (the
 * prelude is compiled `-std=c11`, and `posix_memalign` is not reliably declared there on every
 * target), so the raw pointer is stashed just before the aligned region it returns. */
static void *kiln_c99_gc_aligned_alloc(size_t alignment, size_t size) {
    unsigned char *raw = (unsigned char *)malloc(size + alignment - 1 + sizeof(void *));
    uintptr_t base;
    unsigned char *aligned;
    if (raw == NULL) return NULL;
    base = (uintptr_t)(raw + sizeof(void *));
    aligned = (unsigned char *)((base + alignment - 1) & ~(uintptr_t)(alignment - 1));
    ((void **)aligned)[-1] = raw;
    return aligned;
}

static void kiln_c99_gc_aligned_free(void *aligned) {
    if (aligned == NULL) return;
    free(((void **)aligned)[-1]);
}

/* Which size class (if any) holds a block whose header plus payload totals `total` bytes; -1
 * means the block is large and keeps the `malloc` path. */
static int kiln_c99_gc_slab_class(size_t total) {
    size_t rounded;
    if (total > KILN_GC_SLAB_CLASS_MAX) return -1;
    rounded = total <= KILN_GC_SLAB_CLASS_MIN
                  ? KILN_GC_SLAB_CLASS_MIN
                  : (total + (KILN_GC_SLAB_CLASS_STEP - 1)) &
                        ~(KILN_GC_SLAB_CLASS_STEP - 1);
    if (rounded > KILN_GC_SLAB_CLASS_MAX) return -1;
    return (int)((rounded - KILN_GC_SLAB_CLASS_MIN) / KILN_GC_SLAB_CLASS_STEP);
}

static kiln_gc_block *kiln_c99_gc_slab_slot(const kiln_gc_slab *slab, uint32_t index) {
    return (kiln_gc_block *)((char *)slab + slab->first_slot_offset +
                              (size_t)index * slab->slot_size);
}

static void kiln_c99_gc_slab_unlink_free(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    if (slab->free_prev != NULL) slab->free_prev->free_next = slab->free_next;
    else heap->class_free_head[slab->class_index] = slab->free_next;
    if (slab->free_next != NULL) slab->free_next->free_prev = slab->free_prev;
    slab->free_prev = NULL;
    slab->free_next = NULL;
}

static void kiln_c99_gc_slab_link_free(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    slab->free_prev = NULL;
    slab->free_next = heap->class_free_head[slab->class_index];
    if (heap->class_free_head[slab->class_index] != NULL)
        heap->class_free_head[slab->class_index]->free_prev = slab;
    heap->class_free_head[slab->class_index] = slab;
}

static void kiln_c99_gc_slab_unlink_all(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    if (slab->all_prev != NULL) slab->all_prev->all_next = slab->all_next;
    else heap->all_slabs = slab->all_next;
    if (slab->all_next != NULL) slab->all_next->all_prev = slab->all_prev;
    slab->all_prev = NULL;
    slab->all_next = NULL;
}

/* A slab that is entirely free is destroyed immediately unless it is the only supply left for
 * its class (`free_prev`/`free_next` both NULL): that bounds retention to about one empty slab
 * per class without forcing a fresh slab on the very next allocation of that class. */
static void kiln_c99_gc_slab_maybe_destroy(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    /* An empty slab is kept for reuse while the class has few of them: a loop that fills and
     * frees the same slabs every collection would otherwise create and initialise them again
     * each time, and that churn was a fifth of an allocation-heavy loop. */
    {
        size_t spare = 0;
        const kiln_gc_slab *other = heap->class_free_head[slab->class_index];
        while (other != NULL) {
            if (other != slab && other->free_count == other->slot_count) ++spare;
            other = other->free_next;
        }
        if (spare < 4) return;
    }
    if (slab->free_prev == NULL && slab->free_next == NULL) return;
    kiln_c99_gc_slab_unlink_free(heap, slab);
    kiln_c99_gc_slab_unlink_all(heap, slab);
    kiln_c99_gc_ptrset_remove(&heap->slab_table, slab);
    kiln_c99_gc_aligned_free(slab);
}

static kiln_gc_slab *kiln_c99_gc_slab_new(kiln_gc_heap *heap, int class_index) {
    kiln_gc_slab *slab;
    size_t slot_size = KILN_GC_SLAB_CLASS_MIN + (size_t)class_index * KILN_GC_SLAB_CLASS_STEP;
    size_t first_slot_offset = sizeof(kiln_gc_slab);
    size_t usable;
    size_t slot_count;
    uint32_t index;
    if (first_slot_offset % sizeof(void *) != 0)
        first_slot_offset += sizeof(void *) - (first_slot_offset % sizeof(void *));
    if (first_slot_offset >= KILN_GC_SLAB_SIZE) return NULL;
    usable = KILN_GC_SLAB_SIZE - first_slot_offset;
    slot_count = usable / slot_size;
    if (slot_count == 0) return NULL;
    slab = (kiln_gc_slab *)kiln_c99_gc_aligned_alloc(KILN_GC_SLAB_SIZE, KILN_GC_SLAB_SIZE);
    if (slab == NULL) return NULL;
    slab->class_index = (uint32_t)class_index;
    slab->slot_size = (uint32_t)slot_size;
    slab->slot_count = (uint32_t)slot_count;
    slab->free_count = (uint32_t)slot_count;
    slab->first_slot_offset = (uint32_t)first_slot_offset;
    slab->free_head = NULL;
    for (index = 0; index < (uint32_t)slot_count; ++index) {
        kiln_gc_block *slot = kiln_c99_gc_slab_slot(slab, index);
        slot->size = KILN_GC_SLOT_FREE;
        slot->kind = KILN_GC_NONE;
        slot->mark = 0;
        slot->prev = NULL;
        slot->next = slab->free_head;
        slab->free_head = slot;
    }
    if (kiln_c99_gc_ptrset_insert(&heap->slab_table, slab) != KILN_C99_OK) {
        kiln_c99_gc_aligned_free(slab);
        return NULL;
    }
    slab->all_prev = NULL;
    slab->all_next = heap->all_slabs;
    if (heap->all_slabs != NULL) heap->all_slabs->all_prev = slab;
    heap->all_slabs = slab;
    slab->free_prev = NULL;
    slab->free_next = heap->class_free_head[class_index];
    if (heap->class_free_head[class_index] != NULL)
        heap->class_free_head[class_index]->free_prev = slab;
    heap->class_free_head[class_index] = slab;
    return slab;
}

/* The one membership check every candidate edge pays for (ADR 0041: marking never trusts a
 * pointer). Masking to a slab base and reading its header is safe only because that read happens
 * after `kiln_gc_ptrset_contains` confirms the exact address value is one this heap inserted;
 * nothing derived from the untrusted `payload` is dereferenced before that. A slab hit still
 * checks the slot is exactly a payload start and not free, so an interior or stale pointer is
 * rejected the same way a foreign pointer always was. */
static kiln_gc_block *kiln_c99_gc_lookup_slab(const kiln_gc_heap *heap, const void *payload,
                                              kiln_gc_slab **out_slab) {
    uintptr_t address = (uintptr_t)payload;
    void *slab_base;
    if (out_slab != NULL) *out_slab = NULL;
    /* A candidate edge is not necessarily a pointer at all: `bits` and `ptr` share one word, so
     * a builtin's code or a method's code arrives here as a small integer. Stepping back from it
     * with pointer arithmetic is undefined behaviour on a pointer that never pointed at an
     * object, and the sanitizer traps it -- so addresses are compared and masked as integers and
     * anything that cannot be a payload is rejected before a pointer exists. */
    if (payload == NULL || address < sizeof(kiln_gc_block)) return NULL;
    slab_base = (void *)(address & ~(uintptr_t)(KILN_GC_SLAB_SIZE - 1));
    if (kiln_c99_gc_ptrset_contains(&heap->slab_table, slab_base)) {
        const kiln_gc_slab *slab = (const kiln_gc_slab *)slab_base;
        size_t offset = address - (uintptr_t)slab_base;
        size_t relative;
        size_t slot_index;
        kiln_gc_block *block;
        if (offset < slab->first_slot_offset) return NULL;
        relative = offset - slab->first_slot_offset;
        if (relative % slab->slot_size != sizeof(kiln_gc_block)) return NULL;
        slot_index = relative / slab->slot_size;
        if (slot_index >= slab->slot_count) return NULL;
        block = kiln_c99_gc_slab_slot(slab, (uint32_t)slot_index);
        if (block->size == KILN_GC_SLOT_FREE) return NULL;
        if (out_slab != NULL) *out_slab = (kiln_gc_slab *)slab;
        return block;
    }
    {
        /* Large block: unchanged from the pre-slab collector, an arithmetic step back to the
         * header followed by a value-only table check. */
        kiln_gc_block *block = (kiln_gc_block *)(address - sizeof(kiln_gc_block));
        if (kiln_c99_gc_ptrset_contains(&heap->large_table, block)) return block;
    }
    return NULL;
}

static void kiln_c99_nursery_clear_bit(kiln_gc_heap *heap, const kiln_gc_block *block) {
    const char *ptr = (const char *)block;
    if (heap == NULL || block == NULL) return;
    if (heap->nursery.active && ptr >= heap->nursery.base && ptr < heap->nursery.bump) {
        size_t offset = (size_t)(ptr - heap->nursery.base);
        size_t bit_idx = offset >> 4;
        if (bit_idx >> 6 < heap->nursery.bitmap_words) {
            heap->nursery.bitmap[bit_idx >> 6] &= ~((uint64_t)1 << (bit_idx & 63));
        }
        return;
    }
    {
        kiln_nursery_chunk *chunk = heap->nursery.promoted;
        while (chunk != NULL) {
            if (ptr >= chunk->base && ptr < chunk->limit) {
                size_t offset = (size_t)(ptr - chunk->base);
                size_t bit_idx = offset >> 4;
                if (bit_idx >> 6 < chunk->bitmap_words) {
                    chunk->bitmap[bit_idx >> 6] &= ~((uint64_t)1 << (bit_idx & 63));
                }
                return;
            }
            chunk = chunk->next;
        }
    }
}

static kiln_gc_block *kiln_c99_gc_lookup_nursery(const kiln_gc_heap *heap, const void *payload) {
    if (heap == NULL || !heap->nursery.active || payload == NULL) return NULL;
    /* Active nursery check */
    if ((const char *)payload >= heap->nursery.base + sizeof(kiln_gc_block) &&
        (const char *)payload < heap->nursery.bump) {
        size_t block_offset = (size_t)((const char *)payload - sizeof(kiln_gc_block) - heap->nursery.base);
        if ((block_offset & 15) == 0) {
            size_t bit_idx = block_offset >> 4;
            if (bit_idx >> 6 < heap->nursery.bitmap_words &&
                (heap->nursery.bitmap[bit_idx >> 6] & ((uint64_t)1 << (bit_idx & 63))) != 0) {
                kiln_gc_block *block = (kiln_gc_block *)(heap->nursery.base + block_offset);
                if ((block->kind & KILN_GC_FLAG_DEAD) == 0) return block;
            }
        }
    }
    if ((uintptr_t)payload >= sizeof(kiln_string) &&
        (const char *)payload - sizeof(kiln_string) >= heap->nursery.base + sizeof(kiln_gc_block) &&
        (const char *)payload - sizeof(kiln_string) < heap->nursery.bump) {
        size_t block_offset = (size_t)((const char *)payload - sizeof(kiln_string) - sizeof(kiln_gc_block) - heap->nursery.base);
        if ((block_offset & 15) == 0) {
            size_t bit_idx = block_offset >> 4;
            if (bit_idx >> 6 < heap->nursery.bitmap_words &&
                (heap->nursery.bitmap[bit_idx >> 6] & ((uint64_t)1 << (bit_idx & 63))) != 0) {
                kiln_gc_block *block = (kiln_gc_block *)(heap->nursery.base + block_offset);
                if ((block->kind & KILN_GC_FLAG_DEAD) == 0 && (block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING) return block;
            }
        }
    }
    /* Promoted chunks check */
    for (const kiln_nursery_chunk *chunk = heap->nursery.promoted; chunk != NULL; chunk = chunk->next) {
        if ((const char *)payload >= chunk->base + sizeof(kiln_gc_block) &&
            (const char *)payload < chunk->limit) {
            size_t block_offset = (size_t)((const char *)payload - sizeof(kiln_gc_block) - chunk->base);
            if ((block_offset & 15) == 0) {
                size_t bit_idx = block_offset >> 4;
                if (bit_idx >> 6 < chunk->bitmap_words &&
                    (chunk->bitmap[bit_idx >> 6] & ((uint64_t)1 << (bit_idx & 63))) != 0) {
                    kiln_gc_block *block = (kiln_gc_block *)(chunk->base + block_offset);
                    if ((block->kind & KILN_GC_FLAG_DEAD) == 0) return block;
                }
            }
        }
        if ((uintptr_t)payload >= sizeof(kiln_string) &&
            (const char *)payload - sizeof(kiln_string) >= chunk->base + sizeof(kiln_gc_block) &&
            (const char *)payload - sizeof(kiln_string) < chunk->limit) {
            size_t block_offset = (size_t)((const char *)payload - sizeof(kiln_string) - sizeof(kiln_gc_block) - chunk->base);
            if ((block_offset & 15) == 0) {
                size_t bit_idx = block_offset >> 4;
                if (bit_idx >> 6 < chunk->bitmap_words &&
                    (chunk->bitmap[bit_idx >> 6] & ((uint64_t)1 << (bit_idx & 63))) != 0) {
                    kiln_gc_block *block = (kiln_gc_block *)(chunk->base + block_offset);
                    if ((block->kind & KILN_GC_FLAG_DEAD) == 0 && (block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING) return block;
                }
            }
        }
    }
    return NULL;
}

static kiln_gc_block *kiln_c99_gc_lookup(const kiln_gc_heap *heap, const void *payload) {
    kiln_gc_block *block = kiln_c99_gc_lookup_nursery(heap, payload);
    if (block != NULL) return block;
    block = kiln_c99_gc_lookup_slab(heap, payload, NULL);
    if (block != NULL) return block;
    if (payload != NULL && ((uintptr_t)payload % sizeof(void *)) == 0 && (uintptr_t)payload >= sizeof(kiln_string)) {
        const void *str_payload = (const char *)payload - sizeof(kiln_string);
        block = kiln_c99_gc_lookup_slab(heap, str_payload, NULL);
        if (block != NULL && (block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING) return block;
    }
    return NULL;
}

static uint32_t kiln_c99_gc_kind_of_ptr(const void *ptr) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || ptr == NULL) return 0;
    kiln_gc_block *block = kiln_c99_gc_lookup(heap, ptr);
    if (block == NULL) return 0;
    return block->kind & KILN_GC_KIND_MASK;
}

/* A branch-light ASCII test, 8 bytes at a time: ASCII is exactly the bytes with their top bit
 * clear, so a single mask-and-compare per word answers for all eight at once. The tail below
 * 8 bytes falls back to one byte at a time. */
static int kiln_c99_ascii_scan(const char *bytes, size_t len) {
    size_t index = 0;
    while (index + 8 <= len) {
        uint64_t word;
        memcpy(&word, bytes + index, sizeof word);
        if ((word & UINT64_C(0x8080808080808080)) != 0) return 0;
        index += 8;
    }
    while (index < len) {
        if (((unsigned char)bytes[index]) & 0x80u) return 0;
        ++index;
    }
    return 1;
}

/* Is this string value entirely ASCII, where a UTF-16 code unit is a byte and `length` is `len`
 * (ADR 0022)? A heap string (`kiln_c99_gc_lookup` finds its block) caches the answer in the
 * block's otherwise-unused `kind` high bits (ADR 0041) after the first scan, so a string read
 * more than once -- a repeated header name, a request path used by several route checks --
 * pays for the scan at most once. A constant or foreign pointer has no block to cache in and is
 * scanned every call; constants are short (path segments, JSON keys, literals a program wrote)
 * and the scan is cheap relative to the UTF-8 decode it replaces. */
static int kiln_c99_string_is_ascii(kiln_value value) {
    kiln_gc_heap *heap;
    kiln_gc_block *block;
    int ascii;
    if (value.len == 0) return 1;
    heap = kiln_c99_gc_heap();
    block = heap != NULL ? kiln_c99_gc_lookup(heap, value.ptr) : NULL;
    if (block != NULL && (block->kind & KILN_GC_FLAG_ASCII_KNOWN) != 0) {
        return (block->kind & KILN_GC_FLAG_ASCII) != 0;
    }
    ascii = kiln_c99_ascii_scan(value.ptr, value.len);
    if (block != NULL) {
        block->kind = (block->kind & KILN_GC_KIND_MASK) | KILN_GC_FLAG_ASCII_KNOWN |
                      (ascii ? KILN_GC_FLAG_ASCII : 0u);
    }
    return ascii;
}

static inline int kiln_c99_gc_is_tenured_kind(uint32_t kind) {
    uint32_t k = kind & KILN_GC_KIND_MASK;
    return k == KILN_GC_NODE_SERVER || k == KILN_GC_NODE_SOCKET ||
           k == KILN_GC_NODE_CLIENT_REQUEST || k == KILN_GC_NODE_RESPONSE ||
           k == KILN_GC_NODE_COMPLETION || k == KILN_GC_NODE_FS_COMPLETION ||
           k == KILN_GC_TIMER;
}

#include <setjmp.h>
#include <pthread.h>

/* The highest address of this thread's stack, from the thread itself: the realm runs on one
 * thread, so the base is found once and kept. Above it lie the launcher's and the runtime's
 * own frames, which scanning them cannot harm. */
static void *kiln_c99_gc_stack_base(kiln_gc_heap *heap) {
    if (heap->stack_base == NULL) {
#if defined(__APPLE__)
        heap->stack_base = pthread_get_stackaddr_np(pthread_self());
#else
        extern int pthread_getattr_np(pthread_t, pthread_attr_t *);
        pthread_attr_t attr;
        void *addr = NULL;
        size_t size = 0;
        if (pthread_getattr_np(pthread_self(), &attr) == 0) {
            if (pthread_attr_getstack(&attr, &addr, &size) == 0 && addr != NULL) {
                heap->stack_base = (char *)addr + size;
            }
            pthread_attr_destroy(&attr);
        }
#endif
    }
    return heap->stack_base;
}

/* The block whose start is the nearest set bit at or below `bit_idx` in a nursery bitmap, if
 * `address` falls inside it: blocks are 16-byte aligned and every start has its bit set. */
static kiln_gc_block *kiln_c99_gc_bitmap_block(const uint64_t *bitmap, size_t bitmap_words,
                                               const char *base, uintptr_t address) {
    size_t bit_idx = (size_t)((const char *)address - base) >> 4;
    while (1) {
        size_t word = bit_idx >> 6;
        uint64_t below = ((uint64_t)1 << (bit_idx & 63)) | (((uint64_t)1 << (bit_idx & 63)) - 1);
        uint64_t mask = word < bitmap_words ? bitmap[word] & below : 0;
        if (mask != 0) {
            size_t top = 63 - (size_t)__builtin_clzll(mask);
            kiln_gc_block *block = (kiln_gc_block *)(base + (((word << 6) + top) << 4));
            size_t payload = block->size == 0 ? 1 : block->size;
            if ((block->kind & KILN_GC_FLAG_DEAD) != 0) return NULL;
            if ((const char *)address < (const char *)block + sizeof(kiln_gc_block) + payload) return block;
            return NULL;
        }
        if (word == 0) return NULL;
        bit_idx = (word << 6) - 1;
    }
}

/* The live block that contains `address`, if any: an address anywhere inside a slab slot, a
 * nursery block, or a promoted chunk's block finds the block, which is what a register holding
 * a slots array or an element pointer needs. A large block is found by its payload address
 * only. A free slot or a dead nursery block is nothing. */
static kiln_gc_block *kiln_c99_gc_lookup_interior(const kiln_gc_heap *heap, uintptr_t address) {
    void *slab_base;
    const kiln_nursery_chunk *chunk;
    if (address < sizeof(kiln_gc_block)) return NULL;
    slab_base = (void *)(address & ~(uintptr_t)(KILN_GC_SLAB_SIZE - 1));
    if (kiln_c99_gc_ptrset_contains(&heap->slab_table, slab_base)) {
        const kiln_gc_slab *slab = (const kiln_gc_slab *)slab_base;
        size_t offset = address - (uintptr_t)slab_base;
        size_t slot_index;
        kiln_gc_block *block;
        if (offset < slab->first_slot_offset) return NULL;
        slot_index = (offset - slab->first_slot_offset) / slab->slot_size;
        if (slot_index >= slab->slot_count) return NULL;
        block = kiln_c99_gc_slab_slot(slab, (uint32_t)slot_index);
        if (block->size == KILN_GC_SLOT_FREE) return NULL;
        return block;
    }
    if (heap->nursery.active && (const char *)address >= heap->nursery.base &&
        (const char *)address < heap->nursery.bump) {
        return kiln_c99_gc_bitmap_block(heap->nursery.bitmap, heap->nursery.bitmap_words,
                                        heap->nursery.base, address);
    }
    for (chunk = heap->nursery.promoted; chunk != NULL; chunk = chunk->next) {
        if ((const char *)address >= chunk->base && (const char *)address < chunk->limit) {
            return kiln_c99_gc_bitmap_block(chunk->bitmap, chunk->bitmap_words, chunk->base, address);
        }
    }
    {
        kiln_gc_block *block = (kiln_gc_block *)(address - sizeof(kiln_gc_block));
        if (kiln_c99_gc_ptrset_contains(&heap->large_table, block)) return block;
    }
    return NULL;
}

static void kiln_c99_gc_mark_found(kiln_gc_heap *heap, kiln_gc_block *block);

/* One word of the stack: as a raw address, and as the payload of a boxed value when its top
 * bits are a box prefix. Either that lands inside a live block marks the block. */
static void kiln_c99_gc_mark_word(kiln_gc_heap *heap, uintptr_t word) {
    kiln_gc_block *block = kiln_c99_gc_lookup_interior(heap, word);
    if (block != NULL) kiln_c99_gc_mark_found(heap, block);
    if ((word & KILN_TAG_MASK) >= KILN_TAG_STRING_PREFIX) {
        block = kiln_c99_gc_lookup_interior(heap, word & UINT64_C(0x0000FFFFFFFFFFFF));
        if (block != NULL) kiln_c99_gc_mark_found(heap, block);
    }
}

/* Scans from its own frame, the lowest on the stack, up to the base: the caller has already
 * spilled the callee-saved registers into its frame, which lies above this one. */
__attribute__((noinline)) static void kiln_c99_gc_scan_from_here(kiln_gc_heap *heap) {
    volatile uintptr_t anchor = 0;
    uintptr_t *low = (uintptr_t *)((uintptr_t)&anchor & ~(uintptr_t)7);
    uintptr_t *high = (uintptr_t *)kiln_c99_gc_stack_base(heap);
    (void)anchor;
    if (high == NULL) return;
    for (; low < high; ++low) kiln_c99_gc_mark_word(heap, *low);
}

static void kiln_c99_gc_scan_stack(kiln_gc_heap *heap) {
    jmp_buf registers;
    size_t index;
    /* Both, so the callee-saved registers are on the stack whatever the platform's setjmp
     * keeps or mangles. */
#if defined(__GNUC__) || defined(__clang__)
    __builtin_unwind_init();
#endif
    if (setjmp(registers) == 0) {
        for (index = 0; index + sizeof(uintptr_t) <= sizeof registers; index += sizeof(uintptr_t)) {
            uintptr_t word;
            memcpy(&word, (const char *)registers + index, sizeof word);
            kiln_c99_gc_mark_word(heap, word);
        }
        kiln_c99_gc_scan_from_here(heap);
    }
}

/* A call site that remembers a holder registers itself once; after a collection the entries
 * whose holder or intermediate died are cleared. */
static void kiln_c99_gc_register_site(kiln_c99_site *site) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || site == NULL || site->registered) return;
    if (heap->site_len == heap->site_cap) {
        size_t wanted = heap->site_cap == 0 ? 64 : heap->site_cap * 2;
        kiln_c99_site **grown = (kiln_c99_site **)realloc(heap->sites, wanted * sizeof *grown);
        if (grown == NULL) return;
        heap->sites = grown;
        heap->site_cap = wanted;
    }
    heap->sites[heap->site_len++] = site;
    site->registered = 1;
}

static void kiln_c99_gc_clear_dead_sites(kiln_gc_heap *heap) {
    size_t index;
    for (index = 0; index < heap->site_len; ++index) {
        kiln_c99_site *site = heap->sites[index];
        uint32_t p;
        for (p = 0; p < site->pic_count; ++p) {
            kiln_c99_pic_entry *entry = &site->entries[p];
            if (entry->holder == NULL) continue;
            if (kiln_c99_gc_lookup_interior(heap, (uintptr_t)entry->holder) == NULL ||
                (entry->via != NULL && kiln_c99_gc_lookup_interior(heap, (uintptr_t)entry->via) == NULL)) {
                entry->holder = NULL;
                entry->holder_shape = NULL;
                entry->holder_key = NULL;
                entry->via = NULL;
                entry->via_shape = NULL;
                entry->holder_slot = UINT32_MAX;
                entry->holder_val = 0;
            }
        }
    }
}

static void kiln_c99_gc_collect(void *opaque, int force);

/* A collection from inside a turn, at an allocation. */
static void kiln_c99_gc_collect_mid_turn(kiln_gc_heap *heap) {
    heap->mid_turn = 1;
    kiln_c99_gc_collect(heap, 1);
    heap->mid_turn = 0;
}

static void *kiln_c99_gc_alloc(uint32_t kind, size_t size) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t payload = size == 0 ? 1 : size;
    int class_index;
    kiln_gc_block *block;
    if (heap == NULL || size > ((size_t)-1) - sizeof(kiln_gc_block)) return NULL;
    if (KILN_UNLIKELY(!heap->collecting && heap->enabled && !heap->fallback)) {
        /* A mid-turn collection scans the whole stack and sweeps every slab, so it earns its
         * keep only after a few megabytes, or after allocation has matched twice what
         * survived the last one; the turn's end keeps its own, lower, threshold. */
        size_t threshold = heap->survived * 2;
        int due;
        if (threshold < KILN_GC_MID_TURN_FLOOR) threshold = KILN_GC_MID_TURN_FLOOR;
        due = heap->since_collection >= threshold;
        if (heap->stress_every != 0 && --heap->stress_countdown == 0) {
            heap->stress_countdown = heap->stress_every;
            due = 1;
        }
        if (due) kiln_c99_gc_collect_mid_turn(heap);
    }

    if (!kiln_c99_gc_is_tenured_kind(kind) && heap->nursery.active && (payload + sizeof(kiln_gc_block) <= (heap->nursery.capacity >> 2))) {
        size_t total = sizeof(kiln_gc_block) + payload;
        total = (total + 15) & ~((size_t)15);
        if (heap->nursery.bump + total <= heap->nursery.limit) {
            block = (kiln_gc_block *)heap->nursery.bump;
            size_t offset = (size_t)(heap->nursery.bump - heap->nursery.base);
            size_t bit_idx = offset >> 4;
            heap->nursery.bitmap[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
            heap->nursery.bump += total;
            block->kind = kind | KILN_GC_FLAG_NURSERY;
            block->mark = 0;
            block->size = size;
            block->prev = NULL;
            block->next = NULL;
            heap->nursery.count += 1;
            heap->nursery.bytes += size;
            heap->count += 1;
            heap->bytes += size;
            heap->since_collection += size;
            memset(block + 1, 0, payload);
            return block + 1;
        }
    }
    class_index = kiln_c99_gc_slab_class(sizeof(kiln_gc_block) + payload);
    if (class_index >= 0) {
        kiln_gc_slab *slab = heap->class_free_head[class_index];
        if (slab == NULL) {
            slab = kiln_c99_gc_slab_new(heap, class_index);
            if (slab == NULL) return NULL;
        }
        block = slab->free_head;
        slab->free_head = block->next;
        block->next = NULL;
        if (--slab->free_count == 0) kiln_c99_gc_slab_unlink_free(heap, slab);
        block->kind = kind;
        block->mark = 0;
        block->size = size;
        heap->count += 1;
        heap->bytes += size;
        heap->since_collection += size;
        /* Zeroed like a nursery block: a collection can now run before the caller has filled
         * the payload in, and the tracer must not read a previous occupant's pointers. */
        memset(block + 1, 0, payload);
        return block + 1;
    }
    block = (kiln_gc_block *)malloc(sizeof *block + payload);
    if (block == NULL) return NULL;
    if (kiln_c99_gc_ptrset_insert(&heap->large_table, block) != KILN_C99_OK) {
        free(block);
        return NULL;
    }
    block->kind = kind;
    block->mark = 0;
    block->size = size;
    block->prev = NULL;
    block->next = heap->head;
    if (heap->head != NULL) heap->head->prev = block;
    heap->head = block;
    ++heap->count;
    heap->bytes += size;
    heap->since_collection += size;
    return block + 1;
}

static void *kiln_c99_gc_alloc_tenured(uint32_t kind, size_t size) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t payload = size == 0 ? 1 : size;
    int class_index;
    kiln_gc_block *block;
    if (heap == NULL || size > ((size_t)-1) - sizeof(kiln_gc_block)) return NULL;
    class_index = kiln_c99_gc_slab_class(sizeof(kiln_gc_block) + payload);
    if (class_index >= 0) {
        kiln_gc_slab *slab = heap->class_free_head[class_index];
        if (slab == NULL) {
            slab = kiln_c99_gc_slab_new(heap, class_index);
            if (slab == NULL) return NULL;
        }
        block = slab->free_head;
        slab->free_head = block->next;
        block->next = NULL;
        if (--slab->free_count == 0) kiln_c99_gc_slab_unlink_free(heap, slab);
        block->kind = kind;
        block->mark = 0;
        block->size = size;
        heap->count += 1;
        heap->bytes += size;
        heap->since_collection += size;
        return block + 1;
    }
    block = (kiln_gc_block *)malloc(sizeof *block + payload);
    if (block == NULL) return NULL;
    if (kiln_c99_gc_ptrset_insert(&heap->large_table, block) != KILN_C99_OK) {
        free(block);
        return NULL;
    }
    block->kind = kind;
    block->mark = 0;
    block->size = size;
    block->prev = NULL;
    block->next = heap->head;
    if (heap->head != NULL) heap->head->prev = block;
    heap->head = block;
    ++heap->count;
    heap->bytes += size;
    heap->since_collection += size;
    return block + 1;
}

static void *kiln_c99_gc_calloc(uint32_t kind, size_t size) {
    void *payload = kiln_c99_gc_alloc(kind, size);
    if (payload != NULL && size != 0) memset(payload, 0, size);
    return payload;
}

/* ---- Interned keys ------------------------------------------------------------------------
 *
 * Three entry points, and the difference between them is the whole design:
 *
 *   kiln_c99_intern        canonical pointer, inserting. The one a *write* uses.
 *   kiln_c99_intern_static canonical pointer, inserting, for bytes that never die; pins.
 *   kiln_c99_interned      canonical pointer or NULL, never inserting. The one a *read* uses,
 *                          because storage is total: a spelling the table has never seen is a
 *                          key no object anywhere has, and the read can skip the scan.
 *
 * Canonical pointers are distinct as pointers: a block is adopted as canonical only when its
 * size is exactly the key's length, so two keys can never share an address with different
 * lengths, and the empty key is one static byte rather than whichever empty string came first. */
static const char kiln_c99_key_empty[1] = "";
static void kiln_c99_gc_mark_block(kiln_gc_heap *heap, const void *payload);

/* `KILN_KEYS_CHECK=1`: every pointer-identity scan is re-run by bytes and a disagreement aborts
 * naming the key. The acceptance test for "interning is total", not a mode to ship. Read once
 * per unit rather than kept on the heap, so a compiled site that never touches the heap is
 * checked too. */
static int kiln_c99_keys_check_mode = -1;

static int kiln_c99_keys_checked(void) {
    if (kiln_c99_keys_check_mode < 0) {
        const char *setting = getenv("KILN_KEYS_CHECK");
        kiln_c99_keys_check_mode = setting != NULL && setting[0] != '\0' && setting[0] != '0';
    }
    return kiln_c99_keys_check_mode;
}

static uint64_t kiln_c99_key_hash(const char *bytes, size_t len) {
    /* Eight bytes at a time, because nearly every key is shorter than sixteen.
     *
     * FNV-1a walked a byte per iteration and each multiply waited on the one before it, so a
     * ten-character header name cost ten dependent multiplies before the probe could start.
     * Interning was 4.79% of the mixed server workload with `kiln_c99_intern` the second most
     * expensive function in the prelude, and this is most of what it was doing: the keys that
     * reach it are the ones off the wire, hashed fresh on every request.
     *
     * The mixing constant and the shift are murmur3's finaliser, which is where the avalanche
     * comes from -- the table masks the low bits, so a hash whose low bits track the input would
     * lengthen every probe chain. `len` seeds it so that keys differing only by trailing bytes
     * that fall outside the last word still separate. Nothing persists a hash: it is used to
     * probe and to rehash, both within one process, so changing it changes no artifact. */
    const uint64_t mix = UINT64_C(0xff51afd7ed558ccd);
    uint64_t hash = UINT64_C(0x9e3779b97f4a7c15) ^ (uint64_t)len;
    size_t index = 0;
    while (len - index >= 8) {
        uint64_t word;
        memcpy(&word, bytes + index, sizeof word);
        hash = (hash ^ word) * mix;
        hash ^= hash >> 29;
        index += 8;
    }
    if (index < len) {
        /* The tail goes into a zeroed word, so a shorter key never reads past its own bytes. */
        uint64_t tail = 0;
        memcpy(&tail, bytes + index, len - index);
        hash = (hash ^ tail) * mix;
        hash ^= hash >> 29;
    }
    return hash ^ (hash >> 32);
}

static kiln_key_entry *kiln_c99_key_probe(const kiln_key_table *table, const char *bytes,
                                          size_t len, uint64_t hash) {
    size_t mask;
    size_t at;
    if (table->capacity == 0) return NULL;
    mask = table->capacity - 1;
    at = (size_t)hash & mask;
    for (;;) {
        kiln_key_entry *entry = &table->entries[at];
        if (entry->bytes == NULL) return NULL;
        if (entry->hash == hash && entry->len == len &&
            (entry->bytes == bytes || memcmp(entry->bytes, bytes, len) == 0))
            return entry;
        at = (at + 1) & mask;
    }
}

static void kiln_c99_key_place(kiln_key_table *table, kiln_key_entry entry) {
    size_t mask = table->capacity - 1;
    size_t at = (size_t)entry.hash & mask;
    while (table->entries[at].bytes != NULL) at = (at + 1) & mask;
    table->entries[at] = entry;
    table->count += 1;
}

static int kiln_c99_key_grow(kiln_key_table *table) {
    kiln_key_entry *old = table->entries;
    size_t old_capacity = table->capacity;
    size_t capacity = old_capacity == 0 ? 256 : old_capacity * 2;
    kiln_key_entry *grown;
    size_t index;
    if (capacity > ((size_t)-1) / sizeof *grown) return KILN_C99_OUT_OF_MEMORY;
    grown = (kiln_key_entry *)calloc(capacity, sizeof *grown);
    if (grown == NULL) return KILN_C99_OUT_OF_MEMORY;
    table->entries = grown;
    table->capacity = capacity;
    table->count = 0;
    for (index = 0; index < old_capacity; ++index)
        if (old[index].bytes != NULL) kiln_c99_key_place(table, old[index]);
    free(old);
    return KILN_C99_OK;
}

static const char *kiln_c99_key_insert(kiln_key_table *table, const char *bytes, size_t len,
                                       uint64_t hash, int pinned) {
    kiln_key_entry entry;
    if ((table->count + 1) * 2 > table->capacity && kiln_c99_key_grow(table) != KILN_C99_OK)
        return NULL;
    entry.bytes = bytes;
    entry.len = len;
    entry.hash = hash;
    entry.pinned = pinned;
    kiln_c99_key_place(table, entry);
    return bytes;
}

/* Linear probing deletes by shifting the entries after the hole back over it: an entry moves
 * into the hole when the hole lies on the probe path from its home slot to where it sits. */
static void kiln_c99_key_delete_at(kiln_key_table *table, size_t hole) {
    size_t mask = table->capacity - 1;
    size_t at = (hole + 1) & mask;
    table->entries[hole].bytes = NULL;
    table->count -= 1;
    while (table->entries[at].bytes != NULL) {
        size_t home = (size_t)table->entries[at].hash & mask;
        if (((hole - home) & mask) < ((at - home) & mask)) {
            table->entries[hole] = table->entries[at];
            table->entries[at].bytes = NULL;
            hole = at;
        }
        at = (at + 1) & mask;
    }
}

static const char *kiln_c99_interned_in(kiln_gc_heap *heap, const char *bytes, size_t len) {
    const kiln_key_entry *entry;
    if (len == 0) return kiln_c99_key_empty;
    if (heap == NULL) return NULL;
    entry = kiln_c99_key_probe(&heap->keys, bytes, len, kiln_c99_key_hash(bytes, len));
    return entry == NULL ? NULL : entry->bytes;
}

static const char *kiln_c99_interned(const char *bytes, size_t len) {
    return kiln_c99_interned_in(kiln_c99_gc_heap(), bytes, len);
}

static const char *kiln_c99_intern(const char *bytes, size_t len) {
    kiln_gc_heap *heap;
    uint64_t hash;
    kiln_key_entry *entry;
    char *copy;
    if (len == 0) return kiln_c99_key_empty;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    hash = kiln_c99_key_hash(bytes, len);
    entry = kiln_c99_key_probe(&heap->keys, bytes, len, hash);
    if (entry != NULL) return entry->bytes;
    /* A key must be null-terminated because shape keys and property lookups call strlen(ikey),
     * and string blocks are not otherwise null-terminated. We allocate len + 1 bytes. */
    copy = (char *)kiln_c99_gc_alloc_tenured(KILN_GC_STRING, len + 1);
    if (copy == NULL) return NULL;
    memcpy(copy, bytes, len);
    copy[len] = '\0';
    return kiln_c99_key_insert(&heap->keys, copy, len, hash, 0);
}

/* For bytes that outlive the heap. NULL only when the table cannot grow; a unit that gets NULL
 * for one of its keys fails its entry with out-of-memory rather than run a site with a pointer
 * that is not canonical. */
static const char *kiln_c99_intern_static(const char *bytes, size_t len) {
    kiln_gc_heap *heap;
    uint64_t hash;
    kiln_key_entry *entry;
    if (len == 0) return kiln_c99_key_empty;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    hash = kiln_c99_key_hash(bytes, len);
    entry = kiln_c99_key_probe(&heap->keys, bytes, len, hash);
    if (entry != NULL) {
        entry->pinned = 1;
        return entry->bytes;
    }
    return kiln_c99_key_insert(&heap->keys, bytes, len, hash, 1);
}

/* A string block released by hand (`kiln_c99_gc_release`) must not stay canonical. */
static void kiln_c99_key_forget(kiln_gc_heap *heap, const char *bytes, size_t len) {
    kiln_key_table *table = &heap->keys;
    size_t mask;
    size_t at;
    if (table->capacity == 0) return;
    mask = table->capacity - 1;
    at = (size_t)kiln_c99_key_hash(bytes, len) & mask;
    while (table->entries[at].bytes != NULL) {
        if (table->entries[at].bytes == bytes) {
            kiln_c99_key_delete_at(table, at);
            return;
        }
        at = (at + 1) & mask;
    }
}

static void kiln_c99_key_mark_pinned(kiln_gc_heap *heap) {
    size_t index;
    for (index = 0; index < heap->keys.capacity; ++index) {
        const kiln_key_entry *entry = &heap->keys.entries[index];
        if (entry->bytes != NULL && entry->pinned) kiln_c99_gc_mark_block(heap, entry->bytes);
    }
}

/* After marking and before the sweep: drop every weak entry whose block did not survive. In
 * place, so a purge needs no memory at the one moment the collector must not fail for lack of
 * it. A slot is re-examined after a deletion because the shift moves a later entry into it. */
static void kiln_c99_key_purge(kiln_gc_heap *heap) {
    kiln_key_table *table = &heap->keys;
    size_t index = 0;
    while (index < table->capacity) {
        const kiln_key_entry *entry = &table->entries[index];
        kiln_gc_block *block;
        if (entry->bytes == NULL || entry->pinned) {
            ++index;
            continue;
        }
        block = kiln_c99_gc_lookup(heap, entry->bytes);
        if (block == NULL || block->mark != heap->epoch) {
            kiln_c99_key_delete_at(table, index);
            continue;
        }
        ++index;
    }
}

static void kiln_c99_gc_unlink(kiln_gc_heap *heap, kiln_gc_block *block) {
    if ((block->kind & KILN_GC_FLAG_NURSERY) != 0) return;
    if (block->prev != NULL) block->prev->next = block->next;
    else heap->head = block->next;
    if (block->next != NULL) block->next->prev = block->prev;
    block->prev = NULL;
    block->next = NULL;
    --heap->count;
    heap->bytes -= block->size;
}

static void kiln_c99_gc_finalize(kiln_gc_block *block);

/* Explicit release for a block whose lifetime is already known: queued work after it ran, a
 * host-operation completion after its callback. The collector remains the backstop. */
static void kiln_c99_gc_release(void *payload) {
    kiln_gc_heap *heap;
    kiln_gc_slab *slab = NULL;
    kiln_gc_block *block;
    if (payload == NULL) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    block = kiln_c99_gc_lookup_nursery(heap, payload);
    if (block == NULL && ((uintptr_t)payload % sizeof(void*) == 0) && (uintptr_t)payload >= sizeof(kiln_string)) {
        block = kiln_c99_gc_lookup_nursery(heap, (char *)payload - sizeof(kiln_string));
        if (block != NULL) payload = (char *)payload - sizeof(kiln_string);
    }
    if (block != NULL) {
        if ((block->kind & KILN_GC_FLAG_DEAD) == 0) {
            if ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING)
                kiln_c99_key_forget(heap, (const char *)payload, block->size);
            kiln_c99_gc_finalize(block);
            block->kind |= KILN_GC_FLAG_DEAD;
            kiln_c99_nursery_clear_bit(heap, block);
            heap->freed_blocks += 1;
            heap->freed_bytes += block->size;
            heap->count -= 1;
            heap->bytes -= block->size;
        }
        return;
    }
    block = kiln_c99_gc_lookup_slab(heap, payload, &slab);
    if (block == NULL && ((uintptr_t)payload % sizeof(void*) == 0) && (uintptr_t)payload >= sizeof(kiln_string)) {
        block = kiln_c99_gc_lookup_slab(heap, (char *)payload - sizeof(kiln_string), &slab);
        if (block != NULL) payload = (char *)payload - sizeof(kiln_string);
    }
    if (block == NULL) return;
    if ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING)
        kiln_c99_key_forget(heap, (const char *)payload, block->size);
    kiln_c99_gc_finalize(block);
    heap->freed_blocks += 1;
    heap->freed_bytes += block->size;
    if (slab != NULL) {
        heap->count -= 1;
        heap->bytes -= block->size;
        block->size = KILN_GC_SLOT_FREE;
        block->kind = KILN_GC_NONE;
        block->next = slab->free_head;
        slab->free_head = block;
        if (++slab->free_count == 1) kiln_c99_gc_slab_link_free(heap, slab);
        if (slab->free_count == slab->slot_count) kiln_c99_gc_slab_maybe_destroy(heap, slab);
    } else {
        kiln_c99_gc_unlink(heap, block);
        kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        free(block);
    }
}

static void kiln_c99_gc_mark_found(kiln_gc_heap *heap, kiln_gc_block *block) {
    if (block == NULL || block->mark == heap->epoch) return;
    block->mark = heap->epoch;
    if (heap->stack_len == heap->stack_capacity) {
        size_t wanted = heap->stack_capacity == 0 ? 256 : heap->stack_capacity * 2;
        kiln_gc_block **grown =
            (kiln_gc_block **)realloc(heap->stack, wanted * sizeof *heap->stack);
        if (grown == NULL) {
            heap->mark_failed = 1;
            return;
        }
        heap->stack = grown;
        heap->stack_capacity = wanted;
    }
    heap->stack[heap->stack_len++] = block;
}

static void kiln_c99_gc_mark_block(kiln_gc_heap *heap, const void *payload) {
    kiln_gc_block *block = kiln_c99_gc_lookup(heap, payload);
    if (block == NULL || block->mark == heap->epoch) return;
    block->mark = heap->epoch;
    if (heap->stack_len == heap->stack_capacity) {
        size_t wanted = heap->stack_capacity == 0 ? 256 : heap->stack_capacity * 2;
        kiln_gc_block **grown =
            (kiln_gc_block **)realloc(heap->stack, wanted * sizeof *heap->stack);
        if (grown == NULL) {
            /* A collection that cannot record what it saw must not free anything. */
            heap->mark_failed = 1;
            return;
        }
        heap->stack = grown;
        heap->stack_capacity = wanted;
    }
    heap->stack[heap->stack_len++] = block;
}

static void kiln_c99_gc_mark_value(kiln_gc_heap *heap, const kiln_value *value) {
    /* Every payload-carrying tag keeps its pointer in the shared word, and a pointer the heap
     * does not know (a constant, a well-known symbol, a provider handle) is simply not marked. */
    if (value == NULL || value->tag < KILN_VALUE_STRING) return;
    kiln_c99_gc_mark_block(heap, (const void *)(uintptr_t)value->bits);
}

static void kiln_c99_gc_mark_values(kiln_gc_heap *heap, const kiln_value *values,
                                    size_t count) {
    size_t index;
    if (values == NULL) return;
    for (index = 0; index < count; ++index) kiln_c99_gc_mark_value(heap, &values[index]);
}

static void kiln_c99_gc_mark_val(kiln_gc_heap *heap, kiln_val v) {
    if (KILN_VAL_IS_NUMBER(v) || v < KILN_TAG_STRING_PREFIX) return;
    kiln_c99_gc_mark_block(heap, (const void *)KILN_VAL_PTR(v));
}

static void kiln_c99_gc_mark_vals(kiln_gc_heap *heap, const kiln_val *values,
                                  size_t count) {
    size_t index;
    if (values == NULL) return;
    for (index = 0; index < count; ++index) kiln_c99_gc_mark_val(heap, values[index]);
}

static void kiln_c99_gc_trace(kiln_gc_heap *heap, kiln_gc_block *block) {
    void *payload = block + 1;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_OBJECT: {
        kiln_object *object = (kiln_object *)payload;
        size_t index;
        if (object->shape != NULL) {
            for (index = 0; index < object->shape->property_count; ++index) {
                kiln_c99_gc_mark_block(heap, object->shape->keys[index]);
                if (object->slots != NULL) {
                    kiln_c99_gc_mark_val(heap, object->slots[index]);
                }
            }
        }
        if (object->elements != NULL) {
            kiln_c99_gc_mark_vals(heap, object->elements, object->element_count);
        }
        for (index = 0; index < object->count; ++index) {
            const kiln_property *property = &object->properties[index];
            kiln_c99_gc_mark_block(heap, property->key);
            kiln_c99_gc_mark_value(heap, &property->value);
            kiln_c99_gc_mark_value(heap, &property->getter);
            kiln_c99_gc_mark_value(heap, &property->setter);
        }
        kiln_c99_gc_mark_value(heap, &object->prototype);
        break;
    }
    case KILN_GC_CLOSURE: {
        kiln_closure *closure = (kiln_closure *)payload;
        kiln_c99_gc_mark_vals(heap, closure->upvalues, closure->upvalue_count);
        kiln_c99_gc_mark_val(heap, closure->captured_this);
        kiln_c99_gc_mark_val(heap, closure->captured_new_target);
        kiln_c99_gc_mark_value(heap, &closure->captured_arguments);
        kiln_c99_gc_mark_value(heap, &closure->prototype);
        kiln_c99_gc_mark_value(heap, &closure->properties);
        break;
    }
    case KILN_GC_CELL:
        kiln_c99_gc_mark_val(heap, *(const kiln_val *)payload);
        break;
    case KILN_GC_ARRAY: {
        kiln_array *array = (kiln_array *)payload;
        kiln_c99_gc_mark_vals(heap, array->elements, array->count);
        kiln_c99_gc_mark_value(heap, &array->properties);
        kiln_c99_gc_mark_value(heap, &array->match_input);
        break;
    }
    case KILN_GC_GENERATOR: {
        kiln_generator *frame = (kiln_generator *)payload;
        kiln_c99_gc_mark_value(heap, &frame->function);
        kiln_c99_gc_mark_value(heap, &frame->receiver);
        kiln_c99_gc_mark_value(heap, &frame->promise);
        kiln_c99_gc_mark_value(heap, &frame->sent);
        if (frame->function.tag == KILN_VALUE_FUNCTION) {
            const kiln_closure *closure = (const kiln_closure *)(uintptr_t)frame->function.bits;
            kiln_c99_gc_mark_vals(heap, frame->locals, closure->local_count);
            kiln_c99_gc_mark_vals(heap, frame->stack, closure->stack_size);
        }
        if (frame->queue_promises != NULL) {
            kiln_c99_gc_mark_values(heap, frame->queue_promises, frame->queue_count);
            kiln_c99_gc_mark_values(heap, frame->queue_sent, frame->queue_count);
        }
        break;
    }
    case KILN_GC_PROMISE: {
        kiln_promise *promise = (kiln_promise *)payload;
        size_t index;
        kiln_c99_gc_mark_value(heap, &promise->value);
        kiln_c99_gc_mark_value(heap, &promise->properties);
        for (index = 0; index < promise->count; ++index) {
            kiln_c99_gc_mark_value(heap, &promise->reactions[index].on_fulfilled);
            kiln_c99_gc_mark_value(heap, &promise->reactions[index].on_rejected);
            kiln_c99_gc_mark_value(heap, &promise->reactions[index].derived);
        }
        break;
    }
    case KILN_GC_SETTLER:
        kiln_c99_gc_mark_value(heap, &((kiln_settler *)payload)->promise);
        break;
    case KILN_GC_BOUND: {
        kiln_bound *bound = (kiln_bound *)payload;
        kiln_c99_gc_mark_value(heap, &bound->target);
        kiln_c99_gc_mark_value(heap, &bound->receiver);
        kiln_c99_gc_mark_values(heap, bound->arguments, bound->count);
        kiln_c99_gc_mark_value(heap, &bound->properties);
        break;
    }
    case KILN_GC_COLLECTION: {
        kiln_collection *collection = (kiln_collection *)payload;
        size_t index;
        for (index = 0; index < collection->count; ++index) {
            if (collection->entries[index].dead) continue;
            kiln_c99_gc_mark_value(heap, &collection->entries[index].key);
            kiln_c99_gc_mark_value(heap, &collection->entries[index].value);
        }
        kiln_c99_gc_mark_value(heap, &collection->properties);
        break;
    }
    case KILN_GC_PROXY:
        kiln_c99_gc_mark_value(heap, &((kiln_proxy *)payload)->target);
        kiln_c99_gc_mark_value(heap, &((kiln_proxy *)payload)->handler);
        break;
    case KILN_GC_TYPED_ARRAY:
        kiln_c99_gc_mark_block(heap, ((kiln_typed_array *)payload)->buffer);
        break;
    case KILN_GC_DATA_VIEW:
        kiln_c99_gc_mark_block(heap, ((kiln_data_view *)payload)->buffer);
        break;
    case KILN_GC_METHOD_STORE:
        kiln_c99_gc_mark_value(heap, &((kiln_method_store *)payload)->properties);
        kiln_c99_gc_mark_block(heap, ((kiln_method_store *)payload)->next);
        break;
    case KILN_GC_NODE_SERVER: {
        kiln_node_server *server = (kiln_node_server *)payload;
        kiln_c99_gc_mark_value(heap, &server->object);
        kiln_c99_gc_mark_value(heap, &server->connection_listener);
        kiln_c99_gc_mark_block(heap, server->sockets);
        break;
    }
    case KILN_GC_NODE_SOCKET: {
        kiln_node_socket *socket = (kiln_node_socket *)payload;
        kiln_c99_gc_mark_value(heap, &socket->object);
        kiln_c99_gc_mark_block(heap, socket->next);
        kiln_c99_gc_mark_block(heap, socket->server);
        kiln_c99_gc_mark_block(heap, socket->client_requests);
        kiln_c99_gc_mark_block(heap, socket->responses);
        kiln_c99_gc_mark_block(heap, socket->recycled_response);
        break;
    }
    case KILN_GC_NODE_CLIENT_REQUEST: {
        kiln_node_client_request *request = (kiln_node_client_request *)payload;
        kiln_c99_gc_mark_value(heap, &request->object);
        kiln_c99_gc_mark_value(heap, &request->callback);
        kiln_c99_gc_mark_value(heap, &request->response);
        kiln_c99_gc_mark_block(heap, request->socket);
        kiln_c99_gc_mark_block(heap, request->next);
        break;
    }
    case KILN_GC_NODE_RESPONSE: {
        kiln_node_response *response = (kiln_node_response *)payload;
        kiln_c99_gc_mark_value(heap, &response->request);
        kiln_c99_gc_mark_value(heap, &response->object);
        kiln_c99_gc_mark_value(heap, &response->headers);
        kiln_c99_gc_mark_value(heap, &response->url_object);
        kiln_c99_gc_mark_value(heap, &response->search_params_object);
        kiln_c99_gc_mark_value(heap, &response->pathname_val);
        kiln_c99_gc_mark_value(heap, &response->query_val);
        kiln_c99_gc_mark_value(heap, &response->param_val);
        kiln_c99_gc_mark_block(heap, response->socket);
        kiln_c99_gc_mark_block(heap, response->next);
        break;
    }
    case KILN_GC_NODE_COMPLETION: {
        kiln_node_completion *completion = (kiln_node_completion *)payload;
        kiln_c99_gc_mark_block(heap, completion->server);
        kiln_c99_gc_mark_block(heap, completion->socket);
        kiln_c99_gc_mark_block(heap, completion->client_request);
        break;
    }
    case KILN_GC_NODE_FS_COMPLETION:
        kiln_c99_gc_mark_value(heap, &((kiln_node_fs_completion *)payload)->promise);
        break;
    case KILN_GC_MICROTASK: {
        kiln_microtask *task = (kiln_microtask *)payload;
        kiln_c99_gc_mark_value(heap, &task->handler);
        kiln_c99_gc_mark_value(heap, &task->argument);
        kiln_c99_gc_mark_value(heap, &task->derived);
        break;
    }
    case KILN_GC_NEXT_TICK: {
        kiln_next_tick *tick = (kiln_next_tick *)payload;
        kiln_c99_gc_mark_value(heap, &tick->callback);
        kiln_c99_gc_mark_values(heap, tick->args, tick->argc);
        break;
    }
    case KILN_GC_TIMER: {
        kiln_timer *timer = (kiln_timer *)payload;
        kiln_c99_gc_mark_value(heap, &timer->callback);
        kiln_c99_gc_mark_values(heap, timer->args, timer->argc);
        break;
    }
    case KILN_GC_REGEXP: {
        /* A pattern borrows the bytes it was compiled from, which may be a unit constant or a
         * string the program built; the block table decides which. The provider handle is not a
         * block and is released by the finaliser instead. */
        kiln_regexp *regexp = (kiln_regexp *)payload;
        size_t ci;
        kiln_c99_gc_mark_block(heap, regexp->source);
        kiln_c99_gc_mark_block(heap, regexp->flags);
        for (ci = 0; ci < regexp->cached_group_count && ci < 32; ++ci) {
            kiln_c99_gc_mark_val(heap, regexp->cached_elements[ci]);
        }
        break;
    }
    case KILN_GC_SYMBOL: {
        /* The reserved key is malloc-backed and outlives the heap, but a description can be a
         * collected string (`Symbol(String(x))`), so both are offered to the table. */
        kiln_symbol *symbol = (kiln_symbol *)payload;
        kiln_c99_gc_mark_block(heap, symbol->key);
        kiln_c99_gc_mark_block(heap, symbol->description);
        break;
    }
    case KILN_GC_STRING:
    case KILN_GC_BYTES:
    case KILN_GC_DATE:
    case KILN_GC_TYPED_BUFFER:
    case KILN_GC_COMMONJS_REQUIRE:
    default:
        break;
    }
}

/* Inner growth buffers are the owner's, never a value's, so they are released here and only
 * here. A kind whose payload owns a provider handle releases it here too. */
static void kiln_c99_gc_finalize(kiln_gc_block *block) {
    void *payload = block + 1;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_OBJECT:
        free(((kiln_object *)payload)->properties);
        if (((kiln_object *)payload)->slots != ((kiln_object *)payload)->inline_slots) {
            free(((kiln_object *)payload)->slots);
        }
        free(((kiln_object *)payload)->elements);
        break;
    case KILN_GC_CLOSURE:
        free(((kiln_closure *)payload)->upvalues);
        break;
    case KILN_GC_ARRAY: {
        kiln_array *arr = (kiln_array *)payload;
        if (arr->elements != NULL && arr->elements != arr->inline_elements) {
            free(arr->elements);
        }
        break;
    }
    case KILN_GC_GENERATOR: {
        kiln_generator *frame = (kiln_generator *)payload;
        free(frame->locals);
        free(frame->stack);
        free(frame->handlers);
        free(frame->handler_stack);
        free(frame->queue_promises);
        free(frame->queue_sent);
        break;
    }
    case KILN_GC_PROMISE:
        free(((kiln_promise *)payload)->reactions);
        break;
    case KILN_GC_BOUND:
        free(((kiln_bound *)payload)->arguments);
        break;
    case KILN_GC_COLLECTION:
        free(((kiln_collection *)payload)->entries);
        free(((kiln_collection *)payload)->hash_table);
        break;
    case KILN_GC_TYPED_BUFFER:
        free(((kiln_typed_buffer *)payload)->bytes);
        break;
    case KILN_GC_REGEXP:
        /* The compiled pattern is the provider's, and this is the only place it is handed back. */
        kiln_regexp_free(((kiln_regexp *)payload)->handle);
        break;
    case KILN_GC_NODE_SERVER:
        free(((kiln_node_server *)payload)->address);
        break;
    case KILN_GC_NODE_RESPONSE: {
        kiln_node_response *response = (kiln_node_response *)payload;
        if (response->request_headers_owned && response->request_headers != NULL) {
            free(response->request_headers);
            response->request_headers = NULL;
        }
        break;
    }
    case KILN_GC_NODE_CLIENT_REQUEST: {
        kiln_node_client_request *request = (kiln_node_client_request *)payload;
        free(request->host_name);
        free(request->path);
        free(request->method);
        free(request->headers);
        free(request->pending_body);
        break;
    }
    case KILN_GC_NEXT_TICK:
        free(((kiln_next_tick *)payload)->args);
        break;
    case KILN_GC_TIMER:
        free(((kiln_timer *)payload)->args);
        break;
    default:
        break;
    }
}

static void kiln_c99_gc_drain(kiln_gc_heap *heap) {
    while (heap->stack_len != 0 && !heap->mark_failed) {
        kiln_gc_block *block = heap->stack[--heap->stack_len];
        kiln_c99_gc_trace(heap, block);
    }
}

static void kiln_c99_gc_begin(kiln_gc_heap *heap) {
    ++heap->epoch;
    if (heap->epoch == 0) ++heap->epoch;
    heap->stack_len = 0;
    heap->mark_failed = 0;
}

/* Free every unmarked block. Marked blocks either stay (an ordinary collection) or are orphaned
 * (teardown, where a result a caller still borrows must outlive the heap).
 *
 * Large blocks are unchanged from the pre-slab collector: each is its own allocation, so an
 * orphaned survivor is simply left off every list and table, still valid memory the caller
 * borrows. A slab-resident survivor cannot be orphaned the same way -- its memory is shared with
 * sibling slots in the same slab -- so at teardown a slab that still has a live slot in it is
 * leaked whole instead (a bounded, one-time cost paid only for a borrowed result, ADR 0034); a
 * slab with no survivors is freed like any other. An ordinary collection instead returns each
 * dead slot to its slab's free list and prunes a slab that lands completely empty. */
static void kiln_c99_gc_sweep(kiln_gc_heap *heap, int orphan_marked) {
    kiln_gc_block *block = heap->head;
    kiln_gc_slab *slab = heap->all_slabs;
    size_t survived = 0;
    while (block != NULL) {
        kiln_gc_block *next = block->next;
        if (block->mark != heap->epoch) {
            kiln_c99_gc_unlink(heap, block);
            kiln_c99_gc_finalize(block);
            heap->freed_blocks += 1;
            heap->freed_bytes += block->size;
            kiln_c99_gc_ptrset_remove(&heap->large_table, block);
            free(block);
        } else if (orphan_marked) {
            kiln_c99_gc_unlink(heap, block);
            kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        } else {
            survived += block->size;
        }
        block = next;
    }
    while (slab != NULL) {
        kiln_gc_slab *next_slab = slab->all_next;
        uint32_t index;
        int had_survivor = 0;
        for (index = 0; index < slab->slot_count; ++index) {
            kiln_gc_block *slot = kiln_c99_gc_slab_slot(slab, index);
            if (slot->size == KILN_GC_SLOT_FREE) continue;
            if (slot->mark != heap->epoch) {
                kiln_c99_gc_finalize(slot);
                heap->freed_blocks += 1;
                heap->freed_bytes += slot->size;
                heap->count -= 1;
                heap->bytes -= slot->size;
                slot->size = KILN_GC_SLOT_FREE;
                slot->kind = KILN_GC_NONE;
                slot->next = slab->free_head;
                slab->free_head = slot;
                if (++slab->free_count == 1) kiln_c99_gc_slab_link_free(heap, slab);
            } else if (orphan_marked) {
                had_survivor = 1;
            } else {
                survived += slot->size;
            }
        }
        if (orphan_marked) {
            if (!had_survivor) {
                if (slab->free_count != 0) kiln_c99_gc_slab_unlink_free(heap, slab);
                kiln_c99_gc_slab_unlink_all(heap, slab);
                kiln_c99_gc_ptrset_remove(&heap->slab_table, slab);
                kiln_c99_gc_aligned_free(slab);
            }
            /* else: leaked with a live survivor inside; the heap is being torn down anyway. */
        } else if (slab->free_count == slab->slot_count) {
            kiln_c99_gc_slab_maybe_destroy(heap, slab);
        }
        slab = next_slab;
    }
    {
        kiln_nursery_chunk **pchunk = &heap->nursery.promoted;
        while (*pchunk != NULL) {
            kiln_nursery_chunk *chunk = *pchunk;
            char *curr = chunk->base;
            size_t live = 0;
            while (curr < chunk->limit) {
                kiln_gc_block *b = (kiln_gc_block *)curr;
                size_t payload = b->size == 0 ? 1 : b->size;
                size_t total = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
                if ((b->kind & KILN_GC_FLAG_DEAD) == 0) {
                    if (orphan_marked) {
                        kiln_c99_gc_finalize(b);
                        b->kind |= KILN_GC_FLAG_DEAD;
                        size_t offset = (size_t)(curr - chunk->base);
                        size_t bit_idx = offset >> 4;
                        if (bit_idx >> 6 < chunk->bitmap_words) {
                            chunk->bitmap[bit_idx >> 6] &= ~((uint64_t)1 << (bit_idx & 63));
                        }
                    } else if (b->mark == heap->epoch) {
                        live++;
                        survived += b->size;
                    } else {
                        kiln_c99_gc_finalize(b);
                        heap->freed_blocks += 1;
                        heap->freed_bytes += b->size;
                        heap->count -= 1;
                        heap->bytes -= b->size;
                        b->kind |= KILN_GC_FLAG_DEAD;
                        size_t offset = (size_t)(curr - chunk->base);
                        size_t bit_idx = offset >> 4;
                        if (bit_idx >> 6 < chunk->bitmap_words) {
                            chunk->bitmap[bit_idx >> 6] &= ~((uint64_t)1 << (bit_idx & 63));
                        }
                    }
                }
                curr += total;
            }
            chunk->live_count = live;
            if (orphan_marked || live == 0) {
                *pchunk = chunk->next;
                free(chunk->base);
                free(chunk->bitmap);
                free(chunk);
            } else {
                pchunk = &chunk->next;
            }
        }
    }
    heap->survived = survived;
    heap->since_collection = 0;
}

static void kiln_c99_gc_collect(void *opaque, int force) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    size_t threshold;
    uint64_t before_blocks;
    uint64_t before_bytes;
    int nursery_has_allocs;
    int tenured_due;
    if (heap == NULL || heap->collecting || heap->fallback) return;
    if (!heap->enabled && !heap->stress) return;

    nursery_has_allocs = heap->nursery.active && (heap->nursery.bump > heap->nursery.base);
    threshold = heap->floor > heap->survived ? heap->floor : heap->survived;
    tenured_due = force || heap->stress || (heap->since_collection >= threshold);

    if (!nursery_has_allocs && !tenured_due) return;

    heap->collecting = 1;
    before_blocks = heap->freed_blocks;
    before_bytes = heap->freed_bytes;
    kiln_c99_gc_begin(heap);
    (void)kiln_realm_gc_mark_roots(heap);
    kiln_c99_key_mark_pinned(heap);
    if (heap->mid_turn) kiln_c99_gc_scan_stack(heap);
    kiln_c99_gc_drain(heap);

    if (!heap->mark_failed) {
        kiln_c99_key_purge(heap);
        /* Mid-turn, the nursery is left as it is: its dead blocks wait for the turn's end,
         * when the whole region resets or promotes as before, and its live ones need no
         * promotion because nothing moves. The slabs, where a long turn's allocation goes
         * once the nursery is full, are swept below. */
        if (nursery_has_allocs && !heap->mid_turn) {
            size_t nursery_survivors = 0;
            size_t nursery_dead = 0;
            size_t nursery_reclaimed_bytes = 0;
            char *curr = heap->nursery.base;
            while (curr < heap->nursery.bump) {
                kiln_gc_block *b = (kiln_gc_block *)curr;
                size_t payload = b->size == 0 ? 1 : b->size;
                size_t total = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
                if ((b->kind & KILN_GC_FLAG_DEAD) == 0) {
                    if (b->mark == heap->epoch) {
                        nursery_survivors++;
                    } else {
                        nursery_dead++;
                        nursery_reclaimed_bytes += b->size;
                        kiln_c99_gc_finalize(b);
                        b->kind |= KILN_GC_FLAG_DEAD;
                        size_t offset = (size_t)(curr - heap->nursery.base);
                        size_t bit_idx = offset >> 4;
                        if (bit_idx >> 6 < heap->nursery.bitmap_words) {
                            heap->nursery.bitmap[bit_idx >> 6] &= ~((uint64_t)1 << (bit_idx & 63));
                        }
                    }
                }
                curr += total;
            }

            if (nursery_survivors == 0) {
                /* No survivors: reset the pointer! */
                size_t used = (size_t)(heap->nursery.bump - heap->nursery.base);
                size_t used_words = (used + 1023) >> 10;
                if (used_words > heap->nursery.bitmap_words) used_words = heap->nursery.bitmap_words;
                memset(heap->nursery.bitmap, 0, used_words * sizeof(uint64_t));
                heap->nursery.bump = heap->nursery.base;
                heap->nursery.count = 0;
                heap->nursery.bytes = 0;
                heap->nursery.resets++;
                heap->count -= nursery_dead;
                heap->bytes -= nursery_reclaimed_bytes;
                heap->freed_blocks += nursery_dead;
                heap->freed_bytes += nursery_reclaimed_bytes;
                if (heap->since_collection >= nursery_reclaimed_bytes) {
                    heap->since_collection -= nursery_reclaimed_bytes;
                } else {
                    heap->since_collection = 0;
                }
            } else {
                /* Else promote! */
                kiln_nursery_chunk *chunk = (kiln_nursery_chunk *)malloc(sizeof(kiln_nursery_chunk));
                if (chunk != NULL) {
                    chunk->base = heap->nursery.base;
                    chunk->limit = heap->nursery.bump;
                    chunk->bitmap = heap->nursery.bitmap;
                    chunk->bitmap_words = heap->nursery.bitmap_words;
                    chunk->live_count = nursery_survivors;
                    chunk->next = heap->nursery.promoted;
                    heap->nursery.promoted = chunk;
                    heap->nursery.promotions += nursery_survivors;
                    heap->count -= nursery_dead;
                    heap->bytes -= nursery_reclaimed_bytes;
                    heap->freed_blocks += nursery_dead;
                    heap->freed_bytes += nursery_reclaimed_bytes;
                    if (heap->since_collection >= nursery_reclaimed_bytes) {
                        heap->since_collection -= nursery_reclaimed_bytes;
                    } else {
                        heap->since_collection = 0;
                    }
                    /* Fresh nursery for next turn */
                    heap->nursery.base = (char *)malloc(heap->nursery.capacity);
                    if (heap->nursery.base != NULL) {
                        heap->nursery.bump = heap->nursery.base;
                        heap->nursery.limit = heap->nursery.base + heap->nursery.capacity;
                        heap->nursery.bitmap = (uint64_t *)calloc(heap->nursery.bitmap_words, sizeof(uint64_t));
                        heap->nursery.count = 0;
                        heap->nursery.bytes = 0;
                        if (heap->nursery.bitmap == NULL) {
                            free(heap->nursery.base);
                            heap->nursery.base = NULL;
                            heap->nursery.active = 0;
                        }
                    } else {
                        heap->nursery.active = 0;
                    }
                }
            }
        }

        if (tenured_due || (heap->nursery.promoted != NULL)) {
            kiln_c99_gc_sweep(heap, 0);
            kiln_c99_gc_clear_dead_sites(heap);
            heap->collections += 1;
        }
    }
    if (heap->trace) {
        kiln_c99_gc_trace_text("kiln gc: collection ");
        kiln_c99_gc_trace_count(heap->collections);
        kiln_c99_gc_trace_text(" freed ");
        kiln_c99_gc_trace_count(heap->freed_blocks - before_blocks);
        kiln_c99_gc_trace_text(" blocks (");
        kiln_c99_gc_trace_count(heap->freed_bytes - before_bytes);
        kiln_c99_gc_trace_text(" bytes), ");
        kiln_c99_gc_trace_count((uint64_t)heap->count);
        kiln_c99_gc_trace_text(" blocks (");
        kiln_c99_gc_trace_count((uint64_t)heap->bytes);
        kiln_c99_gc_trace_text(heap->mark_failed
                                   ? " bytes) live (marking ran out of memory; nothing freed)\n"
                                   : " bytes) live\n");
    }
    heap->collecting = 0;
}

static void kiln_c99_gc_teardown_begin(void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    if (heap == NULL || heap->fallback) return;
    heap->collecting = 1;
    kiln_c99_gc_begin(heap);
}

static void kiln_c99_gc_teardown_end(void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    if (heap == NULL || heap->fallback) return;
    kiln_c99_gc_drain(heap);
    if (heap->mark_failed) {
        /* Orphan everything rather than free a block a survivor root still names. */
        ++heap->epoch;
        if (heap->epoch == 0) ++heap->epoch;
        for (kiln_gc_block *block = heap->head; block != NULL; block = block->next)
            block->mark = heap->epoch;
        for (kiln_gc_slab *slab = heap->all_slabs; slab != NULL; slab = slab->all_next) {
            uint32_t index;
            for (index = 0; index < slab->slot_count; ++index) {
                kiln_gc_block *slot = kiln_c99_gc_slab_slot(slab, index);
                if (slot->size != KILN_GC_SLOT_FREE) slot->mark = heap->epoch;
            }
        }
    }
    kiln_c99_gc_sweep(heap, 1);
    if (heap->trace) {
        kiln_c99_gc_trace_text("kiln gc: teardown after ");
        kiln_c99_gc_trace_count(heap->collections);
        kiln_c99_gc_trace_text(" collections, ");
        kiln_c99_gc_trace_count(heap->freed_blocks);
        kiln_c99_gc_trace_text(" blocks (");
        kiln_c99_gc_trace_count(heap->freed_bytes);
        kiln_c99_gc_trace_text(" bytes) freed in total\n");
    }
    if (heap->nursery.active && heap->nursery.base != NULL) {
        char *curr = heap->nursery.base;
        while (curr < heap->nursery.bump) {
            kiln_gc_block *b = (kiln_gc_block *)curr;
            size_t payload = b->size == 0 ? 1 : b->size;
            size_t total = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
            if ((b->kind & KILN_GC_FLAG_DEAD) == 0) {
                kiln_c99_gc_finalize(b);
                b->kind |= KILN_GC_FLAG_DEAD;
            }
            curr += total;
        }
    }
    kiln_c99_nursery_destroy(&heap->nursery);
    free(heap->large_table.table);
    free(heap->slab_table.table);
    free(heap->stack);
    free(heap->keys.entries);
    free(heap);
}

static void kiln_c99_gc_hook_mark_value(void *opaque, const void *value) {
    kiln_c99_gc_mark_value((kiln_gc_heap *)opaque, (const kiln_value *)value);
}

static void kiln_c99_gc_hook_mark_block(void *opaque, void *payload) {
    kiln_c99_gc_mark_block((kiln_gc_heap *)opaque, payload);
}

static const kiln_gc_hooks kiln_c99_gc_hooks = {
    KILN_GC_HOOKS_VERSION,
    (uint32_t)sizeof(kiln_gc_hooks),
    kiln_c99_gc_collect,
    kiln_c99_gc_teardown_begin,
    kiln_c99_gc_teardown_end,
    kiln_c99_gc_hook_mark_value,
    kiln_c99_gc_hook_mark_block,
    kiln_c99_gc_hook_introspect,
};

/* A borrowed result slot: marked during collection and again at teardown, so the value a caller
 * reads after `kiln_realm_destroy` is never freed by it (ADR 0034's borrowed-result contract). */
static void kiln_c99_gc_result_roots(void *opaque, void *context, int phase) {
    if (phase == KILN_GC_PHASE_RESET || context == NULL) return;
    kiln_c99_gc_mark_value((kiln_gc_heap *)opaque, (const kiln_value *)context);
}

/* Allocation helpers for the kinds whose bytes are the value: strings and raw byte payloads. */
static char *kiln_c99_gc_alloc_text(size_t len) {
    kiln_string *s = (kiln_string *)kiln_c99_gc_alloc(KILN_GC_STRING, sizeof(kiln_string) + len + 1);
    if (s == NULL) return NULL;
    s->length = len;
    s->byte_len = len;
    s->bytes = (const char *)(s + 1);
    char *bytes = (char *)(s + 1);
    bytes[len] = '\0';
    return bytes;
}

static char *kiln_c99_gc_alloc_text_tenured(size_t len) {
    kiln_string *s = (kiln_string *)kiln_c99_gc_alloc_tenured(KILN_GC_STRING, sizeof(kiln_string) + len + 1);
    if (s == NULL) return NULL;
    s->length = len;
    s->byte_len = len;
    s->bytes = (const char *)(s + 1);
    char *bytes = (char *)(s + 1);
    bytes[len] = '\0';
    return bytes;
}

static kiln_shape kiln_c99_root_shape = {
    NULL, /* parent */
    NULL, /* ikey */
    0,    /* slot_index */
    0,    /* property_count */
    0,    /* num_transitions */
    {NULL}, /* keys */
    {NULL, NULL}, /* inline_trans_ikey */
    {NULL, NULL}, /* inline_trans_next */
    NULL  /* transitions */
};
