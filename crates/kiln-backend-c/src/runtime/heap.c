
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
/* The nursery's chunk budget: filled chunks are collected inside a turn once this many bytes of
 * them wait, while the live heap is small next to it (`KILN_NURSERY_SMALL_LIVE_BUDGETS`).
 * `KILN_NURSERY_KIB` overrides it. */
#define KILN_NURSERY_BUDGET ((size_t)4 * 1024 * 1024)
/* Chunk collections run only while the live heap is at most this many budgets: a large live
 * heap makes every mark expensive, and there the chunks wait for the size-based trigger. */
#define KILN_NURSERY_SMALL_LIVE_BUDGETS 8
/* A pass whose chunks mostly promoted says the turn is building a structure, not churning;
 * collecting again after one more budget would mark that structure again for nothing. The
 * next chunk collection then waits this many times longer, up to `KILN_NURSERY_BUDGET_CAP`
 * budgets; a pass whose chunks mostly reset brings the wait straight back to one budget.
 * Measured on a 2.2 MB JSON.parse, ten thousand rows all live until the parse returns, twenty
 * parses in one turn: a fixed budget collected each parse four times as it grew and marked its
 * own result each time -- 81 collections and 22.0 ms per parse; with this growth, 13
 * collections and 7.4 ms, against 6.6 ms for the single-region nursery before 9d96e4a8. */
#define KILN_NURSERY_BUDGET_GROWTH 2
#define KILN_NURSERY_BUDGET_CAP 8
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
#define KILN_GC_KIND_MASK ((uint32_t)0x07FFFFFFu)
#define KILN_GC_FLAG_ASCII_KNOWN ((uint32_t)0x80000000u)
#define KILN_GC_FLAG_ASCII ((uint32_t)0x40000000u)
#define KILN_GC_FLAG_NURSERY ((uint32_t)0x20000000u)
#define KILN_GC_FLAG_DEAD ((uint32_t)0x10000000u)
/* A block malloc'd on its own, outside every slab: with `KILN_GC_FLAG_NURSERY` this lets a header
 * alone say where its block lives, which is what `kiln_c99_gc_release_cell` needs. */
#define KILN_GC_FLAG_LARGE ((uint32_t)0x08000000u)

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
#define KILN_GC_SLAB_CLASS_MAX ((size_t)384)
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

static int kiln_c99_gc_ptrset_insert(kiln_gc_ptrset *set, void *pointer);
static void kiln_c99_gc_ptrset_remove(kiln_gc_ptrset *set, const void *pointer);
static int kiln_c99_gc_ptrset_contains(const kiln_gc_ptrset *set, const void *pointer);
static void *kiln_c99_gc_aligned_alloc(size_t alignment, size_t size);
static void kiln_c99_gc_aligned_free(void *aligned);

/* Interned property keys (ADR 0047). One entry per distinct key spelling the realm has ever
 * stored or registered, mapping the bytes to the one pointer every property with that name
 * carries -- so `property->key == key` is the whole of a key comparison and a linear scan never
 * touches key bytes. The table is the heap's because the heap is the one struct every
 * translation unit shares: two units each keeping their own would have two canonical pointers
 * for one name, which is exactly the situation this exists to end. Constant keys live one level
 * further out, in the process's shared table (ADR 0071; see `kiln_shared_key_table`), so that
 * every realm's constant keys are the same pointers; this table holds the rest.
 *
 * An entry is *pinned* when its bytes are immortal (a unit's constant table, a prelude literal,
 * a symbol's reserved key) or when the table itself has undertaken to keep a block alive because
 * a unit registered the same spelling after a computed key had already claimed it. Every other
 * entry is weak: its bytes are a string block that the properties holding the key keep marked,
 * and the collection that finds the block unmarked drops the entry before the sweep frees it. */
/* The bytes of an interned key sit behind a `kiln_string` header, the block layout every other
 * string has, so a key's length is one load and never `strlen`: a key may contain U+0000
 * (`JSON.parse('{"a\\u0000b":1}')`, a registry symbol's binary id), and the collector's
 * interior-pointer fallback for strings already looks one header back from the bytes. The
 * trailing NUL stays so a `%s` diagnostic keeps working; it is not the length. */
static inline size_t kiln_c99_key_len(const kiln_string *ikey) {
    return ikey == NULL ? 0 : ikey->unit_len;
}

typedef struct kiln_key_entry {
    const kiln_string *string; /* NULL when the slot is empty */
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
    /* Chunks filled since the last collection, oldest last; each is reset or promoted on its
     * own at the next collection, so one long-lived object retires one chunk and not the whole
     * nursery. `full_bytes` against `budget` is what makes a collection due inside a turn. */
    kiln_nursery_chunk *full;
    size_t full_bytes;
    size_t budget;
    /* The budget the next chunk collection waits for. A pass whose chunks mostly promoted says
     * the turn is building a structure, not churning; collecting again after one more budget
     * would mark that structure again for nothing, so the wait doubles, up to eight budgets.
     * A pass whose chunks mostly reset brings it straight back to `budget`. Without this, a
     * 2.2 MB JSON.parse was collected four times as it grew and marked its own result each
     * time: 81 collections for twenty parses where the single-region nursery took 8. */
    size_t budget_now;
    /* Emptied chunks kept for reuse, with their bitmaps, up to a budget's worth. */
    kiln_nursery_chunk *spare;
    size_t spare_count;
    /* The lowest and highest address of any chunk ever made: a word outside the pair -- almost
     * every word the conservative scan looks at -- is rejected before the chunk lists are walked. */
    const char *lo;
    const char *hi;
    /* Every chunk is one region: an allocation of `capacity` bytes aligned to `capacity`, with
     * the chunk header at its start and its bitmap after that. `ptr & ~(capacity - 1)` names
     * the region a pointer would lie in, and `regions` -- the set of region bases this nursery
     * has made and not freed -- says whether that name is one of ours before the header is
     * read; the same arrangement the slabs use. The lookup is a mask and a hash probe whatever
     * the number of chunks: walking the two lists per lookup made a 2.2 MB `JSON.parse` fifty
     * times slower once it had filled a few hundred of them. `region` is the current chunk's. */
    char *region;
    kiln_gc_ptrset regions;
} kiln_gc_nursery;

/* One realm-local edge out of a shared shape (ADR 0071): the child is the heap's own, so the edge
 * cannot live in the shared parent, which every realm reads and none may point into another's
 * memory. A `key` of NULL is the parent's count row: `child` then holds how many realm-local
 * edges the parent has in this heap, for the same fan-out ceiling a shape's own edges obey. */
typedef struct kiln_c99_local_edge {
    const kiln_shape *parent;
    const kiln_string *key;
    kiln_shape *child;
} kiln_c99_local_edge;

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
    /* The lowest and highest address of any memory this heap has handed out -- slabs, large
     * blocks and nursery chunks. A word outside the pair, which is nearly every word the
     * conservative scan reads (return addresses, stack addresses, small integers), is rejected
     * with one compare before any table is probed. */
    uintptr_t lo;
    uintptr_t hi;
    uint32_t class_slab_count[(KILN_GC_SLAB_CLASS_MAX - KILN_GC_SLAB_CLASS_MIN) /
                              KILN_GC_SLAB_CLASS_STEP + 1]; /* slabs of each class, live */
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
    const kiln_string *hot_keys[KILN_C99_HOT_KEY_COUNT];
    /* ADR 0071. A process-unique number for this heap, never reused, for a cache that must know
     * which heap it was built under when a freed heap's address can come back as another's. */
    uint64_t serial;
    /* Every realm-local shape this heap made, freed with it; and the side table of realm-local
     * edges out of shared shapes (see `kiln_c99_local_edge`). */
    kiln_shape *local_shapes;
    kiln_c99_local_edge *local_edges;
    size_t local_edge_capacity;
    size_t local_edge_count;
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
    /* The promise job lane (Phase 6.3). A promise reaction and an await resumption used to be a
     * GC cell and a call across the private ABI each; they are values in this array now, which
     * the realm drains by calling back into the prelude. It lives in the heap because the heap
     * is the one per-realm object every emitted unit shares -- the prelude's statics are
     * per-unit with internal linkage, so a ring in one of those would be a queue the other
     * unit's `await` never reaches.
     *
     * Head and tail are indices, not a modulus: a drain runs the ring empty, and empty resets
     * both to zero, so the array only ever has to hold one turn's worth of jobs. */
    kiln_microtask *jobs;
    size_t job_head;
    size_t job_tail;
    size_t job_capacity;
    size_t job_mask;
    /* `process.nextTick` callbacks queued and not yet disposed of. The queue itself is the
     * realm's (Rust); this count is what `__kilnJobsPending` reads to answer "would a checkpoint
     * run anything now" without a call across the private ABI. */
    size_t next_ticks_queued;
} kiln_gc_heap;

static const kiln_gc_hooks kiln_c99_gc_hooks;
/* The heap a unit uses when no realm is active: the prelude's own tests. It never collects. */
static kiln_gc_heap kiln_c99_gc_fallback;

/* ---- Process-wide shared state (ADR 0071) --------------------------------------------------
 *
 * Constant keys and the shapes built from them mean the same thing in every realm, so they are
 * kept once per process. The prelude is emitted per translation unit with internal linkage, so
 * the block that holds them cannot be a static here -- that would be one per unit -- and is
 * reserved by the provider archive instead (`kiln_realm_process_shared`), which every unit of the
 * process links once. The block starts zeroed; the first unit to reach it stamps `layout`.
 *
 * Writers take `lock`, a spin lock held only across a table insert or a shape allocation. Readers
 * take nothing: a published key table or shape edge is complete before the release store that
 * makes it reachable, and is never changed or freed afterwards. */
#define KILN_C99_PROCESS_LAYOUT (UINT64_C(0x6b696c6e00010000) | (uint64_t)sizeof(struct kiln_shared_state))

/* The shared key table: open addressing, never shrunk, never deleted from. A grown table is a new
 * allocation published whole; the one it replaces is left in place for any reader still probing
 * it (tables only double, so everything ever left behind is less than the live one). */
typedef struct kiln_shared_key_table {
    size_t capacity; /* a power of two */
    size_t count;
    kiln_key_entry entries[];
} kiln_shared_key_table;

typedef struct kiln_shared_state {
    uint64_t layout;
    uint32_t lock;
    /* Heaps registered and not yet torn down. Registration (`kiln_c99_register_static_keys`)
     * adds to the shared table only while this is zero, and a constant first met at run time
     * joins it only while the heap interning it is the only live one: either way no spelling is
     * ever a shared key and some live realm's own key at once (see `kiln_c99_intern_static_key`). */
    uint32_t live_heaps;
    /* Set once the built-ins' keys are registered, by whichever prelude copy got there first. */
    uint32_t builtins_registered;
    uint64_t next_heap_serial;
    kiln_shared_key_table *keys;
    /* The empty key, headed like every other so `kiln_c99_key_len` reads zero from it. One for the
     * process rather than one per unit, so "" is one key across units too. */
    kiln_string empty_key;
    uint8_t empty_bytes[8];
    /* The root of the shape tree. */
    kiln_shape root;
} kiln_shared_state;

/* This unit's pointer to the process block, set once it is initialised. */
static kiln_shared_state *kiln_c99_shared_block;

static void kiln_c99_process_lock(kiln_shared_state *process) {
    while (__atomic_exchange_n(&process->lock, 1u, __ATOMIC_ACQUIRE) != 0u) {
        while (__atomic_load_n(&process->lock, __ATOMIC_RELAXED) != 0u) {
        }
    }
}

static void kiln_c99_process_unlock(kiln_shared_state *process) {
    __atomic_store_n(&process->lock, 0u, __ATOMIC_RELEASE);
}

static kiln_shared_state *kiln_c99_process_slow(void) {
    kiln_shared_state *process = (kiln_shared_state *)kiln_realm_process_shared(sizeof *process);
    uint64_t layout;
    if (process == NULL) {
        fputs("kiln: the runtime archive reserves too little process-shared storage for this "
              "prelude; rebuild the native provider\n",
              stderr);
        abort();
    }
    layout = __atomic_load_n(&process->layout, __ATOMIC_ACQUIRE);
    if (layout != KILN_C99_PROCESS_LAYOUT) {
        kiln_c99_process_lock(process);
        layout = process->layout;
        if (layout == 0) {
            process->empty_key.unit_len = 0;
            process->empty_key.flags = KILN_STRING_LATIN1;
            process->empty_key.data = process->empty_bytes;
            process->root.shared = 1;
            layout = KILN_C99_PROCESS_LAYOUT;
            __atomic_store_n(&process->layout, layout, __ATOMIC_RELEASE);
        }
        kiln_c99_process_unlock(process);
        if (layout != KILN_C99_PROCESS_LAYOUT) {
            /* Two units built from different preludes in one process: their shared layouts
             * disagree, and a shape or key one of them publishes would be misread by the other. */
            fputs("kiln: units built from different runtime preludes share this process\n", stderr);
            abort();
        }
    }
    __atomic_store_n(&kiln_c99_shared_block, process, __ATOMIC_RELEASE);
    return process;
}

static inline kiln_shared_state *kiln_c99_process(void) {
    kiln_shared_state *process = __atomic_load_n(&kiln_c99_shared_block, __ATOMIC_ACQUIRE);
    return KILN_LIKELY(process != NULL) ? process : kiln_c99_process_slow();
}

/* A heap joins the count of live heaps when it becomes a realm's (or the fallback), and leaves
 * it at teardown. Registration also gives it its serial. */
static void kiln_c99_gc_heap_register(kiln_gc_heap *heap) {
    kiln_shared_state *process = kiln_c99_process();
    kiln_c99_process_lock(process);
    process->live_heaps += 1;
    heap->serial = ++process->next_heap_serial;
    kiln_c99_process_unlock(process);
}

static void kiln_c99_gc_heap_unregister(kiln_gc_heap *heap) {
    kiln_shared_state *process = kiln_c99_process();
    if (heap->serial == 0) return;
    kiln_c99_process_lock(process);
    if (process->live_heaps != 0) process->live_heaps -= 1;
    kiln_c99_process_unlock(process);
    heap->serial = 0;
}

/* Widen the heap's address bounds to cover [base, base + size). */
static inline void kiln_c99_gc_bound(kiln_gc_heap *heap, const void *base, size_t size) {
    uintptr_t lo = (uintptr_t)base;
    uintptr_t hi = lo + size;
    if (heap->lo == 0 || lo < heap->lo) heap->lo = lo;
    if (hi > heap->hi) heap->hi = hi;
}


/* Set with `KILN_GC_STRESS`, and read only on `kiln_c99_gc_kind_of_ptr`: the header read is checked
 * against the heap search it replaced, so a pointer that never came from the allocator is caught
 * where it is used rather than where its garbage kind is acted on. */
static int kiln_c99_gc_stress_checks;

#define KILN_NURSERY_HEADER ((sizeof(kiln_nursery_chunk) + 63) & ~(size_t)63)

/* One new region: aligned to its own size, entered in the set of region bases, its header and
 * bitmap laid out at its start and the payload beginning after them. NULL when memory is
 * refused, with nothing entered. */
static kiln_nursery_chunk *kiln_c99_nursery_new_region(kiln_gc_nursery *nursery) {
    char *region = (char *)kiln_c99_gc_aligned_alloc(nursery->capacity, nursery->capacity);
    kiln_nursery_chunk *chunk;
    size_t bitmap_bytes;
    if (region == NULL) return NULL;
    if (kiln_c99_gc_ptrset_insert(&nursery->regions, region) != KILN_C99_OK) {
        kiln_c99_gc_aligned_free(region);
        return NULL;
    }
    chunk = (kiln_nursery_chunk *)region;
    chunk->bitmap = (uint64_t *)(region + KILN_NURSERY_HEADER);
    chunk->bitmap_words = nursery->bitmap_words;
    memset(chunk->bitmap, 0, chunk->bitmap_words * sizeof(uint64_t));
    bitmap_bytes = (chunk->bitmap_words * sizeof(uint64_t) + 15) & ~(size_t)15;
    chunk->base = region + KILN_NURSERY_HEADER + bitmap_bytes;
    chunk->limit = chunk->base;
    chunk->live_count = 0;
    chunk->next = NULL;
    if (nursery->lo == NULL || region < nursery->lo) nursery->lo = region;
    if (nursery->hi == NULL || region + nursery->capacity > nursery->hi) {
        nursery->hi = region + nursery->capacity;
    }
    return chunk;
}

static void kiln_c99_nursery_free_region(kiln_gc_nursery *nursery, kiln_nursery_chunk *chunk) {
    kiln_c99_gc_ptrset_remove(&nursery->regions, chunk);
    kiln_c99_gc_aligned_free(chunk);
}

/* Make `chunk`'s region the one allocation bumps through. */
static void kiln_c99_nursery_use(kiln_gc_nursery *nursery, kiln_nursery_chunk *chunk) {
    nursery->region = (char *)chunk;
    nursery->base = chunk->base;
    nursery->bump = chunk->base;
    nursery->limit = nursery->region + nursery->capacity;
    nursery->bitmap = chunk->bitmap;
    nursery->count = 0;
    nursery->bytes = 0;
}

/* Write the current region's fill into its header, ready for one of the lists. */
static kiln_nursery_chunk *kiln_c99_nursery_seal(kiln_gc_nursery *nursery) {
    kiln_nursery_chunk *chunk = (kiln_nursery_chunk *)nursery->region;
    chunk->limit = nursery->bump;
    chunk->live_count = 0;
    chunk->next = NULL;
    return chunk;
}

static void kiln_c99_nursery_init(kiln_gc_nursery *nursery) {
    const char *setting;
    memset(nursery, 0, sizeof *nursery);
    /* The nursery is a budget of chunks, not one block: allocation bumps through a chunk, a
     * filled chunk waits for the next collection, and there each chunk is reset for reuse when
     * nothing in it survived or promoted on its own when something did. `KILN_NURSERY_KIB`
     * sets the budget; the chunk stays a size a few thousand blocks fill. */
    nursery->capacity = (size_t)256 * 1024;
    nursery->budget = KILN_NURSERY_BUDGET;
    setting = getenv("KILN_NURSERY_KIB");
    if (setting != NULL && setting[0] >= '0' && setting[0] <= '9') {
        size_t kib = (size_t)strtoul(setting, NULL, 10);
        if (kib > 0 && kib <= 1024 * 1024) nursery->budget = kib * 1024;
    }
    if (nursery->budget < nursery->capacity) nursery->capacity = nursery->budget;
    nursery->budget_now = nursery->budget;
    /* A region is found by masking, so its size is a power of two; and it is no smaller than a
     * slab, so a small budget still gives a chunk a few thousand blocks fill. */
    {
        size_t rounded = KILN_GC_SLAB_SIZE;
        while (rounded * 2 <= nursery->capacity) rounded *= 2;
        nursery->capacity = rounded;
    }
    nursery->bitmap_words = (nursery->capacity / 16 + 63) / 64;
    {
        kiln_nursery_chunk *chunk = kiln_c99_nursery_new_region(nursery);
        if (chunk != NULL) {
            kiln_c99_nursery_use(nursery, chunk);
            nursery->active = 1;
        }
    }
    setting = getenv("KILN_NURSERY");
    if (setting != NULL && setting[0] == '0' && setting[1] == '\0') {
        nursery->active = 0;
    }
}

static void kiln_c99_nursery_free_list(kiln_gc_nursery *nursery, kiln_nursery_chunk *chunk) {
    while (chunk != NULL) {
        kiln_nursery_chunk *next = chunk->next;
        kiln_c99_nursery_free_region(nursery, chunk);
        chunk = next;
    }
}

static void kiln_c99_nursery_destroy(kiln_gc_nursery *nursery) {
    if (nursery->region != NULL) {
        kiln_c99_nursery_free_region(nursery, (kiln_nursery_chunk *)nursery->region);
        nursery->region = NULL;
        nursery->base = NULL;
        nursery->bitmap = NULL;
    }
    kiln_c99_nursery_free_list(nursery, nursery->promoted);
    nursery->promoted = NULL;
    kiln_c99_nursery_free_list(nursery, nursery->full);
    nursery->full = NULL;
    nursery->full_bytes = 0;
    kiln_c99_nursery_free_list(nursery, nursery->spare);
    nursery->spare = NULL;
    nursery->spare_count = 0;
    free(nursery->regions.table);
    nursery->regions.table = NULL;
    nursery->regions.capacity = 0;
    nursery->regions.count = 0;
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
        kiln_c99_gc_stress_checks = 1;
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
    if (heap->nursery.active) kiln_c99_gc_bound(heap, heap->nursery.region, heap->nursery.capacity);
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

/* The heap for this thread's realm, remembered until the realm changes. Every allocation and
 * every kind lookup asks for it, and the answer used to be a call into the runtime archive each
 * time. The realm bumps a per-thread generation on every entry, exit and heap attach
 * (`kiln_realm_generation_slot` is the address of it); a cached heap is trusted only while the
 * generation it was cached under is the current one, so a thread that hosts several realms in
 * turn -- the daemon -- never serves one realm the other's heap. Thread-local, not static,
 * because two threads may each be in a realm. */
typedef struct kiln_realm_cache {
    const uint64_t *generation_slot;
    uint64_t generation;
    kiln_gc_heap *heap;
} kiln_realm_cache;
static _Thread_local kiln_realm_cache kiln_c99_realm_cached;

static kiln_gc_heap *kiln_c99_gc_heap_uncached(void);
/* The promise lane's two callbacks, defined with the queue itself in `promises.c`. */
static int kiln_c99_run_promise_jobs(void *context, void *host, void *thrown);
static int kiln_c99_promise_jobs_pending(void *context);

static kiln_gc_heap *kiln_c99_gc_heap(void) {
    kiln_realm_cache *cache = &kiln_c99_realm_cached;
    uint64_t generation;
    kiln_gc_heap *heap;
    if (cache->generation_slot == NULL) cache->generation_slot = kiln_realm_generation_slot();
    generation = *cache->generation_slot;
    if (cache->heap != NULL && cache->generation == generation) return cache->heap;
    heap = kiln_c99_gc_heap_uncached();
    /* A fallback heap is the answer for a realm that could not attach one; it is per process
     * and remembering it is harmless. NULL is not remembered. */
    cache->heap = heap;
    cache->generation = generation;
    return heap;
}

/* The generation of the realm this thread is in: the word `kiln_c99_gc_heap` trusts its heap
 * pointer under, for any other cache in compiled C that must not outlive the realm it learned
 * from. */
#ifndef KILN_C99_REALM_GENERATION_DEFINED
#define KILN_C99_REALM_GENERATION_DEFINED
static inline uint64_t kiln_c99_realm_generation(void) {
    kiln_realm_cache *cache = &kiln_c99_realm_cached;
    if (cache->generation_slot == NULL) cache->generation_slot = kiln_realm_generation_slot();
    return *cache->generation_slot;
}
#endif

/* Hand the realm this heap's promise lane and answer with the heap.
 *
 * Registering here rather than once at startup is what makes the lane per realm: a realm is a
 * heap, and this is where a unit learns which heap it is in. The registration is stored on the
 * realm that is active, so two live realms on one thread each keep their own lane rather than
 * one replacing the other. Every unit's copy of the two callbacks behaves identically -- they
 * read the ring out of the context -- so it does not matter which unit's copy a realm holds. */
static kiln_gc_heap *kiln_c99_gc_lane_attached(void *attached) {
    (void)kiln_realm_set_promise_lane(kiln_c99_run_promise_jobs, kiln_c99_promise_jobs_pending,
                                      attached);
    return (kiln_gc_heap *)attached;
}

static kiln_gc_heap *kiln_c99_gc_heap_uncached(void) {
    void *attached = kiln_realm_gc_current_heap();
    kiln_gc_heap *candidate;
    int status;
    if (attached != NULL) return kiln_c99_gc_lane_attached(attached);
    status = kiln_realm_gc_attach(&kiln_c99_gc_hooks, NULL, &attached);
    if (status == KILN_C99_OK && attached != NULL) return kiln_c99_gc_lane_attached(attached);
    if (status != KILN_C99_OK) {
        if (kiln_c99_gc_fallback.hooks == NULL) {
            kiln_c99_gc_heap_init(&kiln_c99_gc_fallback);
            kiln_c99_gc_fallback.enabled = 0;
            kiln_c99_gc_fallback.fallback = 1;
            kiln_c99_gc_heap_register(&kiln_c99_gc_fallback);
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
    else {
        kiln_c99_introspect_armed = kiln_c99_introspect_arm();
        kiln_c99_gc_heap_register(candidate);
    }
    return kiln_c99_gc_lane_attached(attached);
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
        /* Four spares, or a quarter of the class, whichever is more: a class that ran to sixty
         * slabs between collections -- twenty thousand promises and their jobs in one turn --
         * would otherwise return fifty-six of them to the allocator at each collection and take
         * them straight back, and that churn was a tenth of such a loop. A class that stops
         * growing still shrinks, by a quarter per collection. */
        if (spare < 4 || spare * 4 < heap->class_slab_count[slab->class_index]) return;
    }
    if (slab->free_prev == NULL && slab->free_next == NULL) return;
    kiln_c99_gc_slab_unlink_free(heap, slab);
    kiln_c99_gc_slab_unlink_all(heap, slab);
    kiln_c99_gc_ptrset_remove(&heap->slab_table, slab);
    heap->class_slab_count[slab->class_index] -= 1;
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
    heap->class_slab_count[class_index] += 1;
    kiln_c99_gc_bound(heap, slab, KILN_GC_SLAB_SIZE);
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

/* The nursery region holding `ptr` -- the current chunk, one filled this cycle, or a promoted
 * one -- as its base, its allocation limit and its bitmap. */
static int kiln_c99_nursery_locate(const kiln_gc_heap *heap, const char *ptr, char **base,
                                   char **limit, uint64_t **bitmap, size_t *words) {
    const kiln_gc_nursery *nursery;
    const kiln_nursery_chunk *chunk;
    const char *region;
    /* `active` only says whether a new current chunk can receive allocations. Filled and
     * promoted chunks remain owned regions after an allocation failure deactivates the nursery,
     * and roots into those chunks must still be discoverable. */
    if (heap == NULL || ptr == NULL || heap->nursery.capacity == 0) return 0;
    nursery = &heap->nursery;
    if (ptr < nursery->lo || ptr >= nursery->hi) return 0;
    if (nursery->base != NULL && nursery->bump != NULL && ptr >= nursery->base &&
        ptr < nursery->bump) {
        *base = nursery->base;
        *limit = nursery->bump;
        *bitmap = nursery->bitmap;
        *words = nursery->bitmap_words;
        return 1;
    }
    /* The region's base is the pointer masked to the chunk size; it is read as a header only
     * once the set says this nursery made it. The current region past its bump holds nothing. */
    region = (const char *)((uintptr_t)ptr & ~(uintptr_t)(nursery->capacity - 1));
    if (region == nursery->region) return 0;
    if (!kiln_c99_gc_ptrset_contains(&nursery->regions, region)) return 0;
    chunk = (const kiln_nursery_chunk *)region;
    if (ptr < chunk->base || ptr >= chunk->limit) return 0;
    *base = chunk->base;
    *limit = chunk->limit;
    *bitmap = chunk->bitmap;
    *words = chunk->bitmap_words;
    return 1;
}

/* The live block that starts exactly at `candidate` inside a located region, or NULL. */
static kiln_gc_block *kiln_c99_nursery_block_at(const char *base, const char *limit,
                                                const uint64_t *bitmap, size_t words,
                                                const char *candidate) {
    size_t block_offset;
    size_t bit_idx;
    kiln_gc_block *block;
    if (candidate < base || candidate + sizeof(kiln_gc_block) > limit) return NULL;
    block_offset = (size_t)(candidate - base);
    if ((block_offset & 15) != 0) return NULL;
    bit_idx = block_offset >> 4;
    if (bit_idx >> 6 >= words) return NULL;
    if ((bitmap[bit_idx >> 6] & ((uint64_t)1 << (bit_idx & 63))) == 0) return NULL;
    block = (kiln_gc_block *)candidate;
    if (block->kind & KILN_GC_FLAG_DEAD) return NULL;
    return block;
}

static void kiln_c99_nursery_clear_bit(kiln_gc_heap *heap, const kiln_gc_block *block) {
    const char *ptr = (const char *)block;
    char *base;
    char *limit;
    uint64_t *bitmap;
    size_t words;
    if (heap == NULL || block == NULL) return;
    if (kiln_c99_nursery_locate(heap, ptr, &base, &limit, &bitmap, &words)) {
        size_t bit_idx = (size_t)(ptr - base) >> 4;
        if (bit_idx >> 6 < words) bitmap[bit_idx >> 6] &= ~((uint64_t)1 << (bit_idx & 63));
    }
}

static kiln_gc_block *kiln_c99_gc_lookup_nursery(const kiln_gc_heap *heap, const void *payload) {
    const char *p = (const char *)payload;
    char *base;
    char *limit;
    uint64_t *bitmap;
    size_t words;
    kiln_gc_block *block;
    if (heap == NULL || payload == NULL) return NULL;
    if ((uintptr_t)p >= sizeof(kiln_gc_block) &&
        kiln_c99_nursery_locate(heap, p, &base, &limit, &bitmap, &words)) {
        block = kiln_c99_nursery_block_at(base, limit, bitmap, words, p - sizeof(kiln_gc_block));
        if (block != NULL) return block;
    }
    if ((uintptr_t)p >= sizeof(kiln_string) + sizeof(kiln_gc_block)) {
        const char *q = p - sizeof(kiln_string);
        if (kiln_c99_nursery_locate(heap, q, &base, &limit, &bitmap, &words)) {
            block = kiln_c99_nursery_block_at(base, limit, bitmap, words, q - sizeof(kiln_gc_block));
            if (block != NULL && ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING ||
                                  (block->kind & KILN_GC_KIND_MASK) == KILN_GC_SLICED_STRING)) return block;
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
        if (block != NULL && ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING ||
                              (block->kind & KILN_GC_KIND_MASK) == KILN_GC_SLICED_STRING)) return block;
    }
    return NULL;
}

/* Keep `payload` a root until here. The collector finds a C frame's pointers by scanning the
 * stack, and a pointer whose last use is behind it has usually left the stack by the time the
 * frame allocates again; a caller that will hand the payload to something after that -- the
 * queue's drop callback, a finaliser -- names the point it must survive to with this. */
static inline void kiln_c99_gc_keep_alive(const void *payload) {
    __asm__ volatile("" : : "r"(payload) : "memory");
}

/* The kind of a heap payload, read from the block header in front of it. The header is exactly
 * one `kiln_gc_block` before the payload whatever its origin -- nursery, slab or malloc, see
 * `kiln_c99_gc_alloc_ex` above -- so this is pointer arithmetic and one load. */
static inline uint32_t kiln_c99_gc_payload_kind(const void *payload) {
    return ((const kiln_gc_block *)payload - 1)->kind & KILN_GC_KIND_MASK;
}

/* The kind of the payload a boxed pointer names.
 *
 * Every pointer an object, buffer or generator prefix can box came out of `kiln_c99_gc_alloc_ex`,
 * so its kind is the header read above. This used to be the heap search instead -- a nursery
 * bitmap probe, a binary search over the slabs, then the interior fallback -- and that search was
 * the largest single cost of every array and collection method call the emitted C makes, and of
 * every boxed pointer crossing to a `kiln_value`; it answered the same word. The search remains
 * the tool for a word that may not be a pointer at all, which is the conservative scanner's
 * question and never this one. */
static __attribute__((noinline)) void kiln_c99_gc_kind_check(const void *ptr, uint32_t kind) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_gc_block *block = heap == NULL ? NULL : kiln_c99_gc_lookup(heap, ptr);
    if (block == NULL || (block->kind & KILN_GC_KIND_MASK) != kind) __builtin_trap();
}

static uint32_t kiln_c99_gc_kind_of_ptr(const void *ptr) {
    uint32_t kind;
    if (ptr == NULL) return 0;
    kind = kiln_c99_gc_payload_kind(ptr);
    if (KILN_UNLIKELY(kiln_c99_gc_stress_checks)) kiln_c99_gc_kind_check(ptr, kind);
    return kind;
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
    const kiln_string *string;
    kiln_gc_heap *heap;
    kiln_gc_block *block;
    int ascii;
    if (value.ptr == NULL || value.len == 0) return 1;
    string = (const kiln_string *)value.ptr;
    if (string->unit_len == 0) return 1;
    if (kiln_string_is_utf16(string)) {
        for (size_t index = 0; index < string->unit_len; ++index)
            if (kiln_string_unit_at(string, index) >= 0x80) return 0;
        return 1;
    }
    heap = kiln_c99_gc_heap();
    block = heap != NULL ? kiln_c99_gc_lookup(heap, (void *)string) : NULL;
    if (block != NULL && (block->kind & KILN_GC_FLAG_ASCII_KNOWN) != 0) {
        return (block->kind & KILN_GC_FLAG_ASCII) != 0;
    }
    ascii = kiln_c99_ascii_scan((const char *)string->data, string->unit_len);
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
    {
        char *base;
        char *limit;
        uint64_t *bitmap;
        size_t words;
        if (kiln_c99_nursery_locate(heap, (const char *)address, &base, &limit, &bitmap, &words)) {
            return kiln_c99_gc_bitmap_block(bitmap, words, base, address);
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
    kiln_gc_block *block;
    if (word >= heap->lo && word < heap->hi) {
        block = kiln_c99_gc_lookup_interior(heap, word);
        if (block != NULL) kiln_c99_gc_mark_found(heap, block);
    }
    if ((word & KILN_TAG_MASK) >= KILN_TAG_STRING_PREFIX) {
        uintptr_t address = word & UINT64_C(0x0000FFFFFFFFFFFF);
        if (address < heap->lo || address >= heap->hi) return;
        block = kiln_c99_gc_lookup_interior(heap, address);
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
            if (entry->holder == NULL) {
                /* A transitioning store remembers the prototype it was cleared against
                 * (`kiln_c99_transition_store_clear`); forget one that died, so a new object at
                 * its address is never mistaken for it. */
                if (entry->via != NULL &&
                    kiln_c99_gc_lookup_interior(heap, (uintptr_t)entry->via) == NULL)
                    entry->via = NULL;
                continue;
            }
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

/* `zero_payload` is 0 only for a kind `kiln_c99_gc_trace` never reads a byte of -- see the
 * comment there, next to `kiln_c99_gc_alloc_pointer_free` below, for the invariant this trades
 * on. Every other caller goes through `kiln_c99_gc_alloc`, which always asks for zero: a mid-turn
 * collection can run before the payload is filled in, and the tracer must not walk a previous
 * occupant's pointers as if they belonged to the new block. */
/* Take a fresh chunk as the current one: a spare when there is one, else new memory. The
 * nursery goes inactive if memory is refused, and allocation continues in the slabs. */
static int kiln_c99_nursery_fresh_chunk(kiln_gc_heap *heap) {
    kiln_gc_nursery *nursery = &heap->nursery;
    kiln_nursery_chunk *chunk;
    if (nursery->spare != NULL) {
        chunk = nursery->spare;
        nursery->spare = chunk->next;
        nursery->spare_count -= 1;
    } else {
        chunk = kiln_c99_nursery_new_region(nursery);
        if (chunk == NULL) {
            nursery->region = NULL;
            nursery->base = NULL;
            nursery->bitmap = NULL;
            nursery->bump = NULL;
            nursery->limit = NULL;
            nursery->active = 0;
            return 0;
        }
    }
    kiln_c99_nursery_use(nursery, chunk);
    kiln_c99_gc_bound(heap, nursery->region, nursery->capacity);
    return 1;
}

/* The current chunk is full: queue it for the next collection and start another. */
static void kiln_c99_nursery_turn_chunk(kiln_gc_heap *heap) {
    kiln_gc_nursery *nursery = &heap->nursery;
    kiln_nursery_chunk *filled;
    if (nursery->bump == nursery->base) return;
    filled = kiln_c99_nursery_seal(nursery);
    filled->next = nursery->full;
    nursery->full = filled;
    nursery->full_bytes += (size_t)(nursery->bump - nursery->base);
    (void)kiln_c99_nursery_fresh_chunk(heap);
}

/* Memory a cell owns outside the heap -- a typed buffer's bytes -- counts toward the next
 * collection like the cell itself. */
static void kiln_c99_gc_note_external(size_t size) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap != NULL) heap->since_collection += size;
}

/* A filled-chunk collection is worthwhile while the live heap is small enough that the root
 * walk it buys is cheaper than retaining another nursery budget. Keep this predicate shared by
 * allocation and turn-boundary collection so the two entry points cannot drift. */
static inline int kiln_c99_nursery_pressure_due(const kiln_gc_heap *heap) {
    return heap->nursery.full != NULL &&
           heap->nursery.full_bytes >= heap->nursery.budget_now &&
           heap->survived <= heap->nursery.budget * KILN_NURSERY_SMALL_LIVE_BUDGETS;
}

/* The collection an allocation may owe before it hands anything out.
 *
 * Its own function because a caller taking a run of blocks at once -- `kiln_c99_gc_alloc_text_run`
 * below -- owes it once for the run rather than once per block, and the two must not drift: this
 * is where "is a mid-turn collection due" is decided for both. */
static inline void kiln_c99_gc_alloc_due(kiln_gc_heap *heap) {
    if (KILN_UNLIKELY(!heap->collecting && heap->enabled && !heap->fallback)) {
        /* A mid-turn collection scans the whole stack and sweeps every slab, so it earns its
         * keep only after a few megabytes, or after allocation has matched twice what
         * survived the last one; the turn's end keeps its own, lower, threshold. */
        size_t threshold = heap->survived * 2;
        int due;
        if (threshold < KILN_GC_MID_TURN_FLOOR) threshold = KILN_GC_MID_TURN_FLOOR;
        due = heap->since_collection >= threshold;
        /* A budget of filled chunks is collected while the live heap is small next to it: the
         * pass over them either resets a chunk, and allocation stays a bump into hot memory, or
         * promotes it alone. A large live heap makes every mark expensive; there the chunks
         * wait for the size-based trigger above. */
        if (!due && kiln_c99_nursery_pressure_due(heap)) due = 1;
        if (heap->stress_every != 0 && --heap->stress_countdown == 0) {
            heap->stress_countdown = heap->stress_every;
            due = 1;
        }
        if (due) kiln_c99_gc_collect_mid_turn(heap);
    }
}

static void *kiln_c99_gc_alloc_ex(uint32_t kind, size_t size, int zero_payload) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t payload = size == 0 ? 1 : size;
    int class_index;
    kiln_gc_block *block;
    if (heap == NULL || size > ((size_t)-1) - sizeof(kiln_gc_block)) return NULL;
    kiln_c99_gc_alloc_due(heap);

    if (!kiln_c99_gc_is_tenured_kind(kind) && heap->nursery.active && (payload + sizeof(kiln_gc_block) <= (heap->nursery.capacity >> 2))) {
        size_t total = sizeof(kiln_gc_block) + payload;
        total = (total + 15) & ~((size_t)15);
        if (heap->nursery.bump + total > heap->nursery.limit) kiln_c99_nursery_turn_chunk(heap);
        if (heap->nursery.active && heap->nursery.bump + total <= heap->nursery.limit) {
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
            if (zero_payload) memset(block + 1, 0, payload);
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
        if (zero_payload) memset(block + 1, 0, payload);
        return block + 1;
    }
    block = (kiln_gc_block *)malloc(sizeof *block + payload);
    if (block == NULL) return NULL;
    if (kiln_c99_gc_ptrset_insert(&heap->large_table, block) != KILN_C99_OK) {
        free(block);
        return NULL;
    }
    block->kind = kind | KILN_GC_FLAG_LARGE;
    kiln_c99_gc_bound(heap, block, sizeof *block + payload);
    block->mark = 0;
    block->size = size;
    block->prev = NULL;
    block->next = heap->head;
    if (heap->head != NULL) heap->head->prev = block;
    heap->head = block;
    ++heap->count;
    heap->bytes += size;
    heap->since_collection += size;
    /* The large/malloc path has never zeroed: it was already relying on the same "kind fills its
     * own payload before anything else can observe it" property this function generalises. */
    return block + 1;
}

/* The bump that answers almost every allocation, and nothing else.
 *
 * `kiln_c99_gc_alloc_ex` is one function for every kind and every size, and no compiler will
 * inline it anywhere: the size-class decision, the slab refill and the large-block table sit in
 * it beside the bump. But a caller that knows its kind -- `kiln_c99_gc_alloc_text` knows it makes
 * a string, `kiln_c99_new_array` knows it makes an array -- can have the bump inlined into it
 * with the tenured-kind test folded away against the constant, and leave the frame only when the
 * bump misses. On the string benchmark the general entry cost about ten nanoseconds a call and
 * was the largest single line in the profile once splitting stopped allocating one part at a
 * time.
 *
 * Answers NULL for anything it does not take -- an inactive nursery, a block too large for one,
 * a chunk with no room left -- and the caller hands that to `kiln_c99_gc_alloc_ex` unchanged.
 * The header, the bitmap bit and the counters are written exactly as the general path writes
 * them, because this is that path's nursery arm and nothing else.
 *
 * The caller owes `kiln_c99_gc_alloc_due` first, and owes the payload the same treatment the
 * general path would give it: zeroed, unless the kind is one whose caller fills every field the
 * tracer and the finaliser read before either can run. */
static inline void *kiln_c99_gc_nursery_bump(kiln_gc_heap *heap, uint32_t kind, size_t size) {
    kiln_gc_nursery *nursery = &heap->nursery;
    size_t payload = size == 0 ? 1 : size;
    size_t total = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
    kiln_gc_block *block;
    size_t bit_idx;
    if (kiln_c99_gc_is_tenured_kind(kind) || !nursery->active ||
        sizeof(kiln_gc_block) + payload > (nursery->capacity >> 2) ||
        nursery->bump + total > nursery->limit) {
        return NULL;
    }
    block = (kiln_gc_block *)nursery->bump;
    bit_idx = (size_t)(nursery->bump - nursery->base) >> 4;
    nursery->bitmap[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
    nursery->bump += total;
    block->kind = kind | KILN_GC_FLAG_NURSERY;
    block->mark = 0;
    block->size = size;
    block->prev = NULL;
    block->next = NULL;
    nursery->count += 1;
    nursery->bytes += size;
    heap->count += 1;
    heap->bytes += size;
    heap->since_collection += size;
    return block + 1;
}

/* `kiln_c99_gc_alloc_ex` with its nursery arm inlined at the call site. For a caller whose kind
 * and payload shape are constants; everything else stays on the general entry. */
KILN_ALWAYS_INLINE static inline void *kiln_c99_gc_alloc_hot(uint32_t kind, size_t size,
                                                            int zero_payload) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (KILN_LIKELY(heap != NULL)) {
        void *payload;
        kiln_c99_gc_alloc_due(heap);
        payload = kiln_c99_gc_nursery_bump(heap, kind, size);
        if (KILN_LIKELY(payload != NULL)) {
            if (zero_payload) memset(payload, 0, size == 0 ? 1 : size);
            return payload;
        }
    }
    return kiln_c99_gc_alloc_ex(kind, size, zero_payload);
}

static void *kiln_c99_gc_alloc(uint32_t kind, size_t size) {
    return kiln_c99_gc_alloc_ex(kind, size, 1);
}

/* A non-zeroing allocation, for a kind whose `kiln_c99_gc_trace` case is a no-op (see the comment
 * there): such a kind's tracer walks no field of the payload, so a mid-turn collection landing
 * before the caller has filled the bytes in can never read a previous occupant's pointer out of
 * them. The kind's own header fields (anything the kind's read paths dereference before the
 * caller's first write) must still be set by the allocator or the immediate caller before either
 * can yield to another allocation -- `kiln_c99_gc_alloc_text` does this itself. Do not call this
 * for a kind that has, or later gains, a non-empty `kiln_c99_gc_trace` case, and do not call it
 * for a kind whose *finalizer* reads a field the allocator has not set yet on an early-return
 * path (`KILN_GC_TYPED_BUFFER` is exactly that case: its finalizer frees `->bytes`, which a
 * failed secondary `calloc` can leave unset, so it stays on the zeroing path). */
static void *kiln_c99_gc_alloc_pointer_free(uint32_t kind, size_t size) {
    return kiln_c99_gc_alloc_ex(kind, size, 0);
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
    block->kind = kind | KILN_GC_FLAG_LARGE;
    kiln_c99_gc_bound(heap, block, sizeof *block + payload);
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
 *   kiln_c99_intern_static canonical pointer, inserting, for a constant of the program; shared
 *                          across realms when sound, else pinned in the realm's table.
 *   kiln_c99_interned      canonical pointer or NULL, never inserting. The one a *read* uses,
 *                          because storage is total: a spelling the table has never seen is a
 *                          key no object anywhere has, and the read can skip the scan.
 *
 * Two tables answer them (ADR 0071): the process's shared table of constant keys, consulted
 * first, and the realm heap's own table of keys made at run time, second. A spelling is never in
 * both while its realm lives, so the first answer is the only one.
 *
 * Canonical pointers are distinct as pointers: a block is adopted as canonical only when its
 * size is exactly the key's length, so two keys can never share an address with different
 * lengths, and the empty key is one process-wide header rather than whichever empty string came
 * first. */
/* The empty key: one for the process, in the shared block (see `kiln_c99_process`). */
#define kiln_c99_key_empty ((const kiln_string *)&kiln_c99_process()->empty_key)
static void kiln_c99_gc_mark_block(kiln_gc_heap *heap, const void *payload);
static kiln_string *kiln_c99_gc_alloc_key_copy(const kiln_string *view);
static const kiln_string *kiln_c99_intern_key(const kiln_string *string);
static const kiln_string *kiln_c99_intern_static_key(const kiln_string *string);
static const kiln_string *kiln_c99_intern_pinned_key(const kiln_string *string);
static int kiln_c99_register_static_keys(const kiln_string *const *keys, const kiln_string **out,
                                         size_t count);

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

static uint64_t kiln_c99_key_hash(const kiln_string *string) {
    /* Hash JavaScript UTF-16 code units rather than storage bytes. Latin1 and UTF-16 headers
     * with the same units therefore share one canonical key identity. Pack four units into an
     * eight-byte word so the hot path retains the old chunked mixing (one dependent multiply per
     * word), while the normalized 16-bit lanes make the result independent of storage width. */
    const uint64_t mix = UINT64_C(0xff51afd7ed558ccd);
    uint64_t hash = UINT64_C(0x9e3779b97f4a7c15) ^
                    (uint64_t)(string == NULL ? 0 : string->unit_len);
    size_t index;
    if (string == NULL) return hash ^ (hash >> 32);
    index = 0;
    while (string->unit_len - index >= 4) {
        uint64_t word = (uint64_t)kiln_string_unit_at(string, index) |
                        ((uint64_t)kiln_string_unit_at(string, index + 1) << 16) |
                        ((uint64_t)kiln_string_unit_at(string, index + 2) << 32) |
                        ((uint64_t)kiln_string_unit_at(string, index + 3) << 48);
        hash = (hash ^ word) * mix;
        hash ^= hash >> 29;
        index += 4;
    }
    if (index < string->unit_len) {
        uint64_t tail = 0;
        size_t lane = 0;
        while (index < string->unit_len) {
            tail |= (uint64_t)kiln_string_unit_at(string, index++) << (16 * lane++);
        }
        hash = (hash ^ tail) * mix;
        hash ^= hash >> 29;
    }
    return hash ^ (hash >> 32);
}

static kiln_key_entry *kiln_c99_key_probe(const kiln_key_table *table, const kiln_string *string,
                                          uint64_t hash) {
    size_t mask;
    size_t at;
    if (table->capacity == 0) return NULL;
    mask = table->capacity - 1;
    at = (size_t)hash & mask;
    for (;;) {
        kiln_key_entry *entry = &table->entries[at];
        if (entry->string == NULL) return NULL;
        if (entry->hash == hash && kiln_string_equal(entry->string, string))
            return entry;
        at = (at + 1) & mask;
    }
}

static void kiln_c99_key_place(kiln_key_table *table, kiln_key_entry entry) {
    size_t mask = table->capacity - 1;
    size_t at = (size_t)entry.hash & mask;
    while (table->entries[at].string != NULL) at = (at + 1) & mask;
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
        if (old[index].string != NULL) kiln_c99_key_place(table, old[index]);
    free(old);
    return KILN_C99_OK;
}

static const kiln_string *kiln_c99_key_insert(kiln_key_table *table, const kiln_string *string,
                                              uint64_t hash, int pinned) {
    kiln_key_entry entry;
    if ((table->count + 1) * 2 > table->capacity && kiln_c99_key_grow(table) != KILN_C99_OK)
        return NULL;
    entry.string = string;
    entry.hash = hash;
    entry.pinned = pinned;
    kiln_c99_key_place(table, entry);
    return string;
}

/* Linear probing deletes by shifting the entries after the hole back over it: an entry moves
 * into the hole when the hole lies on the probe path from its home slot to where it sits. */
static void kiln_c99_key_delete_at(kiln_key_table *table, size_t hole) {
    size_t mask = table->capacity - 1;
    size_t at = (hole + 1) & mask;
    table->entries[hole].string = NULL;
    table->count -= 1;
    while (table->entries[at].string != NULL) {
        size_t home = (size_t)table->entries[at].hash & mask;
        if (((hole - home) & mask) < ((at - home) & mask)) {
            table->entries[hole] = table->entries[at];
            table->entries[at].string = NULL;
            hole = at;
        }
        at = (at + 1) & mask;
    }
}

/* ---- The shared key table (ADR 0071) -------------------------------------------------------
 *
 * Every lookup consults the shared table first and the realm's own table second; a spelling is
 * never in both while the realm is alive (see `kiln_c99_intern_static_key` for why), so the first
 * answer found is the only one. A shared key is a malloc'd headed string outside every heap: the
 * collector's lookups do not find it and so never mark, sweep or purge it, like any constant. */
static const kiln_string *kiln_c99_shared_key_probe(const kiln_string *string, uint64_t hash) {
    const kiln_shared_key_table *table =
        __atomic_load_n(&kiln_c99_process()->keys, __ATOMIC_ACQUIRE);
    size_t mask;
    size_t at;
    if (table == NULL) return NULL;
    mask = table->capacity - 1;
    at = (size_t)hash & mask;
    for (;;) {
        const kiln_key_entry *entry = &table->entries[at];
        const kiln_string *candidate = __atomic_load_n(&entry->string, __ATOMIC_ACQUIRE);
        if (candidate == NULL) return NULL;
        if (entry->hash == hash && kiln_string_equal(candidate, string)) return candidate;
        at = (at + 1) & mask;
    }
}

/* Whether `ikey` -- already canonical -- is a shared key: its own spelling finds it in the shared
 * table. The empty key is the process's, so it counts. */
static int kiln_c99_key_is_shared(const kiln_string *ikey) {
    if (ikey == NULL) return 0;
    if (ikey->unit_len == 0) return ikey == kiln_c99_key_empty;
    return kiln_c99_shared_key_probe(ikey, kiln_c99_key_hash(ikey)) == ikey;
}

static void kiln_c99_shared_key_place(kiln_shared_key_table *table, const kiln_string *string,
                                      uint64_t hash) {
    size_t mask = table->capacity - 1;
    size_t at = (size_t)hash & mask;
    while (table->entries[at].string != NULL) at = (at + 1) & mask;
    table->entries[at].hash = hash;
    table->entries[at].pinned = 1;
    __atomic_store_n(&table->entries[at].string, string, __ATOMIC_RELEASE);
    table->count += 1;
}

/* Insert under the process lock. The copy is made before it is placed, and placed with a release
 * store, so a reader that sees the pointer sees the bytes; a table that must grow is rebuilt
 * whole and published with one store. */
static const kiln_string *kiln_c99_shared_key_insert(kiln_shared_state *process,
                                                     const kiln_string *string, uint64_t hash) {
    kiln_shared_key_table *table = process->keys;
    kiln_string *copy;
    size_t storage;
    uint32_t enc = string->flags & (KILN_STRING_LATIN1 | KILN_STRING_UTF16);
    if (!kiln_string_storage_size(string->unit_len, enc, &storage) ||
        storage > SIZE_MAX - sizeof *copy - 1)
        return NULL;
    if (table == NULL || (table->count + 1) * 2 > table->capacity) {
        size_t capacity = table == NULL ? 1024 : table->capacity * 2;
        kiln_shared_key_table *grown;
        size_t index;
        if (capacity > (SIZE_MAX - sizeof *grown) / sizeof(kiln_key_entry)) return NULL;
        grown = (kiln_shared_key_table *)calloc(1, sizeof *grown + capacity * sizeof(kiln_key_entry));
        if (grown == NULL) return NULL;
        grown->capacity = capacity;
        if (table != NULL) {
            for (index = 0; index < table->capacity; ++index)
                if (table->entries[index].string != NULL)
                    kiln_c99_shared_key_place(grown, table->entries[index].string,
                                              table->entries[index].hash);
        }
        __atomic_store_n(&process->keys, grown, __ATOMIC_RELEASE);
        table = grown;
    }
    /* `storage` counts the header; the bytes follow it, then a NUL for `%s` diagnostics. */
    copy = (kiln_string *)malloc(storage + 1);
    if (copy == NULL) return NULL;
    copy->unit_len = string->unit_len;
    copy->flags = enc;
    copy->data = (const void *)(copy + 1);
    if (storage > sizeof *copy) memcpy((void *)copy->data, string->data, storage - sizeof *copy);
    ((char *)copy)[storage] = '\0';
    kiln_c99_shared_key_place(table, copy, hash);
    return copy;
}

const kiln_string *kiln_c99_interned_key_in(kiln_gc_heap *heap, const kiln_string *string) {
    const kiln_key_entry *entry;
    const kiln_string *shared;
    uint64_t hash;
    if (string == NULL || string->unit_len == 0) return kiln_c99_key_empty;
    hash = kiln_c99_key_hash(string);
    shared = kiln_c99_shared_key_probe(string, hash);
    if (shared != NULL) return shared;
    if (heap == NULL) return NULL;
    entry = kiln_c99_key_probe(&heap->keys, string, hash);
    return entry == NULL ? NULL : entry->string;
}

const kiln_string *kiln_c99_interned_key(const kiln_string *string) {
    return kiln_c99_interned_key_in(kiln_c99_gc_heap(), string);
}

static kiln_string *kiln_c99_gc_alloc_key_copy(const kiln_string *view) {
    size_t storage;
    size_t total;
    kiln_string *copy;
    uint32_t enc = view != NULL ? (view->flags & (KILN_STRING_LATIN1 | KILN_STRING_UTF16)) : 0;
    if (view == NULL || !kiln_string_storage_size(view->unit_len, enc, &storage) ||
        storage > SIZE_MAX - sizeof *copy) {
        return NULL;
    }
    total = storage;
    copy = (kiln_string *)kiln_c99_gc_alloc_tenured(KILN_GC_STRING, total);
    if (copy == NULL) return NULL;
    copy->unit_len = view->unit_len;
    copy->flags = enc;
    copy->data = (const void *)(copy + 1);
    if (storage > sizeof *copy) {
        memcpy((void *)copy->data, view->data, storage - sizeof *copy);
    }
    return copy;
}

/* Byte callers are legacy fixed-ASCII adapters. They create a borrowed Latin1 view for the
 * duration of this call; only the key APIs retain the returned canonical headed string. */
static const kiln_string *kiln_c99_intern(const char *bytes, size_t len) {
    if (len > UINT32_MAX) return NULL;
    kiln_string view = {(uint32_t)len, KILN_STRING_LATIN1, bytes};
    return kiln_c99_intern_key(&view);
}

static const kiln_string *kiln_c99_interned_in(kiln_gc_heap *heap, const char *bytes,
                                               size_t len) {
    if (len > UINT32_MAX) return NULL;
    kiln_string view = {(uint32_t)len, KILN_STRING_LATIN1, bytes};
    return kiln_c99_interned_key_in(heap, &view);
}

static const kiln_string *kiln_c99_interned(const char *bytes, size_t len) {
    return kiln_c99_interned_in(kiln_c99_gc_heap(), bytes, len);
}

static const kiln_string *kiln_c99_intern_static(const char *bytes, size_t len) {
    if (len > UINT32_MAX) return NULL;
    kiln_string view = {(uint32_t)len, KILN_STRING_LATIN1, bytes};
    return kiln_c99_intern_static_key(&view);
}

/* Legacy byte callers above are being migrated to headed views. The canonical implementations
 * below are the only routines that insert or probe semantic property keys. */
const kiln_string *kiln_c99_intern_key(const kiln_string *string) {
    kiln_gc_heap *heap;
    uint64_t hash;
    kiln_key_entry *entry;
    kiln_string *copy;
    const kiln_string *shared;
    if (string == NULL || string->unit_len == 0) return kiln_c99_key_empty;
    hash = kiln_c99_key_hash(string);
    shared = kiln_c99_shared_key_probe(string, hash);
    if (shared != NULL) return shared;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    entry = kiln_c99_key_probe(&heap->keys, string, hash);
    if (entry != NULL) return entry->string;
    copy = kiln_c99_gc_alloc_key_copy(string);
    if (copy == NULL) return NULL;
    return kiln_c99_key_insert(&heap->keys, copy, hash, 0);
}

/* A spelling that must stay canonical for the realm's whole life but is not a constant of the
 * program -- a symbol's reserved key, which a realm makes as many of as it likes. The shared table
 * still answers first; otherwise the realm's own entry is pinned (a new one is copied into a
 * tenured block), so the collector marks the copy and the caller's storage is never borrowed. */
const kiln_string *kiln_c99_intern_pinned_key(const kiln_string *string) {
    kiln_gc_heap *heap;
    uint64_t hash;
    kiln_key_entry *entry;
    kiln_string *copy;
    const kiln_string *shared;
    if (string == NULL || string->unit_len == 0) return kiln_c99_key_empty;
    hash = kiln_c99_key_hash(string);
    shared = kiln_c99_shared_key_probe(string, hash);
    if (shared != NULL) return shared;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    entry = kiln_c99_key_probe(&heap->keys, string, hash);
    if (entry != NULL) {
        entry->pinned = 1;
        return entry->string;
    }
    copy = kiln_c99_gc_alloc_key_copy(string);
    if (copy == NULL) return NULL;
    return kiln_c99_key_insert(&heap->keys, copy, hash, 1);
}

/* A constant of the program: a unit's constant-table key, a prelude literal. These are what the
 * shared table is for (ADR 0071), and the answer is a shared key whenever that is sound: the
 * caller must use the pointer returned here, never its own literal's address.
 *
 * Every constant a linked unit names, and the built-ins' names, were registered before the first
 * heap (see `kiln_c99_register_static_keys`), so for them the first probe answers, shared, however
 * many realms are alive. What follows is the fallback for a constant nobody registered -- a
 * prelude literal outside the registered tables, or a unit whose registration came after a heap
 * existed -- and it keeps step 1's rule, because registration cannot vouch for these.
 *
 * Sound means no live realm already has the spelling as a key of its own, because a realm that
 * did would then find the shared key first and lose every property stored under its own. The
 * calling realm's table is checked directly; any other live realm's cannot be, so the spelling
 * joins the shared table only while the caller's heap is the only live one. Otherwise -- a second
 * realm is alive, or this realm made the spelling at run time before any unit named it -- the
 * key is this realm's own, pinned, exactly as every constant key was before shared keys. NULL
 * only when a table or the heap cannot grow; a unit that gets NULL for one of its keys fails its
 * entry with out-of-memory rather than run a site with a pointer that is not canonical. */
const kiln_string *kiln_c99_intern_static_key(const kiln_string *string) {
    kiln_gc_heap *heap;
    uint64_t hash;
    const kiln_string *shared;
    kiln_shared_state *process;
    if (string == NULL || string->unit_len == 0) return kiln_c99_key_empty;
    hash = kiln_c99_key_hash(string);
    shared = kiln_c99_shared_key_probe(string, hash);
    if (shared != NULL) return shared;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    if (kiln_c99_key_probe(&heap->keys, string, hash) == NULL) {
        process = kiln_c99_process();
        kiln_c99_process_lock(process);
        shared = kiln_c99_shared_key_probe(string, hash);
        if (shared == NULL && process->live_heaps <= 1 && heap->serial != 0)
            shared = kiln_c99_shared_key_insert(process, string, hash);
        kiln_c99_process_unlock(process);
        if (shared != NULL) return shared;
    }
    return kiln_c99_intern_pinned_key(string);
}

/* ---- Registration: the program's constants, before the first realm (ADR 0071) --------------
 *
 * Every unit registers its constant keys from a load-time constructor (`{sym}_register_keys`,
 * emitted beside its key table), and the prelude registers the built-ins' own from one of its
 * own (`kiln_c99_register_builtin_keys`, in `surface.c`). Constructors run before `main` -- and a
 * shared library's run when it is loaded -- so the linker's own list of the objects it linked is
 * the enumeration: no launcher, embedding host or library entry point has to name its units, and
 * a unit's cache key depends on nothing but the unit.
 *
 * Only while no heap is live. A heap that exists may already hold one of these spellings as a key
 * of its own, and a shared key published over it would hide every property stored under it; a
 * registration that comes too late (a library loaded into a process whose realms are running)
 * does nothing and says so, and those constants take the run-time path in
 * `kiln_c99_intern_static_key` exactly as before. 1 means every key named is now a shared key
 * (the empty key needs no entry: it is the process's already); 0 means too late, or out of
 * memory, and that some may not be. */
static int kiln_c99_register_static_keys_locked(kiln_shared_state *process,
                                                const kiln_string *const *keys,
                                                const kiln_string **out, size_t count) {
    size_t index;
    if (process->live_heaps != 0) return 0;
    for (index = 0; index < count; ++index) {
        const kiln_string *key = keys[index];
        const kiln_string *shared;
        uint64_t hash;
        if (key == NULL) return 0;
        if (key->unit_len == 0) {
            shared = &process->empty_key;
        } else {
            hash = kiln_c99_key_hash(key);
            shared = kiln_c99_shared_key_probe(key, hash);
            if (shared == NULL) shared = kiln_c99_shared_key_insert(process, key, hash);
            if (shared == NULL) return 0;
        }
        if (out != NULL) out[index] = shared;
    }
    return 1;
}

/* `out`, when given, receives each key's shared pointer: the unit's own key array, filled once
 * for every realm. */
int kiln_c99_register_static_keys(const kiln_string *const *keys, const kiln_string **out,
                                  size_t count) {
    kiln_shared_state *process = kiln_c99_process();
    int registered;
    kiln_c99_process_lock(process);
    registered = kiln_c99_register_static_keys_locked(process, keys, out, count);
    kiln_c99_process_unlock(process);
    return registered;
}

/* Byte spellings, for the prelude's own tables of C strings. */
static int kiln_c99_register_static_text(kiln_shared_state *process, const char *bytes, size_t len) {
    kiln_string view;
    const kiln_string *key = &view;
    if (bytes == NULL || len > UINT32_MAX) return 1;
    view.unit_len = (uint32_t)len;
    view.flags = KILN_STRING_LATIN1;
    view.data = bytes;
    return kiln_c99_register_static_keys_locked(process, &key, NULL, 1);
}

/* A string block released by hand (`kiln_c99_gc_release`) must not stay canonical. */
static void kiln_c99_key_forget(kiln_gc_heap *heap, const kiln_string *string) {
    kiln_key_table *table = &heap->keys;
    size_t mask;
    size_t at;
    if (table->capacity == 0) return;
    mask = table->capacity - 1;
    at = (size_t)kiln_c99_key_hash(string) & mask;
    while (table->entries[at].string != NULL) {
        if (table->entries[at].string == string) {
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
        if (entry->string != NULL && entry->pinned) kiln_c99_gc_mark_block(heap, entry->string);
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
        if (entry->string == NULL || entry->pinned) {
            ++index;
            continue;
        }
        block = kiln_c99_gc_lookup(heap, entry->string);
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
/* Release a payload the caller allocated itself and knows to be whole and live: a microtask the
 * queue has run, a settler, a store. The header in front of it says where it lives -- nursery,
 * large block, or a slab, whose base is the address rounded down to the slab size -- so no
 * search is made. `kiln_c99_gc_release` below is the general form, for a pointer that may be a
 * string's bytes or otherwise interior; it searches. */
static void kiln_c99_gc_release_cell(void *payload) {
    kiln_gc_heap *heap;
    kiln_gc_block *block;
    if (payload == NULL) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
    block = (kiln_gc_block *)payload - 1;
    /* Already on a free list, or already finalised: a collection got here first. The general
     * release answers that case by failing its search; this one reads the header's own word. */
    if (block->size == KILN_GC_SLOT_FREE || (block->kind & KILN_GC_FLAG_DEAD)) return;
    if (block->kind & KILN_GC_FLAG_NURSERY) {
        if ((block->kind & KILN_GC_FLAG_DEAD) == 0) {
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
    kiln_c99_gc_finalize(block);
    heap->freed_blocks += 1;
    heap->freed_bytes += block->size;
    if (block->kind & KILN_GC_FLAG_LARGE) {
        kiln_c99_gc_unlink(heap, block);
        kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        free(block);
        return;
    }
    {
        kiln_gc_slab *slab =
            (kiln_gc_slab *)((uintptr_t)block & ~(uintptr_t)(KILN_GC_SLAB_SIZE - 1));
        heap->count -= 1;
        heap->bytes -= block->size;
        block->size = KILN_GC_SLOT_FREE;
        block->kind = KILN_GC_NONE;
        block->next = slab->free_head;
        slab->free_head = block;
        if (++slab->free_count == 1) kiln_c99_gc_slab_link_free(heap, slab);
        if (slab->free_count == slab->slot_count) kiln_c99_gc_slab_maybe_destroy(heap, slab);
    }
}

/* =============================================================================================
 * Zero-Copy Region Splicing and Sublist Transfer across Realm Heaps (ADR 0057, Issue 02)
 * ============================================================================================= */

/*
 * Splice a sublist of GC blocks [first, last] from from_heap to to_heap in O(1) time.
 * Both first and last must be KILN_GC_FLAG_LARGE blocks linked in from_heap->head.
 */
static void kiln_c99_gc_splice_sublist_range(kiln_gc_heap *from_heap, kiln_gc_heap *to_heap,
                                             kiln_gc_block *first, kiln_gc_block *last) {
    kiln_gc_block *cur;
    if (from_heap == NULL || to_heap == NULL || first == NULL || last == NULL || from_heap == to_heap) {
        return;
    }

    /* 1. Unlink sublist [first, last] from from_heap->head in O(1) */
    if (first->prev != NULL) {
        first->prev->next = last->next;
    } else if (from_heap->head == first) {
        from_heap->head = last->next;
    }
    if (last->next != NULL) {
        last->next->prev = first->prev;
    }
    first->prev = NULL;

    /* 2. Update address bounds and table entries across the spliced chain */
    cur = first;
    while (cur != NULL) {
        kiln_gc_block *next = cur->next;
        cur->mark = 0; /* Reset mark epoch so target heap's GC tracks it cleanly */
        kiln_c99_gc_ptrset_remove(&from_heap->large_table, cur);
        kiln_c99_gc_ptrset_insert(&to_heap->large_table, cur);
        kiln_c99_gc_bound(to_heap, cur, sizeof(kiln_gc_block) + cur->size);

        if (from_heap->count > 0) from_heap->count -= 1;
        if (from_heap->bytes >= cur->size) {
            from_heap->bytes -= cur->size;
        } else {
            from_heap->bytes = 0;
        }

        to_heap->count += 1;
        to_heap->bytes += cur->size;
        to_heap->since_collection += cur->size;

        if (cur == last) break;
        cur = next;
    }

    /* 3. Splice [first, last] into to_heap->head in O(1) */
    last->next = to_heap->head;
    if (to_heap->head != NULL) {
        to_heap->head->prev = last;
    }
    to_heap->head = first;
}

/*
 * Splice the unaliased object subgraph rooted at root_ptr from from_heap into to_heap.
 * In the primary path (tenured/large blocks), this executes in O(1) pointer reassignments (< 10 ns).
 * Returns the valid payload pointer in to_heap.
 */
static void *kiln_c99_gc_splice_sublist(kiln_gc_heap *from_heap, kiln_gc_heap *to_heap, void *root_ptr) {
    kiln_gc_block *root_block;
    size_t payload_size;
    kiln_gc_block *promoted;

    if (root_ptr == NULL) return NULL;
    if (from_heap == NULL) from_heap = kiln_c99_gc_heap();
    if (from_heap == NULL || to_heap == NULL || from_heap == to_heap) return root_ptr;

    root_block = ((kiln_gc_block *)root_ptr) - 1;
    if (root_block->size == KILN_GC_SLOT_FREE || (root_block->kind & KILN_GC_FLAG_DEAD) != 0) {
        return NULL;
    }

    /* Standalone large/tenured block: pure O(1) sublist pointer splicing */
    if ((root_block->kind & KILN_GC_FLAG_LARGE) != 0) {
        kiln_c99_gc_splice_sublist_range(from_heap, to_heap, root_block, root_block);
        return root_ptr;
    }

    /* Fallback for nursery or slab blocks: promote into to_heap as a tenured large block */
    payload_size = root_block->size;
    promoted = (kiln_gc_block *)malloc(sizeof(kiln_gc_block) + payload_size);
    if (promoted == NULL) return NULL;

    memcpy(promoted + 1, root_ptr, payload_size);
    promoted->kind = (root_block->kind & KILN_GC_KIND_MASK) | KILN_GC_FLAG_LARGE;
    promoted->mark = 0;
    promoted->size = payload_size;
    promoted->prev = NULL;
    promoted->next = to_heap->head;
    if (to_heap->head != NULL) to_heap->head->prev = promoted;
    to_heap->head = promoted;

    kiln_c99_gc_bound(to_heap, promoted, sizeof(kiln_gc_block) + payload_size);
    kiln_c99_gc_ptrset_insert(&to_heap->large_table, promoted);
    to_heap->count += 1;
    to_heap->bytes += payload_size;
    to_heap->since_collection += payload_size;

    /* Release original block from from_heap */
    kiln_c99_gc_release_cell(root_ptr);
    return promoted + 1;
}

/*
 * Overwrite sender's local slot with undefined to enforce destructive read semantics (ADR 0057).
 */
static inline void kiln_c99_destructive_read_val(kiln_val *slot) {
    if (slot != NULL) {
        *slot = KILN_VAL_UNDEFINED;
    }
}

static inline void kiln_c99_destructive_read_value(kiln_value *slot) {
    if (slot != NULL) {
        slot->tag = KILN_VALUE_UNDEFINED;
        slot->len = 0;
        slot->bits = 0;
    }
}

/*
 * Host operation __kiln_transfer_region(ptr)
 * Unlinks the unaliased region from the current realm's heap so it can be safely sent
 * across threads without data races or concurrent GC interference.
 */
static void *__kiln_transfer_region(void *ptr) {
    kiln_gc_heap *heap;
    kiln_gc_block *block;

    if (ptr == NULL) return NULL;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return ptr;

    block = ((kiln_gc_block *)ptr) - 1;
    if (block->size == KILN_GC_SLOT_FREE || (block->kind & KILN_GC_FLAG_DEAD) != 0) {
        return NULL;
    }

    if ((block->kind & KILN_GC_FLAG_LARGE) != 0) {
        kiln_c99_gc_unlink(heap, block);
        kiln_c99_gc_ptrset_remove(&heap->large_table, block);
    }
    return ptr;
}

/* Destructively transfers a NaN-boxed local variable slot, leaving undefined in the sender */
static inline kiln_val kiln_c99_transfer_val_destructive(kiln_val *slot, kiln_gc_heap *target_heap) {
    kiln_val val;
    uint64_t tag;

    if (slot == NULL) return KILN_VAL_UNDEFINED;
    val = *slot;
    *slot = KILN_VAL_UNDEFINED; /* Destructive read enforced */

    tag = val & KILN_TAG_MASK;
    if (tag == KILN_TAG_OBJECT_PREFIX ||
        tag == KILN_TAG_STRING_PREFIX ||
        tag == KILN_TAG_BUFFER_PREFIX) {
        void *ptr = (void *)(uintptr_t)(val & KILN_PAYLOAD_MASK);
        if (target_heap != NULL) {
            void *transferred = kiln_c99_gc_splice_sublist(kiln_c99_gc_heap(), target_heap, ptr);
            return tag | ((uint64_t)(uintptr_t)transferred & KILN_PAYLOAD_MASK);
        } else {
            void *unlinked = __kiln_transfer_region(ptr);
            return tag | ((uint64_t)(uintptr_t)unlinked & KILN_PAYLOAD_MASK);
        }
    }
    return val;
}

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
            if ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING) {
                const kiln_string *text = (const kiln_string *)payload;
                kiln_c99_key_forget(heap, text);
            }
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
    if ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_STRING) {
        /* Every string block is headed, an interned key's included, so the entry to forget is
         * the one whose bytes are the header's. */
        const kiln_string *text = (const kiln_string *)payload;
        kiln_c99_key_forget(heap, text);
    }
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

/* Every kind that falls to the `default:` case below owns no pointer this tracer walks, which is
 * exactly the invariant `kiln_c99_gc_alloc_pointer_free` trades its zeroing for: a mid-turn
 * collection that lands before such a kind's payload is filled in traces the block (this
 * function runs on it) but reads none of the bytes the caller has not written yet, so garbage
 * left over from the slab or nursery slot's previous occupant is never mistaken for a pointer.
 * Adding a field to one of these kinds that this switch must start marking means it can no
 * longer allocate through `kiln_c99_gc_alloc_pointer_free` -- move its case out of `default` and
 * its allocation back onto `kiln_c99_gc_alloc` in the same change. */
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
        /* Generated names may point at static headers; runtime-created/inferred names point at
         * GC strings.  The marker's lookup treats static headers as non-heap and marks the latter
         * without a second representation or an unsafe header-kind probe. */
        kiln_c99_gc_mark_block(heap, closure->name);
        kiln_c99_gc_mark_value(heap, &closure->source_value);
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
        kiln_c99_gc_mark_value(heap, &array->match_groups);
        break;
    }
    case KILN_GC_GENERATOR: {
        kiln_generator *frame = (kiln_generator *)payload;
        kiln_c99_gc_mark_value(heap, &frame->function);
        kiln_c99_gc_mark_value(heap, &frame->receiver);
        kiln_c99_gc_mark_value(heap, &frame->promise);
        kiln_c99_gc_mark_value(heap, &frame->sent);
        kiln_c99_gc_mark_value(heap, &frame->delegate);
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
    case KILN_GC_NODE_PROCESS_COMPLETION:
        kiln_c99_gc_mark_value(heap, &((kiln_node_process_completion *)payload)->promise);
        kiln_c99_gc_mark_value(heap, &((kiln_node_process_completion *)payload)->result);
        kiln_c99_gc_mark_value(heap, &((kiln_node_process_completion *)payload)->stdout_value);
        kiln_c99_gc_mark_value(heap, &((kiln_node_process_completion *)payload)->stderr_value);
        break;
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
        kiln_c99_gc_mark_value(heap, &symbol->description_value);
        break;
    }
    case KILN_GC_SLICED_STRING: {
        const kiln_sliced_string *sliced = (const kiln_sliced_string *)payload;
        if (sliced->parent != NULL) {
            kiln_c99_gc_mark_block(heap, sliced->parent);
        }
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
    case KILN_GC_STRING:
    case KILN_GC_SLICED_STRING:
    case KILN_GC_BYTES:
        break;
    case KILN_GC_OBJECT:
        free(((kiln_object *)payload)->properties);
        if (((kiln_object *)payload)->slots != ((kiln_object *)payload)->inline_slots) {
            free(((kiln_object *)payload)->slots);
        }
        free(((kiln_object *)payload)->elements);
        break;
    case KILN_GC_CLOSURE: {
        kiln_closure *closure = (kiln_closure *)payload;
        if (closure->upvalues != NULL && closure->upvalues != closure->inline_upvalues) {
            free(closure->upvalues);
        }
        break;
    }
    case KILN_GC_ARRAY: {
        kiln_array *arr = (kiln_array *)payload;
        if (kiln_c99_array_elements_owned(arr)) {
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
        if (((kiln_typed_buffer *)payload)->owns_bytes) {
            kiln_c99_bytes_retire(((kiln_typed_buffer *)payload)->bytes,
                                  ((kiln_typed_buffer *)payload)->byte_length);
        }
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
        free(response->out_headers);
        response->out_headers = NULL;
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
#define KILN_GC_SLAB_CLASSES \
    ((KILN_GC_SLAB_CLASS_MAX - KILN_GC_SLAB_CLASS_MIN) / KILN_GC_SLAB_CLASS_STEP + 1)

static void kiln_c99_gc_retain_survivor(kiln_gc_block *block);

static void kiln_c99_gc_sweep(kiln_gc_heap *heap, int orphan_marked) {
    kiln_gc_block *block = heap->head;
    kiln_gc_slab *slab = heap->all_slabs;
    size_t survived = 0;
    /* What each class had in use as this collection began is what the next turn of the same
     * loop will fill again: a slab the sweep empties is kept, up to that count, before the
     * general retention rule is consulted. Returning sixty slabs to the allocator and taking
     * sixty back at the next collection was a tenth of `string-ops` and went through the
     * kernel's reclaim path on the way. */
    uint32_t in_use_before[KILN_GC_SLAB_CLASSES];
    uint32_t retained[KILN_GC_SLAB_CLASSES];
    memset(in_use_before, 0, sizeof in_use_before);
    memset(retained, 0, sizeof retained);
    if (!orphan_marked) {
        const kiln_gc_slab *each;
        for (each = heap->all_slabs; each != NULL; each = each->all_next) {
            if (each->free_count < each->slot_count) in_use_before[each->class_index] += 1;
        }
    }
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
            kiln_c99_gc_retain_survivor(block);
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
                kiln_c99_gc_retain_survivor(slot);
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
                heap->class_slab_count[slab->class_index] -= 1;
                kiln_c99_gc_aligned_free(slab);
            }
            /* else: leaked with a live survivor inside; the heap is being torn down anyway. */
        } else if (slab->free_count == slab->slot_count) {
            if (retained[slab->class_index] < in_use_before[slab->class_index]) {
                retained[slab->class_index] += 1;
            } else {
                kiln_c99_gc_slab_maybe_destroy(heap, slab);
            }
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
                kiln_c99_nursery_free_region(&heap->nursery, chunk);
            } else {
                pchunk = &chunk->next;
            }
        }
    }
    heap->survived = survived;
    heap->since_collection = 0;
}

/* Walk one nursery chunk after a mark: finalise what died, and say how many blocks survived. */
static size_t kiln_c99_nursery_settle(kiln_gc_heap *heap, char *base, char *limit,
                                      uint64_t *bitmap, size_t words) {
    size_t survivors = 0;
    size_t dead = 0;
    size_t reclaimed = 0;
    char *curr = base;
    while (curr < limit) {
        kiln_gc_block *b = (kiln_gc_block *)curr;
        size_t payload = b->size == 0 ? 1 : b->size;
        size_t total = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
        if ((b->kind & KILN_GC_FLAG_DEAD) == 0) {
            if (b->mark == heap->epoch) {
                survivors++;
            } else {
                size_t bit_idx = (size_t)(curr - base) >> 4;
                dead++;
                reclaimed += b->size;
                kiln_c99_gc_finalize(b);
                b->kind |= KILN_GC_FLAG_DEAD;
                if (bit_idx >> 6 < words) bitmap[bit_idx >> 6] &= ~((uint64_t)1 << (bit_idx & 63));
            }
        }
        curr += total;
    }
    heap->count -= dead;
    heap->bytes -= reclaimed;
    heap->freed_blocks += dead;
    heap->freed_bytes += reclaimed;
    if (heap->since_collection >= reclaimed) {
        heap->since_collection -= reclaimed;
    } else {
        heap->since_collection = 0;
    }
    return survivors;
}

/* An emptied chunk goes back to the spares, up to a budget's worth; beyond that it is freed. */
static void kiln_c99_nursery_spare(kiln_gc_heap *heap, kiln_nursery_chunk *chunk) {
    kiln_gc_nursery *nursery = &heap->nursery;
    size_t used_words = ((size_t)(chunk->limit - chunk->base) / 16 + 63) / 64;
    if (used_words > chunk->bitmap_words) used_words = chunk->bitmap_words;
    memset(chunk->bitmap, 0, used_words * sizeof(uint64_t));
    if (nursery->spare_count >= nursery->budget / nursery->capacity + 2) {
        kiln_c99_nursery_free_region(nursery, chunk);
        return;
    }
    chunk->limit = chunk->base;
    chunk->live_count = 0;
    chunk->next = nursery->spare;
    nursery->spare = chunk;
    nursery->spare_count += 1;
}

/* Every job still waiting in the promise lane is a root, exactly as the queued work item that
 * carried it used to be. A job that has been popped and is running is held in the drain's own
 * local, which the conservative stack scan finds -- the same thing that kept a popped microtask
 * cell alive. */
static void kiln_c99_gc_mark_promise_jobs(kiln_gc_heap *heap) {
    size_t index;
    for (index = heap->job_head; index < heap->job_tail; ++index) {
        kiln_microtask *job = &heap->jobs[index & heap->job_mask];
        kiln_c99_gc_mark_value(heap, &job->handler);
        kiln_c99_gc_mark_value(heap, &job->argument);
        kiln_c99_gc_mark_value(heap, &job->derived);
    }
}

static void kiln_c99_gc_collect(void *opaque, int force) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    size_t threshold;
    uint64_t before_blocks;
    uint64_t before_bytes;
    int nursery_has_allocs;
    int nursery_due;
    int tenured_due;
    if (heap == NULL || heap->collecting || heap->fallback) return;
    if (!heap->enabled && !heap->stress) return;

    nursery_has_allocs = heap->nursery.full != NULL ||
                         (heap->nursery.active && heap->nursery.bump > heap->nursery.base);
    threshold = heap->floor > heap->survived ? heap->floor : heap->survived;
    tenured_due = force || heap->stress || (heap->since_collection >= threshold);

    /* A partial current chunk is safe to leave in the nursery between turn boundaries.  The
     * allocator already triggers a mid-turn pass when the configured filled-chunk budget is
     * reached; the boundary pass should use that same pressure signal rather than collecting
     * every small allocation made by an await continuation.  Keep `nursery_has_allocs` separate:
     * forced, stressed, and tenured collections still settle a partial chunk below. */
    nursery_due = kiln_c99_nursery_pressure_due(heap);

    if (!tenured_due && !nursery_due) return;

    heap->collecting = 1;
    before_blocks = heap->freed_blocks;
    before_bytes = heap->freed_bytes;
    kiln_c99_gc_begin(heap);
    (void)kiln_realm_gc_mark_roots(heap);
    kiln_c99_key_mark_pinned(heap);
    kiln_c99_gc_mark_promise_jobs(heap);
    if (heap->mid_turn) kiln_c99_gc_scan_stack(heap);
    kiln_c99_gc_drain(heap);

    if (!heap->mark_failed) {
        kiln_c99_key_purge(heap);
        /* Mid-turn, the nursery is left as it is: its dead blocks wait for the turn's end,
         * when the whole region resets or promotes as before, and its live ones need no
         * promotion because nothing moves. The slabs, where a long turn's allocation goes
         * once the nursery is full, are swept below. */
        if (nursery_has_allocs) {
            kiln_nursery_chunk *chunk = heap->nursery.full;
            size_t reset_bytes = 0;
            size_t promoted_bytes = 0;
            heap->nursery.full = NULL;
            heap->nursery.full_bytes = 0;
            while (chunk != NULL) {
                kiln_nursery_chunk *next = chunk->next;
                size_t filled = (size_t)(chunk->limit - chunk->base);
                size_t survivors = kiln_c99_nursery_settle(heap, chunk->base, chunk->limit,
                                                           chunk->bitmap, chunk->bitmap_words);
                if (survivors == 0) {
                    reset_bytes += filled;
                    heap->nursery.resets++;
                    kiln_c99_nursery_spare(heap, chunk);
                } else {
                    promoted_bytes += filled;
                    chunk->live_count = survivors;
                    chunk->next = heap->nursery.promoted;
                    heap->nursery.promoted = chunk;
                    heap->nursery.promotions += survivors;
                }
                chunk = next;
            }
            if (promoted_bytes > reset_bytes) {
                if (heap->nursery.budget_now < heap->nursery.budget * KILN_NURSERY_BUDGET_CAP) {
                    heap->nursery.budget_now *= KILN_NURSERY_BUDGET_GROWTH;
                }
            } else {
                heap->nursery.budget_now = heap->nursery.budget;
            }
            if (heap->nursery.active && heap->nursery.base != NULL && heap->nursery.bump != NULL &&
                heap->nursery.bump > heap->nursery.base) {
                size_t survivors = kiln_c99_nursery_settle(heap, heap->nursery.base, heap->nursery.bump,
                                                           heap->nursery.bitmap, heap->nursery.bitmap_words);
                if (survivors == 0) {
                    size_t used_words = ((size_t)(heap->nursery.bump - heap->nursery.base) / 16 + 63) / 64;
                    if (used_words > heap->nursery.bitmap_words) used_words = heap->nursery.bitmap_words;
                    memset(heap->nursery.bitmap, 0, used_words * sizeof(uint64_t));
                    heap->nursery.bump = heap->nursery.base;
                    heap->nursery.count = 0;
                    heap->nursery.bytes = 0;
                    heap->nursery.resets++;
                } else {
                    kiln_nursery_chunk *promoted = kiln_c99_nursery_seal(&heap->nursery);
                    promoted->live_count = survivors;
                    promoted->next = heap->nursery.promoted;
                    heap->nursery.promoted = promoted;
                    heap->nursery.promotions += survivors;
                    (void)kiln_c99_nursery_fresh_chunk(heap);
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

/* A teardown survivor (a result the caller reads after `kiln_realm_destroy`) keeps its shape:
 * mark that realm-local shape and its realm-local ancestors retained, so the free below leaks
 * them instead. Shared shapes are never freed and need no mark. */
static void kiln_c99_shape_retain(kiln_shape *shape) {
    while (shape != NULL && !shape->shared && !shape->retained) {
        shape->retained = 1;
        shape = shape->parent;
    }
}

static void kiln_c99_gc_retain_survivor(kiln_gc_block *block) {
    if ((block->kind & KILN_GC_KIND_MASK) == KILN_GC_OBJECT)
        kiln_c99_shape_retain(((kiln_object *)(block + 1))->shape);
}

/* Free every realm-local shape this heap made, and its side table (ADR 0071). A retained shape
 * survives, but loses its edges: the children they name may be freed here, and a transition from
 * a dead realm's survivor would make a new child rather than follow one. */
static void kiln_c99_gc_free_local_shapes(kiln_gc_heap *heap) {
    kiln_shape *shape = heap->local_shapes;
    while (shape != NULL) {
        kiln_shape *next = shape->owned_next;
        kiln_shape_transition *edge = shape->transitions;
        while (edge != NULL) {
            kiln_shape_transition *after = edge->next_transition;
            free(edge);
            edge = after;
        }
        if (shape->retained) {
            shape->transitions = NULL;
            shape->num_transitions = 0;
            shape->owned_next = NULL;
        } else {
            free(shape);
        }
        shape = next;
    }
    heap->local_shapes = NULL;
    free(heap->local_edges);
    heap->local_edges = NULL;
    heap->local_edge_capacity = 0;
    heap->local_edge_count = 0;
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
    char *curr;
    if (heap->nursery.active && heap->nursery.base != NULL) {
        curr = heap->nursery.base;
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
    {
        kiln_nursery_chunk *chunk = heap->nursery.full;
        for (; chunk != NULL; chunk = chunk->next) {
            curr = chunk->base;
            while (curr < chunk->limit) {
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
    }
    kiln_c99_nursery_destroy(&heap->nursery);
    free(heap->large_table.table);
    free(heap->slab_table.table);
    free(heap->stack);
    free(heap->keys.entries);
    kiln_c99_gc_free_local_shapes(heap);
    kiln_c99_gc_heap_unregister(heap);
    /* Teardown does not run what is left in the lane: a realm being destroyed has no turn to run
     * a job in, and the values are about to be freed with the rest of the heap. */
    free(heap->jobs);
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

/* Allocation helpers for the kinds whose bytes are the value: strings and raw byte payloads.
 *
 * `KILN_GC_STRING` is one of the pointer-free kinds `kiln_c99_gc_trace` never reads a field of
 * (see the comment there), so this goes through `kiln_c99_gc_alloc_pointer_free`: every field of
 * the header below is set before `return`, and every caller of `kiln_c99_gc_alloc_text` fills the
 * `len` body bytes immediately after (`memcpy` or an equivalent loop) before doing anything else
 * that could yield to another allocation, so no one ever reads the slab or nursery slot's
 * previous occupant back out of them. */
static char *kiln_c99_gc_alloc_text(size_t len) {
    size_t storage;
    size_t total;
    kiln_string *s;
    if (len > UINT32_MAX || !kiln_string_storage_size((uint32_t)len, KILN_STRING_LATIN1, &storage) ||
        storage == SIZE_MAX) return NULL;
    total = storage + 1;
    if (total < storage) return NULL;
    s = (kiln_string *)kiln_c99_gc_alloc_hot(KILN_GC_STRING, total, 0);
    if (s == NULL) return NULL;
    s->unit_len = (uint32_t)len;
    s->flags = KILN_STRING_LATIN1;
    s->data = (const void *)(s + 1);
    char *bytes = (char *)(s + 1);
    bytes[len] = '\0';
    return bytes;
}

static char *kiln_c99_gc_alloc_text_tenured(size_t len) {
    size_t storage;
    size_t total;
    kiln_string *s;
    if (len > UINT32_MAX || !kiln_string_storage_size((uint32_t)len, KILN_STRING_LATIN1, &storage) ||
        storage == SIZE_MAX) return NULL;
    total = storage + 1;
    if (total < storage) return NULL;
    s = (kiln_string *)kiln_c99_gc_alloc_tenured(KILN_GC_STRING, total);
    if (s == NULL) return NULL;
    s->unit_len = (uint32_t)len;
    s->flags = KILN_STRING_LATIN1;
    s->data = (const void *)(s + 1);
    char *bytes = (char *)(s + 1);
    bytes[len] = '\0';
    return bytes;
}

/* A run of known-ASCII string blocks out of one nursery reservation.
 *
 * A `split` hands back one part per separator and every part is the same shape: a
 * `KILN_GC_STRING` block holding a `kiln_string` header and its bytes. Asked for one at a time,
 * each part pays the whole of the way in to `kiln_c99_gc_alloc_ex` -- the realm's heap out of
 * thread-local storage, the collection-due test, the tenured-kind and size-class decisions -- to
 * answer a question that cannot change between two parts of the same call. On a nine-field log
 * line that was the single largest cost of `split`, larger than the copying.
 *
 * So the question is asked once and the parts are carved out of one bump. Each part still gets
 * the block header and the nursery bitmap bit the general path would give it, and is still its
 * own block to the tracer, the sweeper and `kiln_c99_gc_lookup`; nothing downstream can tell a
 * carved part from an ordinary one.
 *
 * Answers with how many parts it placed. That is fewer than asked only when the run does not fit
 * the chunk being filled or the nursery is not taking allocations at all, and the caller
 * allocates the remainder the ordinary way -- a short answer is never an error. Each placed
 * part's header is complete and its `len` body bytes are the caller's to fill immediately, which
 * is `kiln_c99_gc_alloc_text`'s contract unchanged.
 *
 * The parts are left without an ASCII verdict like every other fresh string: the caller
 * splits units that are bytes, but bytes are not ASCII -- a Latin1 part would otherwise
 * read as ASCII from the block cache and serialize its raw units as UTF-8. The first
 * asker scans once and caches the true answer. */
static size_t kiln_c99_gc_alloc_text_run(size_t count, const size_t *lens, char **out) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_gc_nursery *nursery;
    size_t total = 0;
    size_t counted = 0;
    size_t index;
    char *cursor;
    if (heap == NULL || count == 0) return 0;
    kiln_c99_gc_alloc_due(heap);
    nursery = &heap->nursery;
    if (!nursery->active || nursery->bitmap == NULL) return 0;
    for (index = 0; index < count; ++index) {
        size_t payload = sizeof(kiln_string) + lens[index] + 1;
        size_t slot = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
        /* The same ceiling the general path applies: a block worth a quarter of a chunk is one
         * the nursery does not want, and one this cannot place either. */
        if (sizeof(kiln_gc_block) + payload > (nursery->capacity >> 2)) return 0;
        total += slot;
    }
    if (nursery->bump + total > nursery->limit) {
        kiln_c99_nursery_turn_chunk(heap);
        if (!nursery->active || nursery->bitmap == NULL ||
            nursery->bump + total > nursery->limit) {
            return 0;
        }
    }
    /* The counters are the run's, not each part's: five read-modify-writes per part over a
     * nine-part line was more of this loop than the header stores were. */
    cursor = nursery->bump;
    for (index = 0; index < count; ++index) {
        size_t len = lens[index];
        size_t payload = sizeof(kiln_string) + len + 1;
        size_t slot = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
        kiln_gc_block *block = (kiln_gc_block *)cursor;
        size_t bit_idx = (size_t)(cursor - nursery->base) >> 4;
        kiln_string *header;
        char *bytes;
        nursery->bitmap[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
        block->kind = KILN_GC_STRING | KILN_GC_FLAG_NURSERY;
        block->mark = 0;
        block->size = payload;
        block->prev = NULL;
        block->next = NULL;
        header = (kiln_string *)(block + 1);
        bytes = (char *)(header + 1);
        if (len > UINT32_MAX) return 0;
        header->unit_len = (uint32_t)len;
        header->flags = KILN_STRING_LATIN1;
        header->data = bytes;
        bytes[len] = '\0';
        out[index] = bytes;
        counted += payload;
        cursor += slot;
    }
    nursery->bump = cursor;
    nursery->count += count;
    nursery->bytes += counted;
    heap->count += count;
    heap->bytes += counted;
    heap->since_collection += counted;
    return count;
}

