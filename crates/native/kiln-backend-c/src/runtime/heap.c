
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
 * scanning both the spilled callee-saved registers (`setjmp`) and every word from there up to
 * the current thread's stack base, marking anything that resolves to a live block or interior
 * pointer and leaving everything else alone.
 *
 * The heap struct's first member is the hooks pointer. The runtime reads exactly that one word
 * through the otherwise opaque heap pointer, so a root function outside the prelude (the graph
 * launcher) can mark through `kiln_realm_gc_mark_value`.
 * ------------------------------------------------------------------------------------------- */

#define KILN_GC_PHASE_MARK 0
#define KILN_GC_PHASE_TEARDOWN 1
#define KILN_GC_PHASE_RESET 2
#define KILN_GC_PHASE_LANE_RESET 3
#define KILN_GC_ROOT_SURVIVES_TEARDOWN 1u
#define KILN_GC_HOOKS_VERSION 2u
#define KILN_GC_DEFAULT_FLOOR ((size_t)256 * 1024)
#define KILN_GC_MID_TURN_FLOOR ((size_t)4 * 1024 * 1024)
/* A moved block can be received and sent back inside one callback. Defer its first mid-turn
 * scan unless several large stores accumulate; the turn-boundary pass still sees net growth. */
#define KILN_GC_MID_TURN_MOVED_FLOOR ((size_t)64 * 1024 * 1024)
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
/* A long-running `isolate` factory must not suppress collection without a memory ceiling.
 * Above this much filled nursery storage, collect normally and invalidate move provenance. */
#define KILN_ACTOR_CANDIDATE_GC_DEFER_BYTES ((size_t)64 * 1024 * 1024)
/* What the `introspect` hook reports back, as a bitmask. `ARMED` is what tells the drive to keep
 * its park short enough to notice a signal; `DUMPED` says a request was pending and was served. */
#define KILN_C99_INTROSPECT_ARMED 1
#define KILN_C99_INTROSPECT_DUMPED 2

/* `kind` carries `kiln_gc_kind` in its low bits and, for a `KILN_GC_STRING` block only, a cached
 * answer to "is this string all ASCII" (ADR 0022: ASCII is exactly the case where a code-unit
 * index equals a byte offset, which is what every position-taking string operation walks UTF-8 to
 * find) in its top two. `KILN_GC_KIND_COUNT` is nowhere near the mask's 26 low bits, so the two
 * high bits are free; every reader of `block->kind` as a `kiln_gc_kind` must mask with
 * `KILN_GC_KIND_MASK` first. */
#define KILN_GC_KIND_MASK ((uint32_t)0x03FFFFFFu)
#define KILN_GC_FLAG_ASCII_KNOWN ((uint32_t)0x80000000u)
#define KILN_GC_FLAG_ASCII ((uint32_t)0x40000000u)
#define KILN_GC_FLAG_NURSERY ((uint32_t)0x20000000u)
#define KILN_GC_FLAG_DEAD ((uint32_t)0x10000000u)
/* A block malloc'd on its own, outside every slab: with `KILN_GC_FLAG_NURSERY` this lets a header
 * alone say where its block lives, which is what `kiln_c99_gc_release_cell` needs. */
#define KILN_GC_FLAG_LARGE ((uint32_t)0x08000000u)
/* Only candidate-owned standalone blocks use the cold owner map. Ordinary headers keep their
 * existing size; the flag is cleared before ownership leaves this heap. */
#define KILN_GC_FLAG_CANDIDATE_LARGE ((uint32_t)0x04000000u)

/* The kinds `kiln_c99_gc_finalize` has work for -- exactly its non-empty cases. A sweep asks this
 * before the call, so a dead cell, settler or string costs no call and no read of its payload.
 * A kind given a finaliser case below must be added here in the same change; one left out would
 * leak what its finaliser frees, never crash. */
#define KILN_GC_FINALIZED_KINDS                                                                    \
    ((UINT64_C(1) << KILN_GC_OBJECT) | (UINT64_C(1) << KILN_GC_CLOSURE) |                          \
     (UINT64_C(1) << KILN_GC_ARRAY) | (UINT64_C(1) << KILN_GC_GENERATOR) |                         \
     (UINT64_C(1) << KILN_GC_PROMISE) | (UINT64_C(1) << KILN_GC_BOUND) |                           \
     (UINT64_C(1) << KILN_GC_COLLECTION) | (UINT64_C(1) << KILN_GC_TYPED_BUFFER) |                 \
     (UINT64_C(1) << KILN_GC_REGEXP) | (UINT64_C(1) << KILN_GC_NODE_SERVER) |                      \
     (UINT64_C(1) << KILN_GC_NODE_RESPONSE) | (UINT64_C(1) << KILN_GC_NODE_CLIENT_REQUEST) |       \
     (UINT64_C(1) << KILN_GC_NEXT_TICK) | (UINT64_C(1) << KILN_GC_TIMER) |                        \
     (UINT64_C(1) << KILN_GC_SHARED_FACADE))

static inline int kiln_c99_gc_kind_finalized(uint32_t kind) {
    uint32_t masked = kind & KILN_GC_KIND_MASK;
    return masked < 64 && ((KILN_GC_FINALIZED_KINDS >> masked) & 1u) != 0;
}

/* The finalised kinds whose finaliser has work from the moment they are made. The other three --
 * a closure, a promise, a generator -- own nothing outside their block until a rarer path gives
 * them something (more than four captures, a second reaction, an async generator's request
 * queue), and that path calls `kiln_c99_gc_note_finalizable`. Until then their finaliser is a
 * no-op by construction, which is what lets a nursery collection skip a dead one unread. */
#define KILN_GC_FINALIZED_AT_ALLOC                                                                 \
    (KILN_GC_FINALIZED_KINDS & ~((UINT64_C(1) << KILN_GC_CLOSURE) | (UINT64_C(1) << KILN_GC_PROMISE) | \
                                 (UINT64_C(1) << KILN_GC_GENERATOR)))

static inline int kiln_c99_gc_kind_finalized_at_alloc(uint32_t kind) {
    uint32_t masked = kind & KILN_GC_KIND_MASK;
    return masked < 64 && ((KILN_GC_FINALIZED_AT_ALLOC >> masked) & 1u) != 0;
}

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
    uint32_t free_count;            /* slots on the free list plus slots never handed out */
    uint32_t first_slot_offset;     /* bytes from the slab base to slot 0 */
    /* Slots `[0, fresh)` have been handed out at least once and carry a header; the rest have
     * never been touched (ADR 0070: a realm must cost what it uses). A slab used to write every
     * slot's header when it was made, so the first key copy of a size class cost the realm the
     * whole 64 KiB slab in resident memory; now it costs the pages its slots reach. Every walk
     * and every lookup stops at `fresh`, and nothing reads a slot past it. */
    uint32_t fresh;
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

/* A nursery chunk keeps three bitmaps over its 16-byte granules, one bit per block start:
 *
 *   bitmap  the block is allocated and has not died (lookups trust nothing else);
 *   marks   the block was marked by the collection in progress (cleared by `kiln_c99_gc_begin`);
 *   finals  the block's finaliser has work to do.
 *
 * With them a collection settles a chunk without reading a single block header: survivors are
 * `bitmap & marks`, the dead are the rest, and only a dead block with its `finals` bit set is
 * visited, to run its finaliser. A header read per block -- the living one to see its mark, the
 * dead one to decide it needed nothing -- was a cache miss per block allocated and per block
 * that survived, and at a hundred thousand pending tasks the walk was a sixth of the run.
 *
 * `alloc_count`/`alloc_bytes` are the blocks this chunk holds (the current chunk's are in the
 * nursery until it is sealed); `marked_count`/`marked_bytes` are what the marking found in it.
 * Their difference is what a collection frees, without the sizes being read back.
 *
 * Marks are cleared lazily, by a chunk's first mark in a collection (`mark_epoch`), and a chunk
 * whose every block is marked settles without a pass over its words. Clearing and scanning every
 * chunk at every collection is a pass over all of them each time -- quadratic under
 * `KILN_GC_STRESS`, which collects at every allocation and promotes a chunk each time. */
typedef struct kiln_c99_candidate_group kiln_c99_candidate_group;
typedef struct kiln_c99_candidate_large kiln_c99_candidate_large;

typedef struct kiln_nursery_chunk {
    char *base;
    char *limit;
    uint64_t *bitmap;
    uint64_t *marks;
    uint64_t *finals;
    size_t bitmap_words;
    size_t live_count;
    size_t alloc_count;
    size_t alloc_bytes;
    size_t marked_count;
    size_t marked_bytes;
    /* The collection (`heap->epoch`) whose marks `marks` and the marked counts hold. A chunk's
     * first mark in a collection clears what an earlier one left; a chunk nothing marked has
     * stale marks, and a stale epoch is read as none marked. */
    uint32_t mark_epoch;
    /* Nonzero only for a graph candidate built in its own arena. A move may use it only in the
     * collection epoch in which the chunk entered the sender heap. This records allocation
     * provenance, not a graph or incoming-root proof. */
    uint32_t candidate_epoch;
    uint64_t candidate_id;
    /* Provisional between CANDIDATE_END and ISOLATE, sealed after the latter proves closure.
     * Both states keep the original bitmap/layout while a live root still names this chunk. */
    uint8_t candidate_state;
    /* Only the full-chunk list is doubly linked. Its membership bit and predecessor make
     * batch handoff validation/unlink independent of the sender's unrelated chunk count. */
    uint8_t in_full;
    struct kiln_nursery_chunk *full_prev;
    struct kiln_nursery_chunk *next;
    /* Candidate membership is separate from the collector's lists: a builder may still be
     * filling a chunk, and collection or handoff may remove an interior group member. */
    kiln_c99_candidate_group *candidate_group;
    struct kiln_nursery_chunk *candidate_prev;
    struct kiln_nursery_chunk *candidate_next;
} kiln_nursery_chunk;

struct kiln_c99_candidate_group {
    uint64_t id;
    kiln_nursery_chunk *head;
    size_t count;
    kiln_c99_candidate_large *large_head;
    size_t large_count;
    /* Direct-factory owner records are weak and live through later writes, not just the
     * factory scope. Arena snapshots never expose pointers to their sender and need no ledger. */
    struct kiln_gc_heap *ledger_heap;
    kiln_c99_candidate_group *ledger_previous;
    kiln_c99_candidate_group *ledger_next;
    kiln_gc_ptrset incoming_owners;
    int incoming_failed;
    /* A synchronous factory or private arena pins metadata while its membership can be empty.
     * Once that builder closes, only member chunks keep the manifest alive. */
    int builder;
};

/* This metadata belongs only to factory allocations that take the standalone path. It is
 * ownership bookkeeping, never a GC root. Collector reclamation removes its reverse entry. */
struct kiln_c99_candidate_large {
    kiln_gc_block *block;
    kiln_c99_candidate_group *group;
    kiln_c99_candidate_large *prev;
    kiln_c99_candidate_large *next;
    uint64_t epoch;
};

typedef struct kiln_c99_candidate_large_map {
    kiln_c99_candidate_large **table;
    size_t capacity;
    size_t count;
    size_t used;
} kiln_c99_candidate_large_map;

typedef struct kiln_gc_nursery {
    char *base;
    char *bump;
    char *limit;
    size_t capacity;
    uint64_t *bitmap;
    uint64_t *finals;
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
    size_t held_bytes;
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

/* Built-in numbers the per-realm modified-built-in bitmap covers, in 64-bit words. */
#define KILN_C99_MODIFIED_BUILTIN_WORDS 2

/* How many candidate edges the marker holds, each with its header prefetched, before it looks the
 * oldest up. A power of two. */
#define KILN_GC_PENDING_EDGES 16u

typedef struct kiln_c99_graph_side_span {
    uintptr_t base;
    uintptr_t limit;
    const void *owner;
} kiln_c99_graph_side_span;

typedef struct kiln_c99_graph_side_set {
    kiln_c99_graph_side_span *spans;
    size_t count;
    size_t capacity;
    uintptr_t lo;
    uintptr_t hi;
    /* The outgoing certificate already validates these regions. Retain its index for the
     * incoming walk, bound to the exact borrowed manifest so unrelated diagnostic batches
     * cannot accidentally use another candidate's membership answer. */
    kiln_gc_ptrset regions;
    kiln_nursery_chunk *const *region_source;
    kiln_gc_ptrset large;
    kiln_gc_block **large_blocks;
    size_t large_count;
    int foreign_dictionary_keys;
} kiln_c99_graph_side_set;

static void kiln_c99_graph_side_set_dispose(kiln_c99_graph_side_set *set) {
    free(set->spans);
    free(set->regions.table);
    free(set->large.table);
    free(set->large_blocks);
    memset(set, 0, sizeof *set);
}

/* A native resource owned by a JavaScript object without making that object a GC root. The
 * collector releases it after marking proves the handle dead, before sweeping its block. */
typedef struct kiln_gc_weak_resource {
    struct kiln_gc_weak_resource *next;
    kiln_value handle;
    void *payload;
    void (*dispose)(void *);
    void (*mark)(struct kiln_gc_heap *, void *);
    int (*process)(struct kiln_gc_heap *, void *);
    int (*aliases)(const struct kiln_gc_heap *, kiln_nursery_chunk *const *, size_t, void *);
} kiln_gc_weak_resource;

typedef struct kiln_gc_heap {
    const kiln_gc_hooks *hooks;
    kiln_gc_block *head; /* large blocks only; slab blocks are found by walking slabs */
    size_t count;
    size_t bytes;
    size_t since_collection;
    size_t survived;
    /* Moved ArrayBuffer stores are allocated elsewhere. Track the bytes this realm currently
     * owns, and collect when that total grows beyond the last collection's live baseline.
     * Moving one store back and forth must not charge a fresh 16 MB on every adoption. */
    size_t moved_external_current;
    size_t moved_external_baseline;
    size_t floor;
    uint32_t epoch;
    int enabled;
    int stress;
    int trace;
    int collecting;
    /* Optional two-turn retained-heap sample for the first few realms. */
    uint8_t profile_pass;
    uint8_t profile_selected;
    /* Nonzero only while one synchronous actor `isolate` factory owns fresh nursery chunks.
     * The factory's allocations are still ordinary heap objects, but no unrelated allocation
     * can enter a chunk tagged with this ID. Nested factories use the outer scope and cannot
     * claim an independent movable candidate. */
    uint64_t actor_candidate_id;
    uint64_t actor_candidate_pending_id;
    int actor_candidate_collected;
    kiln_c99_candidate_group *actor_candidate_group;
    kiln_c99_candidate_large_map *candidate_large;
    size_t candidate_ledger_groups;
    kiln_c99_candidate_group *candidate_ledger_head;
    int candidate_ledger_publication_failed;
    /* Opaque callable contexts need a live root authority even when made before a factory.
     * Weak block witnesses never keep the callable or its native anchor alive. */
    kiln_gc_ptrset candidate_context_owners;
    int candidate_context_inventory_failed;
    /* Temporary realm-thread observer reuses the audited incoming field inventory. */
    void (*graph_pointer_observer)(void *, const void *);
    void *graph_pointer_observer_context;
    /* The exclusive incoming proof owns this temporary side-span list. It is never a GC root. */
    const kiln_c99_graph_side_set *graph_proof_sides;
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
    /* Candidate edges waiting to be looked up, oldest at `pending_head` (see
     * `kiln_c99_gc_mark_block`). */
    const void *pending[KILN_GC_PENDING_EDGES];
    uint32_t pending_head;
    uint32_t pending_len;
    size_t marked_blocks;
    uint64_t collections;
    uint64_t freed_blocks;
    uint64_t freed_bytes;
    /* Weak identities for graph nodes observed in this realm. Links are deliberately absent
     * from the GC tracer; a facade leaves the chain in its finalizer. */
    kiln_shared_facade *shared_facades;
    kiln_shared_facade **shared_facade_buckets;
    size_t shared_facade_capacity;
    size_t shared_facade_count;
    kiln_collection *weak_collections;
    kiln_gc_weak_resource *weak_resources;
    kiln_value *weak_keep_during_job;
    size_t weak_keep_count;
    size_t weak_keep_capacity;
    uint32_t weak_job_depth;
    kiln_key_table keys;
    /* Canonical pointers for the keys the HTTP path writes on every request and response. See
     * KILN_C99_HOT_KEYS. NULL means "not interned in this heap yet"; the table is per-heap
     * because a canonical pointer from one realm means nothing in another. */
    const kiln_string *hot_keys[KILN_C99_HOT_KEY_COUNT];
    /* ADR 0071. A process-unique number for this heap, never reused, for a cache that must know
     * which heap it was built under when a freed heap's address can come back as another's. */
    uint64_t serial;
    /* Every realm-local shape this heap made, freed with it, and how many (bounded per realm by
     * `kiln_c99_shape_edge_room`); and the side table of realm-local edges out of shared shapes
     * (see `kiln_c99_local_edge`). */
    kiln_shape *local_shapes;
    uint32_t local_shape_count;
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
    int mid_turn;
    /* ADR 0069's paused state: nonzero while a parallel call's worker arenas are open (between
     * `kiln_c99_par_arenas_open` and `kiln_c99_par_arenas_join`). The arenas' chunks are no part
     * of the heap yet and objects in them already point into it, so a collection then would sweep
     * what they reach unmarked; `kiln_c99_gc_collect` stops at the cause instead. */
    int parallel_paused;
    size_t stress_every;
    size_t stress_countdown;
    /* Call-site caches with an entry that names one of this heap's realm-local shapes (ADR
     * 0071): registered when they learn one, and made to forget every realm-local shape when the
     * heap is torn down -- before the shapes are freed -- so no entry can match a later shape at a
     * freed one's address. Nothing in an entry is a heap object, so a collection needs nothing
     * from this list. */
    kiln_c99_site **sites;
    size_t site_len;
    size_t site_cap;
    /* The same for private-field sites (`kiln_c99_private_site`), whose one entry may name a
     * realm-local shape: an object with a `Symbol()` key and private fields. */
    kiln_c99_private_site **private_sites;
    size_t private_site_len;
    size_t private_site_cap;
    /* ADR 0071: the built-ins this realm has written an own property onto, one bit per built-in
     * number (`kiln_c99_realm_builtin_modified`), and one flag for any number past the bitmap. */
    uint64_t modified_builtins[KILN_C99_MODIFIED_BUILTIN_WORDS];
    /* How much of `kiln_c99_builtin_store_epoch` this realm added and has not yet taken back
     * (`kiln_c99_builtin_bootstrap_done`). */
    uint64_t builtin_stores;
    /* Bootstrap-only direct construction of sealed callable stores stops before program code
     * runs. Zero-initialized with the heap and set by `kiln_c99_builtin_bootstrap_done`. */
    uint8_t bootstrap_complete;
    /* A transferred dictionary may own key bytes that were never interned in this realm.
     * Unknown-name reads must still walk it by spelling, including through prototypes. */
    uint8_t foreign_dictionary_keys;
    /* This realm's Object.prototype own-property store once it exists (`kiln_c99_callable_store`),
     * kept alive by the realm's registry; read live by `kiln_c99_object_prototype_lacks`. */
    const struct kiln_object *object_prototype_store;
    uint8_t modified_builtin_beyond;
    /* The same record restricted to what the realm stored after its bootstrap finished
     * (`kiln_c99_builtin_bootstrap_done` clears it): which built-ins the *program* has modified.
     * The emitted speculations ask it (`kiln_c99_builtins_unmodified_mask`,
     * `kiln_c99_builtin_member_unmodified`); the bootstrap's own definitions are part of what the
     * built-ins are and must not switch those off. */
    uint8_t program_builtin_beyond;
    uint64_t program_builtins[KILN_C99_MODIFIED_BUILTIN_WORDS];
    /* Moves on every store onto a built-in's own-property store (the one choke point,
     * `kiln_c99_callable_store` with `create`), so an answer computed from those stores is
     * current while this is unchanged. */
    uint64_t builtin_store_serial;
    /* `kiln_c99_realm_builtin_member_untouched`'s answers, each valid while `serial` is the
     * heap's `builtin_store_serial`: a speculation on `Math.sqrt` in a realm that wrote
     * `Math.foo` asks the store once per store, not once per call. Sixty-four of them: a realm
     * that wrote onto Array.prototype asks one per method name its call sites use (issue 30). */
    struct {
        uint64_t serial;
        const char *member;
        uint32_t builtin;
        uint8_t untouched;
        uint8_t cached_member_len;
        char cached_member[64];
    } member_answers[64];
    /* `kiln_c99_realm_tag_method_pristine`'s answers, by method code and receiver kind, valid
     * while `serial` is the heap's `builtin_store_serial` (parallel issue 30): a cached method
     * asks its name's stores once per store onto a built-in, not once per call. */
    struct {
        uint64_t serial;
        uint64_t code;
        uint8_t tag;
        uint8_t pristine;
    } method_answers[64];
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
    /* Rejections waiting for the end of this checkpoint and reported rejections handled since:
     * what the promise lane hands node's tracking once the queues are empty (promises.c). The
     * lists themselves are realm slots, which the runtime already roots. */
    size_t rejections_waiting;
    /* How this realm uses the shared immutable region (`KILN_C99_REGION_MODE_*`), decided at its
     * first region site (`kiln_c99_region_mode`) and fixed from then on. */
    uint8_t region_mode;
    /* The realm context (ADR 0070, realms issue 04 step 5; `foundation.c`): the runtime's realm
     * state and every unit's module block, owned, marked and freed by this heap. */
    kiln_c99_realm_state realm;
} kiln_gc_heap;

/* Defined with the private arena builder in spread.c, after the runtime heap primitives. */
static int kiln_c99_prepared_graph_has(kiln_gc_heap *heap, kiln_value handle);
static int kiln_c99_prepared_graph_create(kiln_gc_heap *heap, kiln_value handle,
                                           kiln_value source, size_t budget);
static int kiln_c99_clone_plain_graph_for_message(kiln_value source);

static const kiln_gc_hooks kiln_c99_gc_hooks;
/* The heap a unit uses when no realm is active: the prelude's own tests, and a launcher turning a
 * realm's failure into text after the realm has left its thread. It never collects. One per
 * thread: realms run on several threads at once, and a single process-wide fallback was made and
 * used by each of them unsynchronized (realms issue 24: one thread zeroed it in `heap_init` while
 * another laid out its first nursery chunk, and read a chunk size of zero). */
static _Thread_local kiln_gc_heap kiln_c99_gc_fallback;

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
 * makes it reachable, and is never changed afterwards. A shape is never freed; a key table that a
 * bigger one replaced is freed by quiescent-state reclamation (`kiln_realm_retire`). */
#define KILN_C99_PROCESS_LAYOUT (UINT64_C(0x6b696c6e00010000) | (uint64_t)sizeof(struct kiln_shared_state))

/* The shared key table: open addressing, never shrunk, never deleted from. A grown table is a new
 * allocation published whole; the one it replaces is retired (`kiln_realm_retire`) and freed once
 * no reader can still be probing it. */
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
    /* The last layout token issued to a list-mode object an inline cache relies on (see
     * `kiln_object.layout_token`); tokens are never reused. */
    uint64_t next_layout_token;
    /* The last id handed to a compiled unit (`kiln_c99_unit`): the unit's row in every realm's
     * table of module blocks. Never reused. */
    uint32_t next_unit_id;
    kiln_shared_key_table *keys;
    /* The empty key, headed like every other so `kiln_c99_key_len` reads zero from it. One for the
     * process rather than one per unit, so "" is one key across units too. */
    kiln_string empty_key;
    uint8_t empty_bytes[8];
    /* How many shared shapes the process has minted, under `lock`; `kiln_c99_shape_edge_room`
     * bounds it. Shared shapes are never freed. A realm's own shapes are counted on its heap
     * (`local_shape_count`), so realms side by side do not use up each other's room. */
    uint32_t shared_shapes;
    /* The root of the shape tree. */
    kiln_shape root;
    /* First active final layout: keyed by callable kind and code, never by a realm object's
     * address. More stable built-in stores can join this table after their consumers are audited. */
    struct {
        uint32_t tag;
        uint64_t code;
        kiln_shape *shape;
    } final_builtin_layout;
    /* Complete wide built-in descriptors. A node is inserted only after its entire sequence
     * has been copied and validated; readers take the head with acquire and never see a prefix.
     * Values and prototype links stay on each realm's objects. */
    kiln_sealed_layout *sealed_layouts;
    /* The shared immutable region (ADR 0070 primitive B; see "The shared immutable region"
     * below). `region_state` is one of `KILN_C99_REGION_*`; the rest is written by the one realm
     * that builds the region, before the release store that seals it. */
    uint32_t region_state;
    uint64_t region_builder;
    char *region_base;
    size_t region_reserve;
    size_t region_used;
    size_t region_blocks;
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
static kiln_nursery_chunk *kiln_c99_chunk_lay_out(char *region, size_t bitmap_words);

static kiln_nursery_chunk *kiln_c99_nursery_new_region(kiln_gc_nursery *nursery) {
    char *region = (char *)kiln_c99_gc_aligned_alloc(nursery->capacity, nursery->capacity);
    kiln_nursery_chunk *chunk;
    if (region == NULL) return NULL;
    if (kiln_c99_gc_ptrset_insert(&nursery->regions, region) != KILN_C99_OK) {
        kiln_c99_gc_aligned_free(region);
        return NULL;
    }
    chunk = kiln_c99_chunk_lay_out(region, nursery->bitmap_words);
    if (nursery->lo == NULL || region < nursery->lo) nursery->lo = region;
    if (nursery->hi == NULL || region + nursery->capacity > nursery->hi) {
        nursery->hi = region + nursery->capacity;
    }
    return chunk;
}

/* A region's header and its three bitmaps, laid out at its start, the payload after them, and
 * nothing allocated: what every chunk of the nursery's kind looks like when it is made, whichever
 * thread makes it (a worker arena's are made on pool threads, `kiln_c99_arena_chunk`). */
static kiln_nursery_chunk *kiln_c99_chunk_lay_out(char *region, size_t bitmap_words) {
    kiln_nursery_chunk *chunk;
    size_t bitmap_bytes;
    chunk = (kiln_nursery_chunk *)region;
    chunk->bitmap_words = bitmap_words;
    bitmap_bytes = (chunk->bitmap_words * sizeof(uint64_t) + 15) & ~(size_t)15;
    chunk->bitmap = (uint64_t *)(region + KILN_NURSERY_HEADER);
    chunk->marks = (uint64_t *)(region + KILN_NURSERY_HEADER + bitmap_bytes);
    chunk->finals = (uint64_t *)(region + KILN_NURSERY_HEADER + 2 * bitmap_bytes);
    memset(chunk->bitmap, 0, 3 * bitmap_bytes);
    chunk->base = region + KILN_NURSERY_HEADER + 3 * bitmap_bytes;
    chunk->limit = chunk->base;
    chunk->live_count = 0;
    chunk->alloc_count = 0;
    chunk->alloc_bytes = 0;
    chunk->marked_count = 0;
    chunk->marked_bytes = 0;
    chunk->mark_epoch = 0;
    chunk->candidate_epoch = 0;
    chunk->candidate_id = 0;
    chunk->candidate_state = 0;
    chunk->in_full = 0;
    chunk->full_prev = NULL;
    chunk->next = NULL;
    chunk->candidate_group = NULL;
    chunk->candidate_prev = NULL;
    chunk->candidate_next = NULL;
    return chunk;
}

static kiln_c99_candidate_group *kiln_c99_candidate_group_new(uint64_t id) {
    kiln_c99_candidate_group *group;
    if (id == 0) return NULL;
    group = (kiln_c99_candidate_group *)calloc(1, sizeof *group);
    if (group != NULL) {
        group->id = id;
        group->builder = 1;
    }
    return group;
}

static void kiln_c99_candidate_group_dispose(kiln_c99_candidate_group *group) {
    if (group->builder || group->count != 0 || group->large_count != 0) abort();
    if (group->ledger_heap != NULL) {
        kiln_gc_heap *heap = group->ledger_heap;
        if (heap->candidate_ledger_groups == 0) abort();
        if (group->ledger_previous != NULL)
            group->ledger_previous->ledger_next = group->ledger_next;
        else {
            if (heap->candidate_ledger_head != group) abort();
            heap->candidate_ledger_head = group->ledger_next;
        }
        if (group->ledger_next != NULL)
            group->ledger_next->ledger_previous = group->ledger_previous;
        --heap->candidate_ledger_groups;
        if (heap->candidate_ledger_groups == 0) heap->candidate_ledger_publication_failed = 0;
    }
    free(group->incoming_owners.table);
    free(group);
}

static void kiln_c99_candidate_group_close(kiln_c99_candidate_group *group) {
    if (group == NULL) return;
    if (!group->builder) abort();
    group->builder = 0;
    if (group->count == 0 && group->large_count == 0) kiln_c99_candidate_group_dispose(group);
}

static void kiln_c99_candidate_group_add(kiln_c99_candidate_group *group,
                                          kiln_nursery_chunk *chunk) {
    if (group == NULL || chunk == NULL || chunk->candidate_group != NULL ||
        group->count == SIZE_MAX) abort();
    chunk->candidate_group = group;
    chunk->candidate_id = group->id;
    chunk->candidate_prev = NULL;
    chunk->candidate_next = group->head;
    if (group->head != NULL) group->head->candidate_prev = chunk;
    group->head = chunk;
    ++group->count;
}

static int kiln_c99_candidate_group_contains(const kiln_c99_candidate_group *group,
                                               const kiln_nursery_chunk *chunk) {
    if (group == NULL || chunk == NULL || chunk->candidate_group != group ||
        chunk->candidate_id != group->id) return 0;
    return chunk->candidate_prev == NULL ? group->head == chunk
        : chunk->candidate_prev->candidate_group == group &&
          chunk->candidate_prev->candidate_next == chunk;
}

static void kiln_c99_candidate_group_remove(kiln_nursery_chunk *chunk) {
    kiln_c99_candidate_group *group = chunk->candidate_group;
    if (group == NULL) return;
    if (!kiln_c99_candidate_group_contains(group, chunk) || group->count == 0) abort();
    if (chunk->candidate_prev == NULL) group->head = chunk->candidate_next;
    else chunk->candidate_prev->candidate_next = chunk->candidate_next;
    if (chunk->candidate_next != NULL)
        chunk->candidate_next->candidate_prev = chunk->candidate_prev;
    chunk->candidate_group = NULL;
    chunk->candidate_prev = NULL;
    chunk->candidate_next = NULL;
    if (--group->count == 0 && group->large_count == 0 && !group->builder)
        kiln_c99_candidate_group_dispose(group);
}

static void kiln_c99_candidate_chunk_clear(kiln_nursery_chunk *chunk) {
    kiln_c99_candidate_group_remove(chunk);
    chunk->candidate_id = 0;
    chunk->candidate_epoch = 0;
    chunk->candidate_state = 0;
}

static void kiln_c99_nursery_free_region(kiln_gc_nursery *nursery, kiln_nursery_chunk *chunk) {
    kiln_c99_candidate_group_remove(chunk);
    kiln_c99_gc_ptrset_remove(&nursery->regions, chunk);
    kiln_c99_gc_aligned_free(chunk);
}

static int kiln_c99_nursery_full_contains(const kiln_gc_nursery *nursery,
                                           const kiln_nursery_chunk *chunk) {
    if (chunk == NULL || !kiln_c99_gc_ptrset_contains(&nursery->regions, chunk) ||
        !chunk->in_full) return 0;
    if (chunk->full_prev == NULL) return nursery->full == chunk;
    return kiln_c99_gc_ptrset_contains(&nursery->regions, chunk->full_prev) &&
           chunk->full_prev->in_full && chunk->full_prev->next == chunk;
}

static void kiln_c99_nursery_full_push(kiln_gc_nursery *nursery, kiln_nursery_chunk *chunk) {
    if (chunk->in_full) abort();
    chunk->full_prev = NULL;
    chunk->next = nursery->full;
    if (chunk->next != NULL) chunk->next->full_prev = chunk;
    chunk->in_full = 1;
    nursery->full = chunk;
}

static void kiln_c99_nursery_full_remove(kiln_gc_nursery *nursery, kiln_nursery_chunk *chunk) {
    if (!kiln_c99_nursery_full_contains(nursery, chunk)) abort();
    if (chunk->full_prev == NULL) nursery->full = chunk->next;
    else chunk->full_prev->next = chunk->next;
    if (chunk->next != NULL) chunk->next->full_prev = chunk->full_prev;
    chunk->full_prev = NULL;
    chunk->next = NULL;
    chunk->in_full = 0;
}

/* Return the ordinary singly linked storage list when a temporary indexing heap relinquishes
 * an arena. The region table still indexes these chunks until the caller discards that table. */
static kiln_nursery_chunk *kiln_c99_nursery_full_take_all(kiln_gc_nursery *nursery) {
    kiln_nursery_chunk *list = nursery->full;
    for (kiln_nursery_chunk *chunk = list; chunk != NULL; chunk = chunk->next) {
        chunk->in_full = 0;
        chunk->full_prev = NULL;
    }
    nursery->full = NULL;
    nursery->full_bytes = 0;
    nursery->held_bytes = 0;
    return list;
}

/* Metadata is not a GC root. Collection updates membership before returning storage, so this
 * snapshot contains exactly the group's remaining sealed chunks, never unrelated heap chunks. */
/* A tombstone cannot be a malloc allocation. These tables are cold and heap-local; no
 * callback or collector treats the owner pointers as reachability. */
#define KILN_C99_CANDIDATE_LARGE_TOMBSTONE ((kiln_c99_candidate_large *)(uintptr_t)1)

static size_t kiln_c99_candidate_large_hash(const kiln_gc_block *block, size_t capacity) {
    uint64_t hash = (uint64_t)(uintptr_t)block >> 4;
    hash ^= hash >> 33;
    hash *= UINT64_C(0xff51afd7ed558ccd);
    hash ^= hash >> 33;
    return (size_t)hash & (capacity - 1);
}

static kiln_c99_candidate_large **kiln_c99_candidate_large_slot(
    kiln_c99_candidate_large_map *map, const kiln_gc_block *block) {
    size_t slot = kiln_c99_candidate_large_hash(block, map->capacity);
    kiln_c99_candidate_large **deleted = NULL;
    for (;;) {
        kiln_c99_candidate_large **entry = &map->table[slot];
        if (*entry == NULL) return deleted != NULL ? deleted : entry;
        if (*entry == KILN_C99_CANDIDATE_LARGE_TOMBSTONE) {
            if (deleted == NULL) deleted = entry;
        } else if ((*entry)->block == block) {
            return entry;
        }
        slot = (slot + 1) & (map->capacity - 1);
    }
}

static kiln_c99_candidate_large *kiln_c99_candidate_large_find(
    const kiln_gc_heap *heap, const kiln_gc_block *block) {
    kiln_c99_candidate_large **entry;
    if (heap->candidate_large == NULL || heap->candidate_large->capacity == 0) return NULL;
    entry = kiln_c99_candidate_large_slot(heap->candidate_large, block);
    return *entry == KILN_C99_CANDIDATE_LARGE_TOMBSTONE ? NULL : *entry;
}

static int kiln_c99_candidate_large_grow(kiln_c99_candidate_large_map *map, size_t wanted) {
    kiln_c99_candidate_large_map grown = {0};
    size_t index;
    if (wanted > SIZE_MAX / 2) return 0;
    grown.capacity = map->capacity == 0 ? 16 : map->capacity;
    while (wanted * 2 >= grown.capacity) {
        if (grown.capacity > SIZE_MAX / 2) return 0;
        grown.capacity *= 2;
    }
    if (grown.capacity > SIZE_MAX / sizeof *grown.table) return 0;
    grown.table = (kiln_c99_candidate_large **)calloc(grown.capacity, sizeof *grown.table);
    if (grown.table == NULL) return 0;
    for (index = 0; index < map->capacity; ++index) {
        kiln_c99_candidate_large *entry = map->table[index];
        if (entry == NULL || entry == KILN_C99_CANDIDATE_LARGE_TOMBSTONE) continue;
        *kiln_c99_candidate_large_slot(&grown, entry->block) = entry;
        ++grown.count;
    }
    grown.used = grown.count;
    free(map->table);
    *map = grown;
    return 1;
}

static int kiln_c99_candidate_large_add(kiln_gc_heap *heap, kiln_gc_block *block) {
    kiln_c99_candidate_group *group = heap->actor_candidate_group;
    kiln_c99_candidate_large_map *map;
    kiln_c99_candidate_large *entry;
    kiln_c99_candidate_large **slot;
    if (group == NULL || !group->builder || group->id != heap->actor_candidate_id) abort();
    if (group->large_count == SIZE_MAX) return 0;
    entry = (kiln_c99_candidate_large *)calloc(1, sizeof *entry);
    if (entry == NULL) return 0;
    if (heap->candidate_large == NULL) {
        heap->candidate_large = (kiln_c99_candidate_large_map *)calloc(1,
                                                        sizeof *heap->candidate_large);
        if (heap->candidate_large == NULL) { free(entry); return 0; }
    }
    map = heap->candidate_large;
    if (map->count == SIZE_MAX || map->used > SIZE_MAX / 2 ||
        (map->used * 2 >= map->capacity &&
         !kiln_c99_candidate_large_grow(map, map->count + 1))) {
        free(entry);
        if (map->count == 0) {
            free(map->table); free(map); heap->candidate_large = NULL;
        }
        return 0;
    }
    slot = kiln_c99_candidate_large_slot(map, block);
    if (*slot != NULL && *slot != KILN_C99_CANDIDATE_LARGE_TOMBSTONE) abort();
    if (*slot == NULL) ++map->used;
    entry->block = block;
    entry->group = group;
    entry->epoch = heap->epoch;
    entry->next = group->large_head;
    if (entry->next != NULL) entry->next->prev = entry;
    group->large_head = entry;
    ++group->large_count;
    *slot = entry;
    ++map->count;
    block->kind |= KILN_GC_FLAG_CANDIDATE_LARGE;
    return 1;
}

static void kiln_c99_candidate_large_remove(kiln_gc_heap *heap, kiln_gc_block *block) {
    kiln_c99_candidate_large_map *map;
    kiln_c99_candidate_large **slot;
    kiln_c99_candidate_large *entry;
    kiln_c99_candidate_group *group;
    if ((block->kind & KILN_GC_FLAG_CANDIDATE_LARGE) == 0) return;
    map = heap->candidate_large;
    if (map == NULL) abort();
    slot = kiln_c99_candidate_large_slot(map, block);
    entry = *slot;
    if (entry == NULL || entry == KILN_C99_CANDIDATE_LARGE_TOMBSTONE) abort();
    group = entry->group;
    if (group->large_count == 0 || map->count == 0) abort();
    if (entry->prev != NULL) entry->prev->next = entry->next;
    else group->large_head = entry->next;
    if (entry->next != NULL) entry->next->prev = entry->prev;
    --group->large_count;
    *slot = KILN_C99_CANDIDATE_LARGE_TOMBSTONE;
    --map->count;
    block->kind &= ~KILN_GC_FLAG_CANDIDATE_LARGE;
    free(entry);
    if (group->count == 0 && group->large_count == 0 && !group->builder)
        kiln_c99_candidate_group_dispose(group);
    if (map->count == 0) {
        free(map->table); free(map); heap->candidate_large = NULL;
    }
}

static int kiln_c99_candidate_group_snapshot_owner(const kiln_gc_heap *heap,
                                               const kiln_c99_candidate_group *group,
                                               kiln_nursery_chunk ***out, size_t *out_count) {
    kiln_nursery_chunk **chunks;
    kiln_nursery_chunk *chunk;
    kiln_nursery_chunk *previous = NULL;
    size_t count = 0;
    if (heap == NULL || group == NULL || out == NULL || out_count == NULL ||
        (group->count == 0 && group->large_count == 0) ||
        group->count > SIZE_MAX / sizeof *chunks) return 0;
    chunks = (kiln_nursery_chunk **)malloc((group->count == 0 ? 1 : group->count) * sizeof *chunks);
    if (chunks == NULL) return 0;
    for (chunk = group->head; chunk != NULL; chunk = chunk->candidate_next) {
        if (count == group->count || !kiln_c99_nursery_full_contains(&heap->nursery, chunk) ||
            chunk->candidate_prev != previous ||
            !kiln_c99_candidate_group_contains(group, chunk) ||
            chunk->candidate_epoch != heap->epoch) {
            free(chunks);
            return 0;
        }
        chunks[count++] = chunk;
        previous = chunk;
    }
    if (count != group->count) {
        free(chunks);
        return 0;
    }
    *out = chunks;
    *out_count = count;
    return 1;
}

static int kiln_c99_candidate_group_snapshot(const kiln_gc_heap *heap,
                                               kiln_nursery_chunk *seed,
                                               kiln_nursery_chunk ***out, size_t *out_count) {
    if (heap == NULL || seed == NULL ||
        !kiln_c99_nursery_full_contains(&heap->nursery, seed) ||
        !kiln_c99_candidate_group_contains(seed->candidate_group, seed)) return 0;
    return kiln_c99_candidate_group_snapshot_owner(heap, seed->candidate_group, out, out_count);
}

/* Make `chunk`'s region the one allocation bumps through. */
static void kiln_c99_nursery_use(kiln_gc_nursery *nursery, kiln_nursery_chunk *chunk) {
    if (chunk->candidate_group != NULL) abort();
    nursery->region = (char *)chunk;
    nursery->base = chunk->base;
    nursery->bump = chunk->base;
    nursery->limit = nursery->region + nursery->capacity;
    nursery->bitmap = chunk->bitmap;
    nursery->finals = chunk->finals;
    nursery->count = 0;
    nursery->bytes = 0;
    chunk->candidate_id = 0;
    chunk->candidate_epoch = 0;
    if (chunk->in_full) abort();
    chunk->full_prev = NULL;
    chunk->candidate_state = 0;
}

/* Write the current region's fill into its header, ready for one of the lists. */
static kiln_nursery_chunk *kiln_c99_nursery_seal(kiln_gc_nursery *nursery) {
    kiln_nursery_chunk *chunk = (kiln_nursery_chunk *)nursery->region;
    chunk->limit = nursery->bump;
    chunk->live_count = 0;
    chunk->alloc_count = (size_t)nursery->count;
    chunk->alloc_bytes = (size_t)nursery->bytes;
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
        nursery->finals = NULL;
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
    /* `node:ffi` callback roots take value-slot keys above the realm's singleton slots. */
    heap->realm.ffi_next_root = UINT32_C(0x80000000);
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
    /* What a realm holds of its own outside the collector (ADR 0070: a realm's cost): its
     * realm-local shapes and interned keys. A realm whose startup objects are built from shared
     * keys has none of either before its program makes one (realms issue 08). */
    kiln_c99_introspect_field(&line, "local_shapes", (uint64_t)heap->local_shape_count);
    kiln_c99_introspect_field(&line, "local_keys", (uint64_t)heap->keys.count);
    /* The shared immutable region (realms issue 08): how this realm uses it (0 undecided, 1 not,
     * 2 built it, 3 reads it) and what the process holds there. */
    kiln_c99_introspect_field(&line, "region_mode", (uint64_t)heap->region_mode);
    kiln_c99_introspect_field(&line, "region_blocks", (uint64_t)kiln_c99_process()->region_blocks);
    kiln_c99_introspect_field(&line, "region_bytes", (uint64_t)kiln_c99_process()->region_used);
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

static void kiln_c99_heap_profile_dump(kiln_gc_heap *heap, uint32_t pass);

static int kiln_c99_gc_hook_introspect(void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    int status;
    if (heap == NULL || heap->fallback) return 0;
    if (heap->profile_pass < 3) {
        if (heap->profile_pass == 0) {
            static uint32_t selected;
            const char *setting = getenv("KILN_HEAP_PROFILE");
            heap->profile_selected = setting != NULL && setting[0] == '1' && setting[1] == '\0' &&
                __atomic_fetch_add(&selected, 1, __ATOMIC_RELAXED) < 4;
            if (!heap->profile_selected) heap->profile_pass = 3;
        }
        if (heap->profile_selected) {
            ++heap->profile_pass;
            kiln_c99_heap_profile_dump(heap, heap->profile_pass);
            if (heap->profile_pass == 2) heap->profile_pass = 3;
        }
    }
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
    /* A fallback heap is the answer for a realm that could not attach one; it is this thread's
     * and remembering it is harmless. NULL is not remembered. */
    cache->heap = heap;
    cache->generation = generation;
    return heap;
}

/* The heap `kiln_c99_gc_heap` would answer when its cache answers, else NULL -- without a call,
 * for a register-preserving slow half that must not call anything on its common path (a call
 * there makes it save every register it could clobber, on every call; see
 * `kiln_c99_realm_builtin_member_untouched`). */
static inline KILN_ALWAYS_INLINE kiln_gc_heap *kiln_c99_gc_heap_if_cached(void) {
    const kiln_realm_cache *cache = &kiln_c99_realm_cached;
    if (KILN_LIKELY(cache->generation_slot != NULL && cache->heap != NULL) &&
        KILN_LIKELY(cache->generation == *cache->generation_slot)) {
        return cache->heap;
    }
    return NULL;
}

/* The realm context of the realm this thread is in (see `kiln_c99_realm_state`). Outside any
 * realm -- the prelude's own tests -- it is the fallback heap's, as the statics it replaces were
 * one per process; and when no heap could be made at all, the fallback's too, rather than NULL. */
static kiln_c99_realm_state *kiln_c99_realm(void) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    return KILN_LIKELY(heap != NULL) ? &heap->realm : &kiln_c99_gc_fallback.realm;
}

/* ---- The shared immutable region (ADR 0070 primitive B; realms issue 08) --------------------
 *
 * Memory for objects that never change once published, read by every realm and every thread
 * without copying. The first thing it holds is the closures the runtime bootstrap makes at its
 * top level: every built-in method the bootstrap writes in TypeScript (`Uint8Array.prototype
 * .toHex`, the iterator helpers, `Intl.Segmenter`'s methods, ...). Each realm runs the bootstrap
 * as before -- its objects, prototypes and stores are its own -- but a closure-making site the
 * emitter proved runs once per realm and captures nothing (`EmissionPolicy::shared_region`)
 * hands every realm the one closure the region holds instead of allocating one per realm.
 *
 * Why closures and not the objects. Realms never exchange object references -- a message is a
 * structured clone, a transfer moves an object -- so which realm an identity "belongs to" is not
 * observable; what must be per realm is every *mutable* part (ADR 0059 invariant 4, ADR 0071
 * "modified built-ins are the realm's own"). A closure's mutable parts are two fields that
 * every path already reaches through a handful of accessors, so a region closure keeps them in the
 * realm's context instead (`kiln_c99_closure_prototype`, `kiln_c99_closure_store`, below) --
 * the same arrangement the C built-ins have always had, whose identity is a number and whose
 * store is the realm's. An object's properties, by contrast, are read inline by every cache hit
 * the emitter writes; giving a shared object a per-realm half would put a test on that path for
 * every object. So objects stay the realm's, and the region takes the part of the bootstrap that
 * is behaviour -- about seven tenths of its live heap.
 *
 * Lifecycle. `region_state` goes EMPTY -> BUILDING -> SEALED, or -> ABANDONED. The first realm to
 * reach a region site claims BUILDING and becomes the builder: its sites allocate here, bumping
 * through a reservation made once per process. When its bootstrap finishes
 * (`kiln_c99_builtin_bootstrap_done`) the builder makes the used pages read-only and publishes
 * SEALED with a release store; a realm that finds SEALED reads each site's closure with acquire,
 * so it sees the closure whole. A realm that arrives while another builds waits for it (the
 * builder's bootstrap is one synchronous module body on another thread, so the wait is its
 * remaining run), and gives up after `KILN_C99_REGION_WAIT_NS` and makes its own closures. A
 * builder torn down before sealing abandons the region: nobody uses it, and nothing in it is
 * freed, because the builder's heap may still name what it made. `KILN_REGION=0` keeps a realm out.
 *
 * The collector's contract. Region memory is not any heap's: no heap's tables contain it, so the
 * marker's lookup never finds a region block, never marks one, never traces one and never frees
 * one -- the same way a unit's static string headers are ignored. A realm object may point into
 * the region. A region object never points into a realm heap: its closures capture nothing, their
 * names and source are static text or copied here, and their prototype and store are the realm's.
 * Sealing makes the pages read-only, so a write any path forgot to route is a fault at the write
 * rather than one realm's change appearing in another.
 *
 * The no-write read path (ADR 0069, parallel-builtins issue 06) reads the region from any thread:
 * see "The no-write read path" below. */
#define KILN_C99_REGION_EMPTY 0u
#define KILN_C99_REGION_BUILDING 1u
#define KILN_C99_REGION_SEALED 2u
#define KILN_C99_REGION_ABANDONED 3u
#define KILN_C99_REGION_MODE_UNDECIDED 0
#define KILN_C99_REGION_MODE_NONE 1
#define KILN_C99_REGION_MODE_BUILDER 2
#define KILN_C99_REGION_MODE_USER 3
/* Address space, not memory: pages the builder never reaches are never touched. */
#define KILN_C99_REGION_RESERVE ((size_t)4 << 20)
#define KILN_C99_REGION_WAIT_NS ((uint64_t)250 * 1000 * 1000)

#if defined(__unix__) || defined(__APPLE__)
#include <sys/mman.h>
#include <time.h>
#endif

static size_t kiln_c99_region_page(void) {
#if defined(__unix__) || defined(__APPLE__)
    long page = sysconf(_SC_PAGESIZE);
    if (page > 0 && ((size_t)page & ((size_t)page - 1)) == 0) return (size_t)page;
#endif
    return 4096;
}

/* Whether `pointer` lies in the region. A range test on the process block, for the paths that
 * must treat a region block differently (never on a cache hit). */
static int kiln_c99_in_region(const void *pointer) {
    const kiln_shared_state *process = kiln_c99_process();
    const char *base = __atomic_load_n(&process->region_base, __ATOMIC_ACQUIRE);
    return base != NULL && (const char *)pointer >= base &&
           (const char *)pointer < base + process->region_reserve;
}

/* A block in the region, headed like a heap block so `kiln_c99_gc_payload_kind` reads its kind,
 * zeroed. Only the builder allocates, and only while it builds; NULL when the reservation is used
 * up, and the caller then makes the object in its own heap as before. */
static void *kiln_c99_region_alloc(uint32_t kind, size_t size) {
    kiln_shared_state *process = kiln_c99_process();
    size_t payload = size == 0 ? 1 : size;
    size_t total = (sizeof(kiln_gc_block) + payload + 15) & ~(size_t)15;
    kiln_gc_block *block;
    if (__atomic_load_n(&process->region_state, __ATOMIC_ACQUIRE) != KILN_C99_REGION_BUILDING ||
        process->region_base == NULL || total > process->region_reserve - process->region_used) {
        return NULL;
    }
    block = (kiln_gc_block *)(process->region_base + process->region_used);
    process->region_used += total;
    process->region_blocks += 1;
    block->prev = NULL;
    block->next = NULL;
    block->size = size;
    block->kind = kind & KILN_GC_KIND_MASK;
    block->mark = 0;
    memset(block + 1, 0, payload);
    return block + 1;
}

/* A copy of `string` in the region, for a region closure's name: the builder's own string would
 * die with the builder's heap, which does not know the region closure names it. */
static const kiln_string *kiln_c99_region_string(const kiln_string *string) {
    size_t storage;
    kiln_string *copy;
    uint32_t enc;
    if (string == NULL || kiln_c99_in_region(string)) return string;
    enc = string->flags & (KILN_STRING_LATIN1 | KILN_STRING_UTF16);
    if (!kiln_string_storage_size(string->unit_len, enc, &storage) || storage > SIZE_MAX - 1)
        return NULL;
    copy = (kiln_string *)kiln_c99_region_alloc(KILN_GC_STRING, storage + 1);
    if (copy == NULL) return NULL;
    copy->unit_len = string->unit_len;
    copy->flags = enc;
    copy->data = (const void *)(copy + 1);
    if (storage > sizeof *copy) memcpy((void *)copy->data, string->data, storage - sizeof *copy);
    return copy;
}

/* The reservation: aligned to the page so sealing can protect exactly the used pages. Never
 * freed -- a sealed region lives as long as the process, like the shared keys and shapes. */
static char *kiln_c99_region_reserve(size_t *reserve) {
    size_t page = kiln_c99_region_page();
    size_t size = (KILN_C99_REGION_RESERVE + page - 1) & ~(page - 1);
    char *base = NULL;
#if defined(__unix__) || defined(__APPLE__)
    void *memory = NULL;
    if (posix_memalign(&memory, page, size) == 0) base = (char *)memory;
#else
    base = (char *)kiln_c99_gc_aligned_alloc(page, size);
#endif
    *reserve = base != NULL ? size : 0;
    return base;
}

/* How the current realm uses the region: decided at its first region site and fixed after. */
static int kiln_c99_region_decide(kiln_gc_heap *heap) {
    kiln_shared_state *process = kiln_c99_process();
    const char *setting;
    uint64_t waited = 0;
    if (heap->fallback || heap->serial == 0) return KILN_C99_REGION_MODE_NONE;
    setting = getenv("KILN_REGION");
    if (setting != NULL && setting[0] == '0' && setting[1] == '\0') return KILN_C99_REGION_MODE_NONE;
    for (;;) {
        uint32_t state = __atomic_load_n(&process->region_state, __ATOMIC_ACQUIRE);
        if (state == KILN_C99_REGION_SEALED) return KILN_C99_REGION_MODE_USER;
        if (state == KILN_C99_REGION_ABANDONED) return KILN_C99_REGION_MODE_NONE;
        if (state == KILN_C99_REGION_EMPTY) {
            uint32_t expected = KILN_C99_REGION_EMPTY;
            if (__atomic_compare_exchange_n(&process->region_state, &expected,
                                            KILN_C99_REGION_BUILDING, 0, __ATOMIC_ACQ_REL,
                                            __ATOMIC_ACQUIRE)) {
                size_t reserve = 0;
                char *base = kiln_c99_region_reserve(&reserve);
                if (base == NULL) {
                    __atomic_store_n(&process->region_state, KILN_C99_REGION_ABANDONED,
                                     __ATOMIC_RELEASE);
                    return KILN_C99_REGION_MODE_NONE;
                }
                process->region_reserve = reserve;
                process->region_used = 0;
                process->region_builder = heap->serial;
                __atomic_store_n(&process->region_base, base, __ATOMIC_RELEASE);
                return KILN_C99_REGION_MODE_BUILDER;
            }
            continue;
        }
        /* Another realm is building it, on another thread: a realm runs its bootstrap in one
         * synchronous module body, so no builder waits on this thread. */
        if (waited >= KILN_C99_REGION_WAIT_NS) return KILN_C99_REGION_MODE_NONE;
#if defined(__unix__) || defined(__APPLE__)
        {
            struct timespec pause = {0, 20 * 1000};
            nanosleep(&pause, NULL);
        }
#endif
        waited += 20 * 1000;
    }
}

static int kiln_c99_region_mode(kiln_gc_heap *heap) {
    if (heap == NULL) return KILN_C99_REGION_MODE_NONE;
    if (KILN_UNLIKELY(heap->region_mode == KILN_C99_REGION_MODE_UNDECIDED))
        heap->region_mode = (uint8_t)kiln_c99_region_decide(heap);
    return heap->region_mode;
}

/* The builder's bootstrap has finished: protect what it made and publish it. */
static void kiln_c99_region_seal(kiln_gc_heap *heap) {
    kiln_shared_state *process;
    if (heap == NULL || heap->region_mode != KILN_C99_REGION_MODE_BUILDER) return;
    process = kiln_c99_process();
    heap->region_mode = KILN_C99_REGION_MODE_USER;
    if (__atomic_load_n(&process->region_state, __ATOMIC_ACQUIRE) != KILN_C99_REGION_BUILDING ||
        process->region_builder != heap->serial)
        return;
#if defined(__unix__) || defined(__APPLE__)
    if (process->region_used != 0) {
        size_t page = kiln_c99_region_page();
        size_t length = (process->region_used + page - 1) & ~(page - 1);
        (void)mprotect(process->region_base, length, PROT_READ);
    }
#endif
    /* Nothing more is allocated: the rest of the reservation stays untouched address space. */
    __atomic_store_n(&process->region_state, KILN_C99_REGION_SEALED, __ATOMIC_RELEASE);
}

/* A builder torn down before it sealed: nobody else will use what it started. */
static void kiln_c99_region_abandon(kiln_gc_heap *heap) {
    kiln_shared_state *process;
    uint32_t expected = KILN_C99_REGION_BUILDING;
    if (heap == NULL || heap->region_mode != KILN_C99_REGION_MODE_BUILDER) return;
    process = kiln_c99_process();
    if (process->region_builder != heap->serial) return;
    (void)__atomic_compare_exchange_n(&process->region_state, &expected,
                                      KILN_C99_REGION_ABANDONED, 0, __ATOMIC_ACQ_REL,
                                      __ATOMIC_ACQUIRE);
}

/* The realm's own prototype (`which` 0) or store (`which` 1) of a region closure: a slot that
 * stays where it is. With `create` clear an absent entry answers NULL and nothing is written --
 * the read of a function that was never given a property allocates nothing. NULL too when there
 * is no realm (a parallel worker's thread): a region closure there has no own properties. */
static kiln_value *kiln_c99_region_own_slot(const struct kiln_closure *closure, int which, int create) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_c99_realm_state *realm;
    kiln_c99_region_own *entry;
    size_t bucket;
    if (heap == NULL) return NULL;
    realm = &heap->realm;
    if (realm->region_own_buckets != 0) {
        bucket = (((uintptr_t)closure >> 4) * UINT64_C(0x9e3779b97f4a7c15)) >> 40;
        for (entry = realm->region_own[bucket & (realm->region_own_buckets - 1)]; entry != NULL;
             entry = entry->next) {
            if (entry->closure == closure) return which == 0 ? &entry->prototype : &entry->store;
        }
    }
    if (!create) return NULL;
    if ((realm->region_own_count + 1) * 2 > realm->region_own_buckets) {
        size_t buckets = realm->region_own_buckets == 0 ? 16 : realm->region_own_buckets * 2;
        kiln_c99_region_own **grown = (kiln_c99_region_own **)calloc(buckets, sizeof *grown);
        size_t index;
        if (grown == NULL) return NULL;
        for (index = 0; index < realm->region_own_buckets; ++index) {
            entry = realm->region_own[index];
            while (entry != NULL) {
                kiln_c99_region_own *next = entry->next;
                bucket = (((uintptr_t)entry->closure >> 4) * UINT64_C(0x9e3779b97f4a7c15)) >> 40;
                entry->next = grown[bucket & (buckets - 1)];
                grown[bucket & (buckets - 1)] = entry;
                entry = next;
            }
        }
        free(realm->region_own);
        realm->region_own = grown;
        realm->region_own_buckets = buckets;
    }
    entry = (kiln_c99_region_own *)malloc(sizeof *entry);
    if (entry == NULL) return NULL;
    entry->closure = closure;
    entry->prototype = kiln_c99_undefined();
    entry->store = kiln_c99_undefined();
    bucket = (((uintptr_t)closure >> 4) * UINT64_C(0x9e3779b97f4a7c15)) >> 40;
    entry->next = realm->region_own[bucket & (realm->region_own_buckets - 1)];
    realm->region_own[bucket & (realm->region_own_buckets - 1)] = entry;
    realm->region_own_count += 1;
    return which == 0 ? &entry->prototype : &entry->store;
}

/* A closure's prototype and property store: the closure's own fields, or for a region closure
 * the realm's (above). The reads write nothing; the `_slot` forms make the realm's entry and
 * answer NULL only when memory runs out. */
static inline kiln_value kiln_c99_closure_prototype(const struct kiln_closure *closure) {
    const kiln_value *slot;
    if (KILN_LIKELY(!closure->region)) return closure->own_prototype;
    slot = kiln_c99_region_own_slot(closure, 0, 0);
    return slot != NULL ? *slot : kiln_c99_undefined();
}

static inline kiln_value kiln_c99_closure_store(const struct kiln_closure *closure) {
    const kiln_value *slot;
    if (KILN_LIKELY(!closure->region)) return closure->own_store;
    slot = kiln_c99_region_own_slot(closure, 1, 0);
    return slot != NULL ? *slot : kiln_c99_undefined();
}

static inline kiln_value *kiln_c99_closure_prototype_slot(struct kiln_closure *closure) {
    if (KILN_LIKELY(!closure->region)) return &closure->own_prototype;
    return kiln_c99_region_own_slot(closure, 0, 1);
}

static inline kiln_value *kiln_c99_closure_store_slot(struct kiln_closure *closure) {
    if (KILN_LIKELY(!closure->region)) return &closure->own_store;
    return kiln_c99_region_own_slot(closure, 1, 1);
}

/* ---- The no-write read path (ADR 0069; parallel-builtins issue 06; realms issue 08) --------
 *
 * Reads that write nothing, for a thread that is not the running realm's: a parallel built-in's
 * worker reading a heap frozen for the call (ADR 0069 tier 2), or any thread reading the shared
 * region. It is the ordinary read path -- the same functions, so the two cannot disagree about an
 * answer -- run with this thread in no-write mode, in which each read that would otherwise write
 * does the no-write half of its work instead:
 *
 *   - an inline cache is consulted but never taught (`kiln_c99_site_remember` learns nothing,
 *     so nothing is registered with the heap either);
 *   - a list-mode prototype level with no layout token yet is not issued one
 *     (`kiln_c99_pic_name_level`), so the answer is found but not cached;
 *   - a string's "all ASCII" answer is computed and not stored in its block header
 *     (`kiln_c99_string_is_ascii`); a key's hash is never stored anywhere to begin with;
 *   - a key is looked up and never interned: every interning entry point answers what the shared
 *     and realm tables already hold and NULL otherwise, which its caller treats as it treats
 *     running out of memory -- the parallel section's signal to finish sequentially;
 *   - a region closure's own prototype and store are read from the realm's table without making
 *     an entry (`kiln_c99_region_own_slot` with `create` clear).
 *
 * What the read path cannot do without writing -- allocate, or materialise lazy state such as a
 * function's first `prototype` -- is outside what a tier-2 callback's proof admits, and the realm
 * thread materialises what the proof says it can reach before the pool starts (ADR 0069).
 *
 * Entry points:
 *
 *   kiln_c99_no_write_enter(heap, &frame)   this thread reads `heap` (the frozen realm's, or NULL
 *                                           for the region alone) and writes nothing until
 *   kiln_c99_no_write_leave(&frame)         restores the thread's own realm cache.
 *   kiln_c99_no_write()                     whether this thread is in no-write mode.
 *   kiln_c99_interned_key_in(heap, string)  the lookup that never interns (a key the tables do
 *                                           not hold names no property of any object).
 *   kiln_c99_in_region(pointer)             whether a block is the region's.
 *
 * A thread that reads shared cache state must also be online for the quiescent-state
 * reclamation of issue 04 step 4 (`reclaim::online` in the provider) for the whole section.
 * Region memory is sealed read-only, so a write any of these paths forgot faults at the write. */
static _Thread_local int kiln_c99_no_write_depth;

typedef struct kiln_c99_no_write_frame {
    kiln_realm_cache saved;
} kiln_c99_no_write_frame;

static inline int kiln_c99_no_write(void) {
    return KILN_UNLIKELY(kiln_c99_no_write_depth != 0);
}

static void kiln_c99_no_write_enter(kiln_gc_heap *heap, kiln_c99_no_write_frame *frame) {
    kiln_realm_cache *cache = &kiln_c99_realm_cached;
    frame->saved = *cache;
    if (heap != NULL) {
        if (cache->generation_slot == NULL) cache->generation_slot = kiln_realm_generation_slot();
        cache->heap = heap;
        cache->generation = *cache->generation_slot;
    }
    kiln_c99_no_write_depth += 1;
}

static void kiln_c99_no_write_leave(kiln_c99_no_write_frame *frame) {
    kiln_c99_no_write_depth -= 1;
    kiln_c99_realm_cached = frame->saved;
}

/* A tier-2 chunk worker's section (parallel issue 07): the no-write frame above, kept in the
 * opaque `kiln_c99_par_frame` a module unit can declare, since this struct is the prelude's. */
_Static_assert(sizeof(kiln_c99_no_write_frame) <= sizeof(kiln_c99_par_frame),
               "kiln_c99_par_frame holds a no-write frame");
_Static_assert(_Alignof(kiln_c99_no_write_frame) <= _Alignof(kiln_c99_par_frame),
               "kiln_c99_par_frame is aligned for a no-write frame");

static void kiln_c99_par_enter(const kiln_c99_par_env *env, kiln_c99_par_frame *frame) {
    kiln_c99_no_write_enter((kiln_gc_heap *)env->heap, (kiln_c99_no_write_frame *)(void *)frame);
}

static void kiln_c99_par_leave(kiln_c99_par_frame *frame) {
    kiln_c99_no_write_leave((kiln_c99_no_write_frame *)(void *)frame);
}

/* The write detector of parallel issue 06: with `KILN_PARALLEL_PROTECT=1` in the environment, a
 * tier-2 call site maps its realm's heap read-only for the length of the pool's run -- every slab
 * and every nursery region, the pages that hold the headers of objects, arrays, strings and
 * views -- so a write any read path forgot (a cache taught, an ASCII answer stored in a string's
 * header, a layout token issued) faults at the write instead of racing. Large blocks are the
 * allocator's and share pages with other memory, so they are not covered. Off, it costs the site
 * one load. Read once per process: 0 not yet read, 1 off, 2 on. */
static int kiln_c99_par_protect_state;

static void kiln_c99_par_protect_regions(kiln_gc_heap *heap, int prot) {
#if defined(__unix__) || defined(__APPLE__)
    kiln_gc_slab *slab;
    size_t index;
    for (slab = heap->all_slabs; slab != NULL; slab = slab->all_next) {
        (void)mprotect(slab, KILN_GC_SLAB_SIZE, prot);
    }
    for (index = 0; index < heap->nursery.regions.capacity; ++index) {
        void *region = heap->nursery.regions.table[index];
        if (region != NULL) (void)mprotect(region, heap->nursery.capacity, prot);
    }
#else
    (void)heap;
    (void)prot;
#endif
}

/* `frozen` nonzero just before a tier-2 pool run, zero just after it. */
static void kiln_c99_par_section(int frozen) {
    int state = __atomic_load_n(&kiln_c99_par_protect_state, __ATOMIC_RELAXED);
    kiln_gc_heap *heap;
    if (KILN_UNLIKELY(state == 0)) {
        const char *setting = getenv("KILN_PARALLEL_PROTECT");
        state = setting != NULL && setting[0] == '1' ? 2 : 1;
        __atomic_store_n(&kiln_c99_par_protect_state, state, __ATOMIC_RELAXED);
    }
    if (KILN_LIKELY(state != 2)) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return;
#if defined(__unix__) || defined(__APPLE__)
    kiln_c99_par_protect_regions(heap, frozen ? PROT_READ : PROT_READ | PROT_WRITE);
#endif
}

/* A compiled unit's block in this thread's realm, made on the realm's first entry into the unit
 * (see `kiln_c99_unit`). The slow half of `kiln_c99_unit_vals`: taken once per realm switch on
 * this thread, then the cache answers until the generation moves. */
static kiln_val *kiln_c99_unit_vals_slow(kiln_c99_unit *unit, kiln_c99_unit_cache *cache) {
    kiln_gc_heap *heap;
    kiln_c99_realm_state *realm;
    kiln_c99_unit_block *block;
    uint32_t id = __atomic_load_n(&unit->id, __ATOMIC_ACQUIRE);
    if (id == 0) {
        uint32_t fresh =
            __atomic_add_fetch(&kiln_c99_process()->next_unit_id, 1u, __ATOMIC_RELAXED);
        uint32_t expected = 0;
        id = __atomic_compare_exchange_n(&unit->id, &expected, fresh, 0, __ATOMIC_ACQ_REL,
                                         __ATOMIC_ACQUIRE)
                 ? fresh
                 : expected;
    }
    if (cache->generation_slot == NULL) cache->generation_slot = kiln_realm_generation_slot();
    /* Asked before the generation is read: attaching the realm's heap moves it. */
    heap = kiln_c99_gc_heap();
    realm = heap != NULL ? &heap->realm : NULL;
    if (realm != NULL && id >= realm->unit_capacity) {
        size_t capacity = realm->unit_capacity < 16 ? 16 : realm->unit_capacity;
        kiln_c99_unit_block **grown;
        while (capacity <= id) capacity *= 2;
        grown = (kiln_c99_unit_block **)realloc(realm->units, capacity * sizeof *grown);
        if (grown == NULL) {
            realm = NULL;
        } else {
            memset(grown + realm->unit_capacity, 0,
                   (capacity - realm->unit_capacity) * sizeof *grown);
            realm->units = grown;
            realm->unit_capacity = capacity;
        }
    }
    block = realm != NULL ? realm->units[id] : NULL;
    if (realm != NULL && block == NULL) {
        block = (kiln_c99_unit_block *)calloc(
            1, sizeof *block + (size_t)unit->val_count * sizeof(kiln_val));
        if (block != NULL) {
            block->val_count = unit->val_count;
            realm->units[id] = block;
        }
    }
    if (block == NULL) {
        /* A unit's module state was a static array that could not fail to exist; the block that
         * replaces it has nowhere to fail to. */
        fputs("kiln: out of memory for a module's realm state\n", stderr);
        abort();
    }
    if (cache->generation_slot != NULL) {
        cache->generation = *cache->generation_slot;
        cache->vals = block->vals;
    }
    return block->vals;
}

/* A unit whose constant keys could not be registered before the first realm -- a library loaded
 * into a process that already had one -- interns them into the realm that enters it, and its key
 * statics then hold that realm's pointers. Only one live realm can own them at a time: `owner` is
 * that realm's heap serial, claimed at the unit's entry and given back at the realm's reset. 1:
 * this realm owns them already. 0: this realm just claimed them and must intern. -1: another
 * live realm owns them, and the entry must refuse rather than hand this realm the other's keys.
 * (Registering them late, or keeping a copy per realm, would need every live realm's key table or
 * an indirection on every key use; a unit loaded after realms exist is the rare case.) */
static int kiln_c99_unit_keys_claim(uint64_t *owner) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    uint64_t unowned = 0;
    if (heap == NULL || heap->serial == 0) return 0;
    if (__atomic_load_n(owner, __ATOMIC_ACQUIRE) == heap->serial) return 1;
    return __atomic_compare_exchange_n(owner, &unowned, heap->serial, 0, __ATOMIC_ACQ_REL,
                                       __ATOMIC_ACQUIRE)
               ? 0
               : -1;
}

static void kiln_c99_unit_keys_release(uint64_t *owner, void *opaque) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    uint64_t serial;
    if (heap == NULL) return;
    serial = heap->serial;
    (void)__atomic_compare_exchange_n(owner, &serial, 0, 0, __ATOMIC_RELEASE, __ATOMIC_RELAXED);
}

#ifndef KILN_C99_UNIT_VALS_DEFINED
#define KILN_C99_UNIT_VALS_DEFINED
static inline kiln_val *kiln_c99_unit_vals(kiln_c99_unit *unit, kiln_c99_unit_cache *cache) {
    const uint64_t *slot = cache->generation_slot;
    if (KILN_LIKELY(slot != NULL && *slot == cache->generation)) return cache->vals;
    return kiln_c99_unit_vals_slow(unit, cache);
}
#endif

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

#if defined(__unix__) || defined(__APPLE__)
enum {
    KILN_PROFILE_PROPERTIES,
    KILN_PROFILE_SLOTS,
    KILN_PROFILE_SEALED_VALUES,
    KILN_PROFILE_OBJECT_ELEMENTS,
    KILN_PROFILE_ARRAY_ELEMENTS,
    KILN_PROFILE_UPVALUES,
    KILN_PROFILE_SIDE_COUNT
};

enum { KILN_PROFILE_SOURCE_LIMIT = 128 };
typedef struct kiln_c99_heap_profile_source {
    const char *name;
    uint64_t count;
    uint64_t payload;
    uint64_t capturing;
} kiln_c99_heap_profile_source;

/* The optional object inventory prints key spellings only, never values. It identifies
 * candidate bootstrap subtrees without adding allocation-site fields to ordinary objects. */
static void kiln_c99_heap_profile_object(const kiln_gc_block *block, uint64_t serial) {
    const kiln_object *object = (const kiln_object *)(block + 1);
    kiln_c99_introspect_line line = {0};
    size_t count = object->shape != NULL ? object->shape->property_count : object->count;
    size_t index;
    kiln_c99_introspect_text(&line, "kiln heap-profile object");
    kiln_c99_introspect_field(&line, "serial", serial);
    kiln_c99_introspect_field(&line, "address", (uintptr_t)object);
    kiln_c99_introspect_field(&line, "payload", block->size);
    kiln_c99_introspect_field(&line, "keys", count);
    kiln_c99_introspect_field(&line, "sealed", object->sealed != NULL);
    kiln_c99_introspect_field(&line, "property_capacity", object->capacity);
    kiln_c99_introspect_field(&line, "slot_capacity", object->slot_capacity);
    kiln_c99_introspect_field(&line, "element_capacity", object->element_capacity);
    for (index = 0; index < count && index < 12 &&
                    (object->shape != NULL || object->properties != NULL); ++index) {
        const kiln_string *key = object->shape != NULL ? object->shape->keys[index]
                                                        : object->properties[index].key;
        size_t unit;
        if (key == NULL) continue;
        kiln_c99_introspect_text(&line, " key=");
        for (unit = 0; unit < key->unit_len && unit < 28 && line.len + 1 < sizeof line.bytes;
             ++unit) {
            uint32_t ch = kiln_string_unit_at(key, unit);
            line.bytes[line.len++] = ch >= 33 && ch <= 126 && ch != '=' ? (char)ch : '_';
        }
    }
    kiln_c99_introspect_flush(&line);
}

static void kiln_c99_heap_profile_count(const kiln_gc_block *block,
                                        uint64_t counts[KILN_GC_KIND_COUNT],
                                        uint64_t bytes[KILN_GC_KIND_COUNT],
                                        uint64_t closures[4],
                                        uint64_t side[KILN_PROFILE_SIDE_COUNT],
                                        kiln_c99_heap_profile_source *sources,
                                        size_t *source_count, uint64_t detail_serial) {
    uint32_t kind = block->kind & KILN_GC_KIND_MASK;
    if (kind == KILN_GC_NONE || kind >= KILN_GC_KIND_COUNT ||
        (block->kind & KILN_GC_FLAG_DEAD) != 0) return;
    ++counts[kind];
    bytes[kind] += block->size;
    if (kind == KILN_GC_OBJECT && block->size >= sizeof(kiln_object)) {
        const kiln_object *object = (const kiln_object *)(block + 1);
        if (detail_serial != 0) kiln_c99_heap_profile_object(block, detail_serial);
        if (object->properties != NULL && !kiln_c99_object_properties_in_block(object))
            side[KILN_PROFILE_PROPERTIES] += object->capacity * sizeof(kiln_property);
        if (object->sealed != NULL) {
            side[KILN_PROFILE_SEALED_VALUES] += sizeof(kiln_sealed_values) +
                object->slot_capacity * (sizeof(kiln_val) + sizeof(uint32_t));
        } else if (kiln_c99_object_slots_owned(object)) {
            side[KILN_PROFILE_SLOTS] += object->slot_capacity * sizeof(kiln_val);
        }
        if (object->elements != NULL)
            side[KILN_PROFILE_OBJECT_ELEMENTS] += object->element_capacity * sizeof(kiln_val) +
                (object->candidate_elements ? sizeof(kiln_c99_candidate_object_elements) : 0);
    }
    if (kind == KILN_GC_ARRAY && block->size >= sizeof(kiln_array)) {
        const kiln_array *array = (const kiln_array *)(block + 1);
        if (kiln_c99_array_elements_owned(array))
            side[KILN_PROFILE_ARRAY_ELEMENTS] += array->capacity * sizeof(kiln_val) +
                (array->candidate_elements ? sizeof(kiln_c99_candidate_array_elements) : 0);
    }
    if (kind == KILN_GC_CLOSURE && block->size >= sizeof(kiln_closure)) {
        const kiln_closure *closure = (const kiln_closure *)(block + 1);
        const char *name = closure->diagnostic_source != NULL
                               ? closure->diagnostic_source : "<runtime>";
        size_t source;
        for (source = 0; source < *source_count; ++source) {
            if (sources[source].name == name || strcmp(sources[source].name, name) == 0) break;
        }
        if (source == *source_count && source < KILN_PROFILE_SOURCE_LIMIT - 1) {
            sources[source].name = name;
            ++*source_count;
        }
        if (source == *source_count) {
            source = KILN_PROFILE_SOURCE_LIMIT - 1;
            sources[source].name = "<other>";
        }
        ++sources[source].count;
        sources[source].payload += block->size;
        sources[source].capturing += closure->upvalue_count != 0;
        /* These are an opportunity bound, not a sharing proof: a site must also execute once
         * per realm and all its metadata and observable identity need a realm-safe owner. */
        ++closures[closure->upvalue_count != 0 ? 0 : closure->arrow ? 1 : 2];
        if (closure->source == NULL) ++closures[3];
        if (closure->upvalues != NULL && closure->upvalues != closure->inline_upvalues)
            side[KILN_PROFILE_UPVALUES] += closure->upvalue_count * sizeof(kiln_val);
    }
}

static void kiln_c99_heap_profile_chunk(const kiln_nursery_chunk *chunk, const char *limit,
                                        uint64_t counts[KILN_GC_KIND_COUNT],
                                        uint64_t bytes[KILN_GC_KIND_COUNT],
                                        uint64_t closures[4],
                                        uint64_t side[KILN_PROFILE_SIDE_COUNT],
                                        kiln_c99_heap_profile_source *sources,
                                        size_t *source_count, uint64_t detail_serial) {
    size_t word;
    for (word = 0; word < chunk->bitmap_words; ++word) {
        uint64_t bits = chunk->bitmap[word];
        while (bits != 0) {
            size_t bit = (word << 6) + (size_t)__builtin_ctzll(bits);
            const char *address = chunk->base + (bit << 4);
            bits &= bits - 1;
            if (address + sizeof(kiln_gc_block) <= limit)
                kiln_c99_heap_profile_count((const kiln_gc_block *)address, counts, bytes,
                                            closures, side, sources, source_count, detail_serial);
        }
    }
}

/* A promoted chunk may contain very little live payload yet keep most of its pages resident.
 * Count pages crossed by each bitmap-live block, including its header and the complete payload;
 * optionally discard pages no block can read again. Only complete body pages qualify: a partial
 * edge page may also hold the chunk header or bitmap. */
#if !defined(MADV_DONTNEED) && defined(__linux__)
#define MADV_DONTNEED 4 /* Linux's stable numeric advice when strict C11 hides the macro. */
#endif
static size_t kiln_c99_nursery_occupied_pages(const kiln_nursery_chunk *chunk,
                                              size_t *complete_pages, int reclaim) {
    long host_page = sysconf(_SC_PAGESIZE);
    uintptr_t begin, end;
    size_t pages, occupied = 0, word;
    unsigned char *live;
    if (complete_pages != NULL) *complete_pages = 0;
    if (host_page <= 0) return 0;
    begin = ((uintptr_t)chunk->base + (uintptr_t)host_page - 1) /
            (uintptr_t)host_page * (uintptr_t)host_page;
    end = (uintptr_t)chunk->limit / (uintptr_t)host_page * (uintptr_t)host_page;
    if (end <= begin) return 0;
    pages = (end - begin) / (size_t)host_page;
    live = (unsigned char *)calloc(pages, 1);
    if (live == NULL) return 0;
    for (word = 0; word < chunk->bitmap_words; ++word) {
        uint64_t bits = chunk->bitmap[word];
        while (bits != 0) {
            size_t bit = (word << 6) + (size_t)__builtin_ctzll(bits);
            const kiln_gc_block *block =
                (const kiln_gc_block *)(chunk->base + (bit << 4));
            uintptr_t first = (uintptr_t)block;
            uintptr_t last;
            size_t page_index;
            bits &= bits - 1;
            if (first < (uintptr_t)chunk->base ||
                first > (uintptr_t)chunk->limit ||
                (uintptr_t)chunk->limit - first < sizeof *block ||
                block->size > (uintptr_t)chunk->limit - first - sizeof *block) {
                free(live);
                return 0;
            }
            last = first + sizeof *block + block->size;
            if (last <= begin || first >= end) continue;
            if (first < begin) first = begin;
            if (last > end) last = end;
            for (page_index = (first - begin) / (size_t)host_page;
                 page_index <= (last - 1 - begin) / (size_t)host_page; ++page_index)
                live[page_index] = 1;
        }
    }
    for (word = 0; word < pages; ++word) occupied += live[word] != 0;
    if (reclaim) {
#ifdef MADV_DONTNEED
        for (word = 0; word < pages;) {
            size_t start = word;
            if (live[word] != 0) { ++word; continue; }
            while (word < pages && live[word] == 0) ++word;
            /* These pages contain no bitmap-live block, including an interior block span.
             * The region remains reserved and its header/bitmap stay on separate pages. */
            extern int madvise(void *, size_t, int);
            (void)madvise((void *)(begin + start * (size_t)host_page),
                          (word - start) * (size_t)host_page, MADV_DONTNEED);
        }
#else
        (void)reclaim;
#endif
    }
    free(live);
    if (complete_pages != NULL) *complete_pages = pages;
    return occupied;
}

static void kiln_c99_heap_profile_pages(const kiln_nursery_chunk *chunk, uint64_t serial,
                                        uint32_t pass, size_t index) {
    size_t pages = 0;
    size_t occupied = kiln_c99_nursery_occupied_pages(chunk, &pages, 0);
    kiln_c99_introspect_line line;
    if (pages == 0) return;
    line.len = 0;
    kiln_c99_introspect_text(&line, "kiln heap-profile pages");
    kiln_c99_introspect_field(&line, "serial", serial);
    kiln_c99_introspect_field(&line, "pass", pass);
    kiln_c99_introspect_field(&line, "index", index);
    kiln_c99_introspect_field(&line, "page_size", kiln_c99_region_page());
    kiln_c99_introspect_field(&line, "complete", pages);
    kiln_c99_introspect_field(&line, "occupied", occupied);
    kiln_c99_introspect_flush(&line);
}

/* Read-only, opt-in sampling at two turn boundaries. A public actor's second boundary includes
 * its target module and constructor, which the bootstrap-complete hook cannot see. The kind
 * totals are payload rather than RSS; chunk and slab counts expose the allocator residency that
 * a small live graph can keep behind it. */
static void kiln_c99_heap_profile_dump(kiln_gc_heap *heap, uint32_t pass) {
    static const char *names[KILN_GC_KIND_COUNT] = {
        "none", "string", "sliced_string", "bytes", "object", "closure", "cell", "array",
        "shared_facade", "generator", "promise", "settler", "bound", "collection",
        "proxy", "date", "regexp", "symbol", "typed_buffer", "typed_array", "data_view",
        "commonjs_require", "method_store", "node_server", "node_socket",
        "node_client_request", "node_response", "node_completion", "node_fs_completion",
        "node_process_completion", "next_tick", "timer"
    };
    uint64_t counts[KILN_GC_KIND_COUNT] = {0};
    uint64_t bytes[KILN_GC_KIND_COUNT] = {0};
    uint64_t closures[4] = {0};
    uint64_t side[KILN_PROFILE_SIDE_COUNT] = {0};
    kiln_c99_heap_profile_source sources[KILN_PROFILE_SOURCE_LIMIT] = {{0}};
    size_t source_count = 0;
    const char *detail_setting = getenv("KILN_HEAP_PROFILE_OBJECTS");
    uint64_t detail_serial = pass == 2 && detail_setting != NULL &&
                             detail_setting[0] == '1' && detail_setting[1] == '\0'
                                 ? heap->serial : 0;
    uint64_t total_count = 0, total_bytes = 0;
    size_t slabs = 0, promoted = 0, full = 0;
    uint32_t kind;
    kiln_c99_introspect_line line;
    kiln_gc_block *block;
    kiln_gc_slab *slab;
    kiln_nursery_chunk *chunk;
    for (block = heap->head; block != NULL; block = block->next)
        kiln_c99_heap_profile_count(block, counts, bytes, closures, side,
                                    sources, &source_count, detail_serial);
    for (slab = heap->all_slabs; slab != NULL; slab = slab->all_next) {
        uint32_t index;
        ++slabs;
        for (index = 0; index < slab->fresh; ++index) {
            block = kiln_c99_gc_slab_slot(slab, index);
            if (block->size != KILN_GC_SLOT_FREE)
                kiln_c99_heap_profile_count(block, counts, bytes, closures, side,
                                            sources, &source_count, detail_serial);
        }
    }
    for (chunk = heap->nursery.promoted; chunk != NULL; chunk = chunk->next) {
        line.len = 0;
        kiln_c99_introspect_text(&line, "kiln heap-profile chunk");
        kiln_c99_introspect_field(&line, "serial", heap->serial);
        kiln_c99_introspect_field(&line, "pass", pass);
        kiln_c99_introspect_field(&line, "index", promoted);
        kiln_c99_introspect_field(&line, "blocks", chunk->alloc_count);
        kiln_c99_introspect_field(&line, "payload", chunk->alloc_bytes);
        kiln_c99_introspect_field(&line, "used", (size_t)(chunk->limit - chunk->base));
        kiln_c99_introspect_flush(&line);
        kiln_c99_heap_profile_pages(chunk, heap->serial, pass, promoted);
        ++promoted;
        kiln_c99_heap_profile_chunk(chunk, chunk->limit, counts, bytes, closures, side,
                                    sources, &source_count, detail_serial);
    }
    for (chunk = heap->nursery.full; chunk != NULL; chunk = chunk->next) {
        ++full;
        kiln_c99_heap_profile_chunk(chunk, chunk->limit, counts, bytes, closures, side,
                                    sources, &source_count, detail_serial);
    }
    if (heap->nursery.active && heap->nursery.region != NULL)
        kiln_c99_heap_profile_chunk((const kiln_nursery_chunk *)heap->nursery.region,
                                    heap->nursery.bump, counts, bytes, closures, side,
                                    sources, &source_count, detail_serial);
    for (kind = 1; kind < KILN_GC_KIND_COUNT; ++kind) {
        total_count += counts[kind];
        total_bytes += bytes[kind];
    }
    line.len = 0;
    kiln_c99_introspect_text(&line, "kiln heap-profile");
    kiln_c99_introspect_field(&line, "serial", heap->serial);
    kiln_c99_introspect_field(&line, "pass", pass);
    kiln_c99_introspect_field(&line, "blocks", total_count);
    kiln_c99_introspect_field(&line, "payload", total_bytes);
    kiln_c99_introspect_field(&line, "heap_blocks", heap->count);
    kiln_c99_introspect_field(&line, "heap_payload", heap->bytes);
    kiln_c99_introspect_field(&line, "slabs", slabs);
    kiln_c99_introspect_field(&line, "promoted", promoted);
    kiln_c99_introspect_field(&line, "full", full);
    kiln_c99_introspect_field(&line, "active", heap->nursery.active);
    kiln_c99_introspect_field(&line, "spare", heap->nursery.spare_count);
    kiln_c99_introspect_flush(&line);
    line.len = 0;
    kiln_c99_introspect_text(&line, "kiln heap-profile side-capacity");
    kiln_c99_introspect_field(&line, "serial", heap->serial);
    kiln_c99_introspect_field(&line, "pass", pass);
    kiln_c99_introspect_field(&line, "properties", side[KILN_PROFILE_PROPERTIES]);
    kiln_c99_introspect_field(&line, "slots", side[KILN_PROFILE_SLOTS]);
    kiln_c99_introspect_field(&line, "sealed_values", side[KILN_PROFILE_SEALED_VALUES]);
    kiln_c99_introspect_field(&line, "object_elements", side[KILN_PROFILE_OBJECT_ELEMENTS]);
    kiln_c99_introspect_field(&line, "array_elements", side[KILN_PROFILE_ARRAY_ELEMENTS]);
    kiln_c99_introspect_field(&line, "upvalues", side[KILN_PROFILE_UPVALUES]);
    kiln_c99_introspect_flush(&line);
    if (counts[KILN_GC_CLOSURE] != 0) {
        line.len = 0;
        kiln_c99_introspect_text(&line, "kiln heap-profile closures");
        kiln_c99_introspect_field(&line, "serial", heap->serial);
        kiln_c99_introspect_field(&line, "pass", pass);
        kiln_c99_introspect_field(&line, "capturing", closures[0]);
        kiln_c99_introspect_field(&line, "arrows_no_upvalues", closures[1]);
        kiln_c99_introspect_field(&line, "plain_no_upvalues", closures[2]);
        kiln_c99_introspect_field(&line, "runtime_source", closures[3]);
        kiln_c99_introspect_flush(&line);
    }
    for (kind = 0; kind < source_count +
                           (sources[KILN_PROFILE_SOURCE_LIMIT - 1].count != 0); ++kind) {
        size_t source = kind < source_count ? kind : KILN_PROFILE_SOURCE_LIMIT - 1;
        line.len = 0;
        kiln_c99_introspect_text(&line, "kiln heap-profile closure-source");
        kiln_c99_introspect_field(&line, "serial", heap->serial);
        kiln_c99_introspect_field(&line, "pass", pass);
        kiln_c99_introspect_field(&line, "count", sources[source].count);
        kiln_c99_introspect_field(&line, "payload", sources[source].payload);
        kiln_c99_introspect_field(&line, "capturing", sources[source].capturing);
        kiln_c99_introspect_text(&line, " source=");
        kiln_c99_introspect_text(&line, sources[source].name);
        kiln_c99_introspect_flush(&line);
    }
    for (kind = 1; kind < KILN_GC_KIND_COUNT; ++kind) {
        if (counts[kind] == 0) continue;
        line.len = 0;
        kiln_c99_introspect_text(&line, "kiln heap-profile");
        kiln_c99_introspect_field(&line, "serial", heap->serial);
        kiln_c99_introspect_field(&line, "pass", pass);
        kiln_c99_introspect_text(&line, " kind=");
        kiln_c99_introspect_text(&line, names[kind]);
        kiln_c99_introspect_field(&line, "count", counts[kind]);
        kiln_c99_introspect_field(&line, "payload", bytes[kind]);
        kiln_c99_introspect_flush(&line);
    }
}
static void kiln_c99_nursery_reclaim_pages(const kiln_nursery_chunk *chunk) {
#ifdef MADV_DONTNEED
    (void)kiln_c99_nursery_occupied_pages(chunk, NULL, 1);
#else
    (void)chunk;
#endif
}
#else
static void kiln_c99_nursery_reclaim_pages(const kiln_nursery_chunk *chunk) {
    (void)chunk;
}
#endif

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
    slab->fresh = 0;
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

/* One slot of `slab`, which has one: a freed slot first, so the pages already touched are
 * reused, and otherwise the next slot never handed out. The caller writes the header's kind,
 * mark and size before anything can look the slot up. */
static inline kiln_gc_block *kiln_c99_gc_slab_take(kiln_gc_heap *heap, kiln_gc_slab *slab) {
    kiln_gc_block *block = slab->free_head;
    if (block != NULL) {
        slab->free_head = block->next;
    } else {
        block = kiln_c99_gc_slab_slot(slab, slab->fresh);
        slab->fresh += 1;
        block->prev = NULL;
    }
    block->next = NULL;
    if (--slab->free_count == 0) kiln_c99_gc_slab_unlink_free(heap, slab);
    return block;
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
        if (slot_index >= slab->fresh) return NULL;
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

/* The chunk a nursery block lives in. Only for a block the heap already knows is its own and
 * carries `KILN_GC_FLAG_NURSERY`: the region is the block's address masked to the chunk size,
 * and its header is read without the membership check a candidate pointer needs. */
static inline kiln_nursery_chunk *kiln_c99_nursery_chunk_of(const kiln_gc_heap *heap,
                                                           const kiln_gc_block *block) {
    return (kiln_nursery_chunk *)((uintptr_t)block & ~(uintptr_t)(heap->nursery.capacity - 1));
}

/* Record a nursery block the marking has just reached, in its chunk's mark bitmap and counts. */
static inline void kiln_c99_nursery_note_mark(const kiln_gc_heap *heap, const kiln_gc_block *block) {
    kiln_nursery_chunk *chunk = kiln_c99_nursery_chunk_of(heap, block);
    size_t bit_idx = (size_t)((const char *)block - chunk->base) >> 4;
    if (KILN_UNLIKELY(chunk->mark_epoch != heap->epoch)) {
        const char *limit = (char *)chunk == heap->nursery.region ? heap->nursery.bump : chunk->limit;
        size_t words = ((size_t)(limit - chunk->base) / 16 + 63) / 64;
        if (words > chunk->bitmap_words) words = chunk->bitmap_words;
        memset(chunk->marks, 0, words * sizeof(uint64_t));
        chunk->marked_count = 0;
        chunk->marked_bytes = 0;
        chunk->mark_epoch = heap->epoch;
    }
    chunk->marks[bit_idx >> 6] |= (uint64_t)1 << (bit_idx & 63);
    chunk->marked_count += 1;
    chunk->marked_bytes += block->size;
}

/* Block epochs are realm-local. A transferred nursery block can carry the same header mark
 * as the receiver's next collection, so the owning chunk's bitmap is the final authority.
 * Resetting that chunk epoch on handoff invalidates all old marks without touching payloads. */
static inline int kiln_c99_gc_is_marked(const kiln_gc_heap *heap, const kiln_gc_block *block) {
    const kiln_nursery_chunk *chunk;
    size_t bit;
    if (block == NULL || block->mark != heap->epoch) return 0;
    if ((block->kind & KILN_GC_FLAG_NURSERY) == 0) return 1;
    chunk = kiln_c99_nursery_chunk_of(heap, block);
    if (chunk->mark_epoch != heap->epoch) return 0;
    bit = (size_t)((const char *)block - chunk->base) >> 4;
    return (chunk->marks[bit >> 6] & (UINT64_C(1) << (bit & 63))) != 0;
}

/* A nursery block released by hand: its bits go, and its chunk holds one block fewer. */
static void kiln_c99_nursery_note_release(kiln_gc_heap *heap, const kiln_gc_block *block) {
    kiln_nursery_chunk *chunk = kiln_c99_nursery_chunk_of(heap, block);
    size_t bit_idx = (size_t)((const char *)block - chunk->base) >> 4;
    uint64_t bit = (uint64_t)1 << (bit_idx & 63);
    chunk->bitmap[bit_idx >> 6] &= ~bit;
    chunk->finals[bit_idx >> 6] &= ~bit;
    if ((char *)chunk == heap->nursery.region) {
        heap->nursery.count -= 1;
        heap->nursery.bytes -= block->size;
    } else {
        chunk->alloc_count -= 1;
        chunk->alloc_bytes -= block->size;
    }
}

/* `payload` now owns memory its finaliser must release. A nursery block of a kind that was not
 * finalised from birth (see `KILN_GC_FINALIZED_AT_ALLOC`) says so here, or a collection that
 * finds it dead would skip it unread and leak what it owns. Slab and large blocks are always
 * finalised by their sweep, which reads every header anyway. */
static void kiln_c99_gc_note_finalizable(const void *payload) {
    const kiln_gc_block *block;
    kiln_gc_heap *heap;
    if (payload == NULL) return;
    block = (const kiln_gc_block *)payload - 1;
    if ((block->kind & KILN_GC_FLAG_NURSERY) == 0 || (block->kind & KILN_GC_FLAG_LARGE) != 0) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL || heap->nursery.capacity == 0) return;
    {
        kiln_nursery_chunk *chunk = kiln_c99_nursery_chunk_of(heap, block);
        size_t bit_idx = (size_t)((const char *)block - chunk->base) >> 4;
        chunk->finals[bit_idx >> 6] |= (uint64_t)1 << (bit_idx & 63);
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

/* Resolve provenance through live heap indexes before reading a header. An arbitrary tagged
 * word or stale owner witness cannot authorize a dereference. Ordinary slab values have no
 * candidate group; large candidates use the cold reverse map. */
static kiln_c99_candidate_group *kiln_c99_candidate_block_group(
    const kiln_gc_heap *heap, const kiln_gc_block *block) {
    if (block == NULL) return NULL;
    if ((block->kind & KILN_GC_FLAG_NURSERY) != 0) {
        const kiln_nursery_chunk *chunk = kiln_c99_nursery_chunk_of(heap, block);
        return chunk->candidate_group;
    }
    if ((block->kind & KILN_GC_FLAG_CANDIDATE_LARGE) != 0) {
        const kiln_c99_candidate_large *owner = kiln_c99_candidate_large_find(heap, block);
        return owner == NULL ? NULL : owner->group;
    }
    return NULL;
}

static kiln_gc_block *kiln_c99_gc_lookup_interior(const kiln_gc_heap *, uintptr_t);

/* Remember possible owners, not slot addresses: growth can replace side buffers, and clearing
 * an edge must permit retry. Metadata does not root either endpoint. The admission walker will
 * inspect the owner's current complete layout. Missing coverage still requires the full scan. */
static void kiln_c99_candidate_record_pointer_store(kiln_gc_heap *heap,
                                                    const void *owner_payload,
                                                    const void *target_payload) {
    kiln_gc_block *target, *owner;
    kiln_c99_candidate_group *group;
    if (heap == NULL || heap->candidate_ledger_groups == 0 || target_payload == NULL ||
        kiln_c99_no_write_depth != 0) return;
    target = kiln_c99_gc_lookup(heap, target_payload);
    if (target == NULL && (uintptr_t)target_payload >= heap->lo &&
        (uintptr_t)target_payload < heap->hi) {
        target = kiln_c99_gc_lookup_interior(heap, (uintptr_t)target_payload);
        /* Large-block interior addresses are absent from the collector's exact-payload
         * table. Only the cold candidate map can supply their live allocation authority. */
        if (target == NULL && heap->candidate_large != NULL) {
            for (size_t index = 0; index < heap->candidate_large->capacity; ++index) {
                kiln_c99_candidate_large *entry = heap->candidate_large->table[index];
                uintptr_t address = (uintptr_t)target_payload;
                uintptr_t base;
                if (entry == NULL || entry == KILN_C99_CANDIDATE_LARGE_TOMBSTONE) continue;
                base = (uintptr_t)entry->block;
                if (address >= base && (address - base < sizeof(kiln_gc_block) ||
                    address - base - sizeof(kiln_gc_block) < entry->block->size)) {
                    target = entry->block;
                    break;
                }
            }
        }
    }
    group = kiln_c99_candidate_block_group(heap, target);
    if (group == NULL || group->ledger_heap != heap || group->incoming_failed) return;
    owner = kiln_c99_gc_lookup(heap, owner_payload);
    /* Unit-block cells and native root slots have their own complete realm/provider audit. */
    if (owner == NULL || kiln_c99_candidate_block_group(heap, owner) == group ||
        kiln_c99_gc_ptrset_contains(&group->incoming_owners, owner)) return;
    if (group->incoming_owners.count == SIZE_MAX ||
        group->incoming_owners.count >= SIZE_MAX / 2 ||
        kiln_c99_gc_ptrset_insert(&group->incoming_owners, owner) != KILN_C99_OK)
        group->incoming_failed = 1;
}

static KILN_ALWAYS_INLINE inline void kiln_c99_candidate_record_val_store(
    const void *owner_payload, kiln_val value) {
    if (owner_payload == NULL || KILN_VAL_IS_NUMBER(value) ||
        value < KILN_TAG_STRING_PREFIX || kiln_c99_no_write_depth != 0) return;
    kiln_c99_candidate_record_pointer_store(kiln_c99_gc_heap(), owner_payload,
                                            KILN_VAL_PTR(value));
}

/* Decoupled emitted units keep the immediate-value skip inline and call this bridge only
 * for pointer stores. The realm's ledger and worker read-path guard remain runtime-owned. */
static void kiln_c99_heap_record_pointer(const void *owner_payload, const void *pointer) {
    if (owner_payload == NULL || kiln_c99_no_write_depth != 0) return;
    kiln_c99_candidate_record_pointer_store(kiln_c99_gc_heap(), owner_payload, pointer);
}

static KILN_ALWAYS_INLINE inline const void *kiln_c99_gc_value_pointer(kiln_value value) {
    return value.tag == KILN_VALUE_SYMBOL && value.ptr != NULL ? value.ptr
         : (const void *)(uintptr_t)value.bits;
}

/* The slot can live inside its block or an owned side allocation. The owner payload supplies
 * stable identity across side-buffer growth; the slot address is never retained in metadata. */
static KILN_ALWAYS_INLINE inline void kiln_c99_heap_store_val(
    const void *owner_payload, kiln_val *slot, kiln_val value) {
    kiln_c99_candidate_record_val_store(owner_payload, value);
    *slot = value;
}

static KILN_ALWAYS_INLINE inline void kiln_c99_heap_store_value(
    const void *owner_payload, kiln_value *slot, kiln_value value) {
    if ((value.tag >= KILN_VALUE_STRING || value.tag == KILN_VALUE_SYMBOL) &&
        kiln_c99_no_write_depth == 0)
        kiln_c99_candidate_record_pointer_store(kiln_c99_gc_heap(), owner_payload,
                                                kiln_c99_gc_value_pointer(value));
    *slot = value;
}

/* Object-pointer fields use memcpy rather than a void** lvalue, preserving their declared
 * pointer type under strict aliasing. Callers supply the audited GC owner of the field. */
static KILN_ALWAYS_INLINE inline void kiln_c99_heap_store_pointer(
    const void *owner_payload, void *slot, const void *pointer) {
    if (pointer != NULL && owner_payload != NULL && kiln_c99_no_write_depth == 0) {
        /* Keep the usual path free of calls: even a heap getter forces constructor register
         * spills. A missing or stale cache must resolve the current realm before recording. */
        kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
        if (KILN_UNLIKELY(heap == NULL))
            kiln_c99_heap_record_pointer(owner_payload, pointer);
        else if (KILN_UNLIKELY(heap->candidate_ledger_groups != 0))
            kiln_c99_candidate_record_pointer_store(heap, owner_payload, pointer);
    }
    memcpy(slot, &pointer, sizeof pointer);
}

/* An offset constructed from this allocation cannot name another candidate group. Unlike
 * borrowed pointers, these self-owned layout edges need no incoming-owner ledger lookup.
 * Callers use offsetof for inline fields or sizeof(owner type) for an allocated tail. */
static KILN_ALWAYS_INLINE inline void kiln_c99_heap_store_interior_pointer(
    const void *owner_payload, void *slot, size_t offset) {
    const void *pointer = (const char *)owner_payload + offset;
    memcpy(slot, &pointer, sizeof pointer);
}

static void kiln_c99_candidate_record_vals(const void *owner_payload,
                                            const kiln_val *values, size_t count) {
    kiln_gc_heap *heap;
    if (kiln_c99_no_write_depth != 0 || count == 0) return;
    heap = kiln_c99_gc_heap_if_cached();
    if (KILN_UNLIKELY(heap == NULL)) heap = kiln_c99_gc_heap();
    if (heap == NULL || heap->candidate_ledger_groups == 0) return;
    for (size_t index = 0; index < count; ++index) {
        kiln_val value = values[index];
        if (!KILN_VAL_IS_NUMBER(value) && value >= KILN_TAG_STRING_PREFIX)
            kiln_c99_candidate_record_pointer_store(heap, owner_payload, KILN_VAL_PTR(value));
    }
}

static KILN_ALWAYS_INLINE inline void kiln_c99_heap_copy_vals(
    const void *owner_payload, kiln_val *destination, const kiln_val *source, size_t count) {
    memcpy(destination, source, count * sizeof *source);
    if (kiln_c99_no_write_depth != 0 || count == 0) return;
    kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
    if (KILN_UNLIKELY(heap == NULL || heap->candidate_ledger_groups != 0))
        kiln_c99_candidate_record_vals(owner_payload, destination, count);
}

/* Wide boundary-value buffers have the same ownership rule as compact value buffers. */
static void kiln_c99_heap_copy_values(const void *owner_payload, kiln_value *destination,
                                      const kiln_value *source, size_t count) {
    if (count == 0) return;
    memcpy(destination, source, count * sizeof *source);
    if (kiln_c99_no_write_depth != 0) return;
    kiln_gc_heap *heap = kiln_c99_gc_heap_if_cached();
    if (KILN_UNLIKELY(heap == NULL)) heap = kiln_c99_gc_heap();
    if (heap == NULL || heap->candidate_ledger_groups == 0) return;
    for (size_t index = 0; index < count; ++index)
        if (destination[index].tag >= KILN_VALUE_STRING ||
            destination[index].tag == KILN_VALUE_SYMBOL)
            kiln_c99_candidate_record_pointer_store(heap, owner_payload,
                kiln_c99_gc_value_pointer(destination[index]));
}

/* Relocation is restricted to the same owner's existing storage. Witnesses name that
 * stable owner, not a slot address, so moving its bytes cannot introduce a new incoming
 * owner. Cross-owner copies must use heap_copy_vals instead. No allocation or user code
 * runs between the copy and publishing the replacement buffer. */
static KILN_ALWAYS_INLINE inline void kiln_c99_heap_relocate_vals(
    const void *owner_payload, kiln_val *destination, const kiln_val *source, size_t count) {
    (void)owner_payload;
    memcpy(destination, source, count * sizeof *source);
}

/* The source and destination are ranges of one owner's existing value buffer. Overlap
 * changes positions, not incoming owner identity; the weak owner witness remains valid. */
static KILN_ALWAYS_INLINE inline void kiln_c99_heap_move_vals(
    const void *owner_payload, kiln_val *destination, const kiln_val *source, size_t count) {
    (void)owner_payload;
    memmove(destination, source, count * sizeof *source);
}

static void kiln_c99_candidate_record_property(const void *owner_payload,
                                                const kiln_property *property) {
    kiln_gc_heap *heap;
    if (kiln_c99_no_write_depth != 0) return;
    heap = kiln_c99_gc_heap();
    if (heap == NULL || heap->candidate_ledger_groups == 0) return;
    kiln_c99_candidate_record_pointer_store(heap, owner_payload, property->key);
    if (property->refused) return;
    if (property->accessor) {
        if (property->getter.tag >= KILN_VALUE_STRING ||
            property->getter.tag == KILN_VALUE_SYMBOL)
            kiln_c99_candidate_record_pointer_store(heap, owner_payload,
                kiln_c99_gc_value_pointer(property->getter));
        if (property->setter.tag >= KILN_VALUE_STRING ||
            property->setter.tag == KILN_VALUE_SYMBOL)
            kiln_c99_candidate_record_pointer_store(heap, owner_payload,
                kiln_c99_gc_value_pointer(property->setter));
    } else if (property->value.tag >= KILN_VALUE_STRING ||
               property->value.tag == KILN_VALUE_SYMBOL) {
        kiln_c99_candidate_record_pointer_store(heap, owner_payload,
            kiln_c99_gc_value_pointer(property->value));
    }
}

static void kiln_c99_heap_copy_properties(const void *owner_payload,
                                          kiln_property *destination,
                                          const kiln_property *source, size_t count) {
    memcpy(destination, source, count * sizeof *source);
    if (kiln_c99_no_write_depth != 0 || count == 0) return;
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || heap->candidate_ledger_groups == 0) return;
    for (size_t index = 0; index < count; ++index)
        kiln_c99_candidate_record_property(owner_payload, &destination[index]);
}

/* Whole descriptors carry keys and either data or accessor values. Copying their storage
 * through this authority keeps relocation and cross-owner publication in one inventory. */
static void kiln_c99_heap_store_property(const void *owner_payload, kiln_property *slot,
                                        kiln_property value) {
    kiln_c99_candidate_record_property(owner_payload, &value);
    *slot = value;
}

/* Keep `payload` a root until here. The collector finds a C frame's pointers by scanning the
 * stack, and a pointer whose last use is behind it has usually left the stack by the time the
 * frame allocates again; a caller that will hand the payload to something after that -- the
 * queue's drop callback, a finaliser -- names the point it must survive to with this. */
static inline void kiln_c99_gc_keep_alive(const void *payload) {
    __asm__ volatile("" : : "r"(payload) : "memory");
}

/* `kiln_c99_gc_payload_kind` (`prelude_types.h`, where emitted code can inline it) reads the kind
 * word of the `kiln_gc_block` in front of a payload without this struct: keep them agreeing. */
typedef char kiln_c99_block_kind_offset_agrees[sizeof(kiln_gc_block) - offsetof(kiln_gc_block, kind) ==
                                                       KILN_C99_BLOCK_KIND_OFFSET
                                                   ? 1
                                                   : -1];
typedef char kiln_c99_block_kind_mask_agrees[KILN_GC_KIND_MASK == KILN_C99_BLOCK_KIND_MASK ? 1 : -1];
typedef char kiln_c99_block_kind_fits_a_byte[KILN_GC_KIND_COUNT <= 256 ? 1 : -1];

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
    /* A block in the shared region is no heap's, and headed like one (realms issue 08). */
    if (block == NULL && kiln_c99_in_region(ptr)) {
        if ((((const kiln_gc_block *)ptr - 1)->kind & KILN_GC_KIND_MASK) != kind) __builtin_trap();
        return;
    }
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
    if (block != NULL && !kiln_c99_no_write()) {
        /* Only the two ASCII bits change: the placement flags (`KILN_GC_FLAG_NURSERY`,
         * `_LARGE`) are what the marker and the release paths find the block's chunk by, and
         * dropping them here left a nursery string marked in its header but not in its chunk's
         * mark bitmap -- freed live under `KILN_GC_STRESS`. */
        block->kind = (block->kind & ~(KILN_GC_FLAG_ASCII_KNOWN | KILN_GC_FLAG_ASCII)) |
                      KILN_GC_FLAG_ASCII_KNOWN | (ascii ? KILN_GC_FLAG_ASCII : 0u);
    }
    return ascii;
}

static inline int kiln_c99_gc_is_tenured_kind(uint32_t kind) {
    uint32_t k = kind & KILN_GC_KIND_MASK;
    return k == KILN_GC_NODE_SERVER || k == KILN_GC_NODE_SOCKET ||
           k == KILN_GC_NODE_CLIENT_REQUEST || k == KILN_GC_NODE_RESPONSE ||
           k == KILN_GC_NODE_COMPLETION || k == KILN_GC_NODE_FS_COMPLETION ||
           k == KILN_GC_TIMER || k == KILN_GC_SHARED_FACADE;
}

#include <setjmp.h>
#include <pthread.h>

/* A stack bound belongs to an OS thread, not a realm heap. Scheduler turns can move a heap
 * between lanes after every generated frame has returned, so caching the first collector's
 * bound in the heap would scan the old lane's address range on the next mid-turn collection.
 * Query once per thread; a replacement thread gets its own TLS slot even if an OS ID is reused. */
static void *kiln_c99_gc_stack_base(void) {
    static _Thread_local void *stack_base;
    if (stack_base == NULL) {
#if defined(__APPLE__)
        stack_base = pthread_get_stackaddr_np(pthread_self());
#else
        extern int pthread_getattr_np(pthread_t, pthread_attr_t *);
        pthread_attr_t attr;
        void *addr = NULL;
        size_t size = 0;
        if (pthread_getattr_np(pthread_self(), &attr) == 0) {
            if (pthread_attr_getstack(&attr, &addr, &size) == 0 && addr != NULL) {
                stack_base = (char *)addr + size;
            }
            pthread_attr_destroy(&attr);
        }
#endif
    }
    return stack_base;
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
        if (slot_index >= slab->fresh) return NULL;
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
        /* The shared-facade tag uses bit 47; taking a 48-bit address here turns that tag bit
         * into a false pointer and lets a live facade die during a generated expression. */
        uintptr_t address = word & KILN_PAYLOAD_MASK;
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
    uintptr_t *high = (uintptr_t *)kiln_c99_gc_stack_base();
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

/* A site that names a realm-local shape or key registers itself with that realm's heap, which
 * makes it forget them at teardown (`kiln_c99_gc_forget_realm_local`). Keep each list sorted and
 * unique: a shared site can alternate between realm serials on every cache write, so sorting a
 * full, duplicate-heavy list only when it fills spends work proportional to message traffic. */
static size_t kiln_c99_site_slot(kiln_c99_site *const *sites, size_t length,
                                 const kiln_c99_site *site) {
    size_t low = 0, high = length;
    uintptr_t needle = (uintptr_t)site;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        if ((uintptr_t)sites[middle] < needle) low = middle + 1;
        else high = middle;
    }
    return low;
}

static size_t kiln_c99_private_site_slot(kiln_c99_private_site *const *sites, size_t length,
                                         const kiln_c99_private_site *site) {
    size_t low = 0, high = length;
    uintptr_t needle = (uintptr_t)site;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        if ((uintptr_t)sites[middle] < needle) low = middle + 1;
        else high = middle;
    }
    return low;
}

static int kiln_c99_shape_is_local(const kiln_shape *shape) {
    return shape != NULL && !shape->shared;
}

static int kiln_c99_key_is_shared(const kiln_string *ikey);

/* Whether a published entry, whose key word read `shape`, names anything a realm frees at
 * teardown: a realm-local shape, or a realm-local key checked against a list-mode holder's record.
 * The key word is passed, not read again: another thread may claim the entry meanwhile, and
 * while this thread runs a realm the entry's other words stay as they were. */
static int kiln_c99_pic_entry_is_local(const kiln_c99_pic_entry *entry, const kiln_shape *shape) {
    return kiln_c99_shape_is_local(shape) || kiln_c99_shape_is_local(entry->transition_to) ||
           kiln_c99_shape_is_local(entry->level_shape[0]) ||
           kiln_c99_shape_is_local(entry->level_shape[1]) ||
           (entry->key != NULL && !kiln_c99_key_is_shared(entry->key));
}

/* Retire every published entry of `site` that names a realm-local shape or key. That includes
 * another live realm's, which only costs that realm a relearn; what is left names shared shapes
 * and keys, which are never freed. */
static void kiln_c99_site_forget_local(kiln_c99_site *site) {
    uint32_t p, count = kiln_c99_site_count(site);
    for (p = 0; p < count; ++p) {
        const kiln_shape *key = kiln_c99_slot_key(&site->entries[p].shape);
        if (KILN_C99_SLOT_IS_ANSWER(key) && kiln_c99_pic_entry_is_local(&site->entries[p], key))
            kiln_c99_slot_retire(&site->entries[p].shape, key);
    }
}

/* Register `site` with the current heap once an entry names a realm-local shape or key (ADR
 * 0071): the heap frees them at teardown, and first makes every registered site forget them.
 * A site holding only shared shapes -- every site of a program whose keys are all constants --
 * is never registered. `registered` is the serial of the last heap the site joined, so a site
 * two live realms both teach joins each of them; the sorted list contains each site once,
 * which bounds it by the number of distinct sites. `registered` is only a hint against joining
 * twice: two realms racing on it can at worst reach the current heap's sorted lookup again. */
static void kiln_c99_gc_register_site(kiln_c99_site *site) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t slot;
    if (heap == NULL || site == NULL || heap->serial == 0 ||
        __atomic_load_n(&site->registered, __ATOMIC_RELAXED) == heap->serial)
        return;
    slot = kiln_c99_site_slot(heap->sites, heap->site_len, site);
    if (slot < heap->site_len && heap->sites[slot] == site) {
        __atomic_store_n(&site->registered, heap->serial, __ATOMIC_RELAXED);
        return;
    }
    if (heap->site_len == heap->site_cap) {
        size_t wanted = heap->site_cap == 0 ? 64 : heap->site_cap * 2;
        kiln_c99_site **grown = (kiln_c99_site **)realloc(heap->sites, wanted * sizeof *grown);
        if (grown == NULL) {
            /* Unregistered, an entry naming a local shape could outlive it: forget them now. */
            kiln_c99_site_forget_local(site);
            return;
        }
        heap->sites = grown;
        heap->site_cap = wanted;
    }
    memmove(heap->sites + slot + 1, heap->sites + slot,
            (heap->site_len - slot) * sizeof *heap->sites);
    heap->sites[slot] = site;
    heap->site_len++;
    __atomic_store_n(&site->registered, heap->serial, __ATOMIC_RELAXED);
}

/* Every path that writes a site's entries calls this after: a site that now names a realm-local
 * shape is registered with the heap that owns it. */
static void kiln_c99_site_learned(kiln_c99_site *site) {
    uint32_t p, count;
    if (site == NULL) return;
    count = kiln_c99_site_count(site);
    for (p = 0; p < count; ++p) {
        const kiln_shape *key = kiln_c99_slot_key(&site->entries[p].shape);
        if (KILN_C99_SLOT_IS_ANSWER(key) && kiln_c99_pic_entry_is_local(&site->entries[p], key)) {
            kiln_c99_gc_register_site(site);
            return;
        }
    }
}

/* Register a private-field site that is about to learn a realm-local shape with the current heap
 * (see `kiln_c99_gc_register_site`). `registered` is left zero when the heap cannot take it, and
 * the caller then does not learn. */
static void kiln_c99_gc_register_private_site(kiln_c99_private_site *site) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t slot;
    if (heap == NULL || site == NULL || heap->serial == 0) return;
    if (__atomic_load_n(&site->registered, __ATOMIC_RELAXED) == heap->serial) return;
    slot = kiln_c99_private_site_slot(heap->private_sites, heap->private_site_len, site);
    if (slot < heap->private_site_len && heap->private_sites[slot] == site) {
        __atomic_store_n(&site->registered, heap->serial, __ATOMIC_RELAXED);
        return;
    }
    if (heap->private_site_len == heap->private_site_cap) {
        size_t wanted = heap->private_site_cap == 0 ? 64 : heap->private_site_cap * 2;
        kiln_c99_private_site **grown = (kiln_c99_private_site **)realloc(
            heap->private_sites, wanted * sizeof *grown);
        if (grown == NULL) {
            __atomic_store_n(&site->registered, 0, __ATOMIC_RELAXED);
            return;
        }
        heap->private_sites = grown;
        heap->private_site_cap = wanted;
    }
    memmove(heap->private_sites + slot + 1, heap->private_sites + slot,
            (heap->private_site_len - slot) * sizeof *heap->private_sites);
    heap->private_sites[slot] = site;
    heap->private_site_len++;
    __atomic_store_n(&site->registered, heap->serial, __ATOMIC_RELAXED);
}

/* Teardown, before the heap frees anything: every registered site forgets the entries that name
 * a realm-local shape or key. */
static void kiln_c99_gc_forget_realm_local(kiln_gc_heap *heap) {
    size_t index;
    for (index = 0; index < heap->site_len; ++index) {
        kiln_c99_site *site = heap->sites[index];
        kiln_c99_site_forget_local(site);
        (void)__atomic_compare_exchange_n(&site->registered, &(uint64_t){heap->serial}, 0, 0,
                                          __ATOMIC_RELAXED, __ATOMIC_RELAXED);
    }
    free(heap->sites);
    heap->sites = NULL;
    heap->site_len = 0;
    heap->site_cap = 0;
    for (index = 0; index < heap->private_site_len; ++index) {
        kiln_c99_private_site *site = heap->private_sites[index];
        const kiln_shape *key = kiln_c99_slot_key(&site->shape);
        if (KILN_C99_SLOT_IS_ANSWER(key) && !key->shared) kiln_c99_slot_retire(&site->shape, key);
        (void)__atomic_compare_exchange_n(&site->registered, &(uint64_t){heap->serial}, 0, 0,
                                          __ATOMIC_RELAXED, __ATOMIC_RELAXED);
    }
    free(heap->private_sites);
    heap->private_sites = NULL;
    heap->private_site_len = 0;
    heap->private_site_cap = 0;
}

/* Claim an entry of `site` no answer has used yet, raising the count over it; -1 when all are. */
static int kiln_c99_site_claim_fresh(kiln_c99_site *site) {
    uint32_t p;
    for (p = kiln_c99_site_count(site); p < KILN_C99_PIC_SIZE; ++p) {
        const kiln_shape *never = NULL;
        if (__atomic_compare_exchange_n(&site->entries[p].shape, &never, KILN_C99_SLOT_CLAIMED, 0,
                                        __ATOMIC_ACQUIRE, __ATOMIC_RELAXED)) {
            uint32_t count = __atomic_load_n(&site->pic_count, __ATOMIC_RELAXED);
            while (count <= p &&
                   !__atomic_compare_exchange_n(&site->pic_count, &count, p + 1, 1,
                                                __ATOMIC_RELEASE, __ATOMIC_RELAXED)) {
            }
            return (int)p;
        }
    }
    return -1;
}

/* Put `entry` in the site under its receiver shape (ADR 0071, the slot protocol above
 * `kiln_c99_pic_entry`): over the entry that shape already has, else in a free place -- one never
 * used, or a retired one every reader has left -- else over the last. Where the slot to replace
 * may still be under another thread's reader it is retired rather than rewritten, and the answer
 * goes in a free place or waits for the next miss. Then register the site if it now names a
 * realm-local shape (`kiln_c99_site_learned`). */
static void kiln_c99_site_remember(kiln_c99_site *site, const kiln_c99_pic_entry *given) {
    uint32_t p, count;
    int at = -1;
    kiln_c99_pic_entry entry = *given;
    /* The no-write read path consults a site and never teaches it. */
    if (kiln_c99_no_write()) return;
    count = kiln_c99_site_count(site);
    /* An entry is compared against a receiver's shape but its `transition_to` is *given* to the
     * receiver. From a shared shape by a realm-local key the child is the learning realm's own:
     * another realm running now would move its object into that realm's shape (and key). Such a
     * store stays uncached; from a realm-local shape only its own realm can arrive.
     *
     * A keyed entry whose key is realm-local is the exception, because every hit compares the key:
     * that key is the learning realm's own pointer, which no other realm holds while it is alive
     * (another realm's `Symbol()` is another key), and the entry is forgotten before the realm's
     * teardown frees it (`kiln_c99_pic_entry_is_local`). So only the learning realm can arrive. */
    if (entry.transition_to != NULL && !entry.transition_to->shared && entry.shape->shared &&
        !(entry.keyed && entry.key != NULL && !kiln_c99_key_is_shared(entry.key)))
        return;
    entry.one_step = entry.depth == 1 && !entry.absent && entry.transition_to == NULL &&
                     entry.slot == UINT32_MAX && entry.level_shape[0] != NULL;
    for (p = 0; p < count; ++p) {
        if (kiln_c99_slot_key(&site->entries[p].shape) == entry.shape &&
            (!entry.keyed || site->entries[p].key == entry.key)) {
            if (kiln_c99_slot_claim(&site->entries[p].shape, 1)) at = (int)p;
            break;
        }
    }
    if (at < 0) at = kiln_c99_site_claim_fresh(site);
    for (p = 0; at < 0 && p < count; ++p) {
        if (KILN_C99_SLOT_IS_RETIRED(kiln_c99_slot_key(&site->entries[p].shape)) &&
            kiln_c99_slot_claim(&site->entries[p].shape, 0))
            at = (int)p;
    }
    if (at < 0 && count == KILN_C99_PIC_SIZE &&
        kiln_c99_slot_claim(&site->entries[KILN_C99_PIC_SIZE - 1].shape, 1))
        at = KILN_C99_PIC_SIZE - 1;
    if (at < 0) return;
    /* Everything but the key word, then the key word: the entry is whole before it is found. */
    memcpy((char *)&site->entries[at] + sizeof entry.shape, (const char *)&entry + sizeof entry.shape,
           sizeof entry - sizeof entry.shape);
    kiln_c99_slot_publish(&site->entries[at].shape, entry.shape);
    kiln_c99_site_learned(site);
}
/* The copy above leaves out exactly the key word. */
typedef char kiln_c99_pic_entry_key_is_first[offsetof(kiln_c99_pic_entry, shape) == 0 ? 1 : -1];

/* Move entry `hot` to the front (see `kiln_c99_site_answered`): the two entries trade places.
 * That rewrites two published answers, so it happens only while no other thread runs a realm
 * (`kiln_realm_retire_tag` answers 0) -- every single-realm process, where a site that changes
 * phase keeps its hits inline as it always did. Otherwise the entries stay where they are, both
 * still true, and the later one is found by the scan. 1: they traded places. */
static __attribute__((noinline, unused)) int kiln_c99_site_promote(kiln_c99_site *site,
                                                                    uint32_t hot) {
    const kiln_shape *first = kiln_c99_slot_key(&site->entries[0].shape);
    const kiln_shape *later = kiln_c99_slot_key(&site->entries[hot].shape);
    kiln_c99_pic_entry moved;
    if (hot == 0 || hot >= KILN_C99_PIC_SIZE || !KILN_C99_SLOT_IS_ANSWER(first) ||
        !KILN_C99_SLOT_IS_ANSWER(later) ||
        !__atomic_compare_exchange_n(&site->entries[0].shape, &first, KILN_C99_SLOT_CLAIMED, 0,
                                     __ATOMIC_SEQ_CST, __ATOMIC_RELAXED))
        return 0;
    if (!__atomic_compare_exchange_n(&site->entries[hot].shape, &later, KILN_C99_SLOT_CLAIMED, 0,
                                     __ATOMIC_SEQ_CST, __ATOMIC_RELAXED)) {
        kiln_c99_slot_publish(&site->entries[0].shape, first);
        return 0;
    }
    if (kiln_realm_retire_tag() != 0) {
        kiln_c99_slot_publish(&site->entries[hot].shape, later);
        kiln_c99_slot_publish(&site->entries[0].shape, first);
        return 0;
    }
    memcpy((char *)&moved + sizeof moved.shape, (const char *)&site->entries[0] + sizeof moved.shape,
           sizeof moved - sizeof moved.shape);
    memcpy((char *)&site->entries[0] + sizeof moved.shape,
           (const char *)&site->entries[hot] + sizeof moved.shape, sizeof moved - sizeof moved.shape);
    memcpy((char *)&site->entries[hot] + sizeof moved.shape, (const char *)&moved + sizeof moved.shape,
           sizeof moved - sizeof moved.shape);
    kiln_c99_slot_publish(&site->entries[hot].shape, first);
    kiln_c99_slot_publish(&site->entries[0].shape, later);
    return 1;
}

static void kiln_c99_site_remember_own(kiln_c99_site *site, const kiln_shape *shape,
                                       uint32_t slot) {
    kiln_c99_pic_entry entry;
    memset(&entry, 0, sizeof entry);
    entry.shape = shape;
    entry.slot = slot;
    kiln_c99_site_remember(site, &entry);
}

/* A store that moves a receiver from `from` to `to`, writing `slot`. */
static void kiln_c99_site_remember_transition(kiln_c99_site *site, const kiln_shape *from,
                                              const kiln_shape *to, uint32_t slot) {
    kiln_c99_pic_entry entry;
    memset(&entry, 0, sizeof entry);
    entry.shape = from;
    entry.transition_to = to;
    entry.slot = slot;
    kiln_c99_site_remember(site, &entry);
}

/* ADR 0071: name a prototype level the way a cache entry states it -- its shape, or for a level
 * in list mode its layout token, issued now if it has none. 0 when a list-mode level cannot be
 * named, because the process has issued every token there is; the caller then remembers
 * nothing. Tokens come from the process block, so no two objects in any realm share one. */
static int kiln_c99_pic_name_level(const kiln_object *level, const kiln_shape **shape,
                                   uint32_t *token) {
    if (level->shape != NULL) {
        *shape = level->shape;
        *token = 0;
        return 1;
    }
    if (level->layout_token == 0) {
        uint64_t issued;
        /* No-write mode names only a level that already has a token. */
        if (kiln_c99_no_write()) return 0;
        issued =
            __atomic_add_fetch(&kiln_c99_process()->next_layout_token, 1, __ATOMIC_RELAXED);
        if (issued > UINT32_MAX) return 0;
        ((kiln_object *)level)->layout_token = (uint32_t)issued;
    }
    *shape = NULL;
    *token = level->layout_token;
    return 1;
}

static void kiln_c99_gc_collect(void *opaque, int force);

/* A collection from inside a turn, at an allocation. */
static void kiln_c99_gc_collect_mid_turn(kiln_gc_heap *heap) {
    /* A thread in no-write mode -- a parallel section's chunk (ADR 0069, parallel issue 07) --
     * reads a heap that other threads read at the same time, so it must never allocate, and an
     * allocation that reached a collection is a proof the backend admitted by mistake: sweeping
     * now would free memory the other workers are reading. Stop at the cause instead. On the
     * collection path only, so an allocation costs nothing more; `KILN_GC_STRESS`, which
     * collects at every allocation, makes any such allocation arrive here. */
    if (KILN_UNLIKELY(kiln_c99_no_write())) {
        fputs("kiln: a collection was due on a thread reading a frozen heap (a parallel section "
              "allocated)\n",
              stderr);
        abort();
    }
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
            nursery->finals = NULL;
            nursery->bump = NULL;
            nursery->limit = NULL;
            nursery->active = 0;
            return 0;
        }
    }
    kiln_c99_nursery_use(nursery, chunk);
    if (heap->actor_candidate_id != 0)
        kiln_c99_candidate_group_add(heap->actor_candidate_group, chunk);
    kiln_c99_gc_bound(heap, nursery->region, nursery->capacity);
    return 1;
}

/* The current chunk is full: queue it for the next collection and start another. */
static void kiln_c99_nursery_turn_chunk(kiln_gc_heap *heap) {
    kiln_gc_nursery *nursery = &heap->nursery;
    kiln_nursery_chunk *filled;
    if (nursery->bump == nursery->base) return;
    filled = kiln_c99_nursery_seal(nursery);
    if (filled->candidate_id != 0) filled->candidate_epoch = heap->epoch;
    kiln_c99_nursery_full_push(nursery, filled);
    nursery->full_bytes += (size_t)(nursery->bump - nursery->base);
    (void)kiln_c99_nursery_fresh_chunk(heap);
}

/* Memory a cell owns outside the heap -- a typed buffer's bytes -- counts toward the next
 * collection like the cell itself. */
static void kiln_c99_gc_note_external(size_t size) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap != NULL) heap->since_collection += size;
}

static void kiln_c99_gc_note_moved_external(size_t size) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || size == 0) return;
    if (size > SIZE_MAX - heap->moved_external_current) abort();
    heap->moved_external_current += size;
}

static void kiln_c99_gc_unnote_moved_external(size_t size) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || size == 0) return;
    if (size > heap->moved_external_current) abort();
    heap->moved_external_current -= size;
}

/* A filled-chunk collection is worthwhile while the live heap is small enough that the root
 * walk it buys is cheaper than retaining another nursery budget. Keep this predicate shared by
 * allocation and turn-boundary collection so the two entry points cannot drift. */
static inline int kiln_c99_nursery_pressure_due(const kiln_gc_heap *heap) {
    return heap->nursery.full != NULL &&
           heap->nursery.full_bytes - heap->nursery.held_bytes >= heap->nursery.budget_now &&
           heap->survived <= heap->nursery.budget * KILN_NURSERY_SMALL_LIVE_BUDGETS;
}

static inline int kiln_c99_actor_candidate_defer_gc(const kiln_gc_heap *heap) {
    /* Stress mode must still collect on its configured allocation cadence. A collection
     * invalidates candidate provenance instead of silently weakening the stress probe. */
    return heap->stress_every == 0 && heap->actor_candidate_id != 0 &&
           !heap->actor_candidate_collected &&
           heap->nursery.active &&
           heap->nursery.full_bytes < KILN_ACTOR_CANDIDATE_GC_DEFER_BYTES;
}

/* The collection an allocation may owe before it hands anything out.
 *
 * Its own function because a caller taking a run of blocks at once -- `kiln_c99_gc_alloc_text_run`
 * below -- owes it once for the run rather than once per block, and the two must not drift: this
 * is where "is a mid-turn collection due" is decided for both. */
static inline void kiln_c99_gc_alloc_due(kiln_gc_heap *heap) {
    /* A candidate's current and sealed chunks are in this heap, but a collection could
     * promote/reuse them before the factory publishes its result. Defer collection while its
     * nursery stays active and the filled allocation is under its 64 MiB ceiling. Stress mode
     * and either limit resume collection and invalidate candidate provenance. */
    if (KILN_UNLIKELY(kiln_c99_actor_candidate_defer_gc(heap))) return;
    if (KILN_UNLIKELY(!heap->collecting && heap->enabled && !heap->fallback)) {
        /* A mid-turn collection scans the whole stack and sweeps every slab, so it earns its
         * keep only after a few megabytes, or after allocation has matched twice what
         * survived the last one; the turn's end keeps its own, lower, threshold. */
        size_t threshold = heap->survived * 2;
        int due;
        if (threshold < KILN_GC_MID_TURN_FLOOR) threshold = KILN_GC_MID_TURN_FLOOR;
        due = heap->since_collection >= threshold;
        if (!due && heap->moved_external_current > heap->moved_external_baseline) {
            size_t moved_threshold = threshold > KILN_GC_MID_TURN_MOVED_FLOOR
                                         ? threshold : KILN_GC_MID_TURN_MOVED_FLOOR;
            due = heap->moved_external_current - heap->moved_external_baseline >= moved_threshold;
        }
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

/* Ordinary allocations keep large blocks out of the nursery to avoid wasting a chunk. A
 * direct actor factory intends to move its sealed chunks as one group, so a block that fits
 * a fresh chunk should stay inside that group instead of becoming an unmovable large block.
 * The capsule proof still rejects unsupported kinds and outgoing edges. The ordinary small
 * allocation path returns before reading candidate state. */
static inline int kiln_c99_nursery_accepts_block(const kiln_gc_heap *heap, size_t bytes) {
    const kiln_gc_nursery *nursery = &heap->nursery;
    if (bytes <= nursery->capacity >> 2) return 1;
    return heap->actor_candidate_id != 0 && !heap->actor_candidate_collected &&
           nursery->base != NULL && nursery->limit >= nursery->base &&
           bytes <= (size_t)(nursery->limit - nursery->base);
}

static void *kiln_c99_gc_alloc_ex(uint32_t kind, size_t size, int zero_payload) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t payload = size == 0 ? 1 : size;
    int class_index;
    kiln_gc_block *block;
    if (heap == NULL || size > ((size_t)-1) - sizeof(kiln_gc_block)) return NULL;
    kiln_c99_gc_alloc_due(heap);

    if (!kiln_c99_gc_is_tenured_kind(kind) && heap->nursery.active &&
        kiln_c99_nursery_accepts_block(heap, payload + sizeof(kiln_gc_block))) {
        size_t total = sizeof(kiln_gc_block) + payload;
        total = (total + 15) & ~((size_t)15);
        if (heap->nursery.bump + total > heap->nursery.limit) kiln_c99_nursery_turn_chunk(heap);
        if (heap->nursery.active && heap->nursery.bump + total <= heap->nursery.limit) {
            block = (kiln_gc_block *)heap->nursery.bump;
            size_t offset = (size_t)(heap->nursery.bump - heap->nursery.base);
            size_t bit_idx = offset >> 4;
            heap->nursery.bitmap[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
            if (kiln_c99_gc_kind_finalized_at_alloc(kind)) {
                heap->nursery.finals[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
            }
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
        block = kiln_c99_gc_slab_take(heap, slab);
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
    if (heap->actor_candidate_id != 0 && !heap->actor_candidate_collected &&
        !kiln_c99_candidate_large_add(heap, block)) {
        kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        free(block);
        return NULL;
    }
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
        !kiln_c99_nursery_accepts_block(heap, sizeof(kiln_gc_block) + payload) ||
        nursery->bump + total > nursery->limit) {
        return NULL;
    }
    block = (kiln_gc_block *)nursery->bump;
    bit_idx = (size_t)(nursery->bump - nursery->base) >> 4;
    nursery->bitmap[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
    if (kiln_c99_gc_kind_finalized_at_alloc(kind)) {
        nursery->finals[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
    }
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

static void *kiln_c99_gc_alloc_tenured_ex(uint32_t kind, size_t size, int capture_candidate) {
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
        block = kiln_c99_gc_slab_take(heap, slab);
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
    if (capture_candidate && heap->actor_candidate_id != 0 && !heap->actor_candidate_collected &&
        !kiln_c99_candidate_large_add(heap, block)) {
        kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        free(block);
        return NULL;
    }
    if (heap->head != NULL) heap->head->prev = block;
    heap->head = block;
    ++heap->count;
    heap->bytes += size;
    heap->since_collection += size;
    return block + 1;
}

static void *kiln_c99_gc_alloc_tenured(uint32_t kind, size_t size) {
    return kiln_c99_gc_alloc_tenured_ex(kind, size, 1);
}

static void *kiln_c99_gc_calloc(uint32_t kind, size_t size) {
    void *payload = kiln_c99_gc_alloc(kind, size);
    if (payload != NULL && size != 0) memset(payload, 0, size);
    return payload;
}

/* ---- Ownership transfer: chunks built off the heap, adopted in one step ---------------------
 *
 * ADR 0070's primitive C moves a group of blocks into a heap by relinking them, never by copying.
 * Its second user, and the first built (realms issue 10 step 2), is ADR 0069's worker arenas: a
 * parallel call's workers allocate into private chunks of the nursery's own kind, and when the
 * call returns the realm thread makes every one of them the heap's at once.
 *
 * The unit moved is a *chunk*, not a block. A chunk of the nursery's kind already carries
 * everything the collector needs to know about its blocks -- the allocation bitmap, the
 * finaliser bitmap, the fill and the counts -- in its own header, and the heap finds it by
 * masking an address to the chunk size and asking its set of region bases. So adopting a chunk
 * is: enter its base in that set, widen the heap's address bounds over it, queue it on the
 * nursery's filled list (where the next collection resets it when nothing in it lives and
 * promotes it whole when something does, like any chunk the realm filled itself), and add its
 * counts to the heap's. Nothing is read or written per block: a million objects adopt in the time
 * of the handful of chunks that hold them.
 *
 *   kiln_c99_arena_chunk(capacity, words)   a chunk of the heap's kind for no heap yet, on any
 *                                           thread (it is `malloc`'s)
 *   kiln_c99_heap_adopt_chunks(heap, list)  on the heap's own realm thread, with no collection
 *                                           running: every sealed chunk of `list` is the heap's
 *                                           (1), or none is and the list is the caller's (0,
 *                                           the region set could not grow)
 *   kiln_c99_arena_chunks_free(list)        a list nothing will ever reference, returned
 *
 * What makes it sound is who can see a chunk before the step. Its builder alone: the chunk is in
 * no heap's set, so a collection of the heap -- which the builder's protocol must rule out anyway
 * while objects in the chunk point into the heap unmarked -- would neither find nor free it. The
 * parallel section's protocol is the paused state (`parallel_paused`): a collection due while an
 * arena is open aborts rather than run. A sealed chunk's `limit` is its fill and `alloc_count`/
 * `alloc_bytes` its blocks, exactly what `kiln_c99_nursery_seal` leaves for a chunk of the realm's
 * own, and every block in it has its header, its bitmap bit and, for a kind finalised from birth,
 * its finaliser bit (`kiln_c99_par_alloc`). */
static kiln_nursery_chunk *kiln_c99_arena_chunk(size_t capacity, size_t bitmap_words) {
    char *region = (char *)kiln_c99_gc_aligned_alloc(capacity, capacity);
    if (region == NULL) return NULL;
    return kiln_c99_chunk_lay_out(region, bitmap_words);
}

static void kiln_c99_arena_chunks_free(kiln_nursery_chunk *chunks) {
    while (chunks != NULL) {
        kiln_nursery_chunk *next = chunks->next;
        kiln_c99_candidate_group_remove(chunks);
        kiln_c99_gc_aligned_free(chunks);
        chunks = next;
    }
}

static void kiln_c99_candidate_record_chunk_layout(kiln_gc_heap *, const kiln_nursery_chunk *);

static int kiln_c99_heap_adopt_chunks(kiln_gc_heap *heap, kiln_nursery_chunk *chunks) {
    kiln_gc_nursery *nursery = &heap->nursery;
    kiln_nursery_chunk *chunk;
    size_t count = 0;
    for (chunk = chunks; chunk != NULL; chunk = chunk->next) count += 1;
    if (count == 0) return 1;
    /* Room for every base first, so no insert below can fail half way through the list. */
    if ((nursery->regions.count + count) * 2 > nursery->regions.capacity &&
        kiln_c99_gc_ptrset_grow(&nursery->regions, nursery->regions.count + count) != KILN_C99_OK) {
        return 0;
    }
    while (chunks != NULL) {
        char *region = (char *)chunks;
        chunk = chunks;
        chunks = chunk->next;
        (void)kiln_c99_gc_ptrset_insert(&nursery->regions, region);
        if (nursery->lo == NULL || region < nursery->lo) nursery->lo = region;
        if (nursery->hi == NULL || region + nursery->capacity > nursery->hi) {
            nursery->hi = region + nursery->capacity;
        }
        kiln_c99_gc_bound(heap, region, nursery->capacity);
        if (chunk->candidate_id != 0) chunk->candidate_epoch = heap->epoch;
        kiln_c99_nursery_full_push(nursery, chunk);
        nursery->full_bytes += (size_t)(chunk->limit - chunk->base);
        heap->count += chunk->alloc_count;
        heap->bytes += chunk->alloc_bytes;
        heap->since_collection += chunk->alloc_bytes;
    }
    /* All regions must be indexed before inspecting edges between adopted chunks. Workers
     * skip the realm's ledger; publication records their initialized fields on this thread. */
    if (heap->candidate_ledger_groups != 0) {
        chunk = nursery->full;
        for (size_t index = 0; index < count; ++index, chunk = chunk->next)
            kiln_c99_candidate_record_chunk_layout(heap, chunk);
    }
    return 1;
}

/* These object and array layouts have fully accounted finalizer actions. Inline and in-block
 * storage needs no free; a provenance-backed candidate array owns exactly one movable side
 * buffer and its receiver's finalizer frees it. Other side storage and realm-owned indexes
 * remain refused. Keep this narrower than the collector's supported layouts: a finalizer bit
 * permits arbitrary teardown for every other kind. */
static int kiln_c99_graph_inert_array_storage(const kiln_gc_block *block,
                                               const kiln_array *array) {
    const kiln_c99_candidate_array_elements *header;
    if (array->count > array->capacity) return 0;
    if (array->elements == NULL) return array->count == 0 && array->capacity == 0;
    if (array->candidate_elements != 0) {
        if (array->candidate_elements != 1 || array->capacity == 0 ||
            array->elements == array->inline_elements ||
            array->elements == (const kiln_val *)(array + 1) ||
            array->capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val)) return 0;
        header = kiln_c99_array_candidate_header(array);
        return header->owner == array && header->capacity == array->capacity &&
               header->candidate_id != 0 && header->generation != 0;
    }
    if (array->elements == array->inline_elements)
        return array->capacity <= sizeof array->inline_elements / sizeof array->inline_elements[0];
    if (array->elements == (const kiln_val *)(array + 1))
        return array->capacity <= (block->size - sizeof *array) / sizeof(kiln_val);
    return 0;
}

static int kiln_c99_graph_inert_finalizer_block(const kiln_gc_block *block) {
    size_t index;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_OBJECT: {
        const kiln_object *object = (const kiln_object *)(block + 1);
        const kiln_shape *shape;
        if (block->size < sizeof *object || object->sealed != NULL ||
            object->element_count > object->element_capacity ||
            (object->candidate_slots == 0 && object->slots == object->inline_slots &&
             object->slot_capacity != KILN_C99_INLINE_SLOTS) ||
            (object->candidate_slots == 0 && object->slots == NULL &&
             (object->slot_capacity != 0 || object->shape != NULL)) ||
            (object->candidate_slots == 0 && object->slots != NULL &&
             object->slots != object->inline_slots &&
             (object->slots != (const kiln_val *)(object + 1) ||
              object->slot_capacity <= KILN_C99_INLINE_SLOTS ||
              object->slot_capacity > (block->size - sizeof *object) / sizeof(kiln_val))) ||
            object->module_namespace || object->arguments_brand || object->error_brand ||
            object->layout_token != 0 || object->accessor_count != 0 ||
            object->read_only_seen || object->frozen || object->not_extensible ||
            object->primitive_wrapper_kind != KILN_C99_WRAPPER_NONE ||
            (object->prototype.tag != KILN_VALUE_UNDEFINED &&
             object->prototype.tag != KILN_VALUE_NULL)) return 0;
        if (object->elements != NULL || object->element_count != 0 ||
            object->element_capacity != 0 || object->candidate_elements != 0) {
            const kiln_c99_candidate_object_elements *header;
            if (object->candidate_elements != 1 || object->elements == NULL ||
                object->element_capacity == 0 ||
                object->element_capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val))
                return 0;
            header = kiln_c99_object_candidate_elements_header(object);
            if (header->owner != object || header->capacity != object->element_capacity ||
                header->candidate_id == 0 || header->generation == 0) return 0;
        }
        shape = object->shape;
        if (shape == NULL) {
            if (object->candidate_slots != 0 || object->count == 0 ||
                object->count > object->capacity ||
                (object->slots != object->inline_slots &&
                 (object->slots != NULL || object->slot_capacity != 0))) return 0;
            if (object->candidate_properties != 0) {
                const kiln_c99_candidate_object_properties *header;
                if (object->candidate_properties != 1 || object->properties == NULL ||
                    kiln_c99_object_properties_in_block(object) ||
                    object->capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_property))
                    return 0;
                header = kiln_c99_object_candidate_header(object);
                if (header->owner != object || header->capacity != object->capacity ||
                    header->candidate_id == 0 || header->generation == 0) return 0;
            } else if (!kiln_c99_object_properties_in_block(object) ||
                       object->capacity != object->count ||
                       object->count > (block->size - sizeof *object) / sizeof(kiln_property))
                return 0;
            for (index = 0; index < object->count; ++index) {
                const kiln_property *property = &object->properties[index];
                if (property->key == NULL || !property->enumerable ||
                    !property->writable || !property->configurable ||
                    property->symbol_hidden || property->accessor ||
                    property->refused) return 0;
            }
            return 1;
        }
        if (object->properties != NULL || object->count != 0 || object->capacity != 0 ||
            object->candidate_properties != 0) return 0;
        if (object->candidate_slots != 0) {
            const kiln_c99_candidate_object_slots *header;
            if (object->candidate_slots != 1 || object->slots == NULL ||
                object->slots == object->inline_slots ||
                object->slots == (const kiln_val *)(object + 1) ||
                object->slot_capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val))
                return 0;
            header = kiln_c99_object_candidate_slots_header(object);
            if (header->owner != object || header->capacity != object->slot_capacity ||
                header->candidate_id == 0 || header->generation == 0) return 0;
        }
        if ((shape->shared & 1u) == 0 || shape->property_count > object->slot_capacity ||
            shape->special_mask != 0 || shape->accessor_mask != 0 || shape->code_mask != 0)
            return 0;
        for (index = 0; index < shape->property_count; ++index) {
            if (!kiln_c99_key_is_shared(shape->keys[index])) return 0;
        }
        return 1;
    }
    case KILN_GC_ARRAY: {
        const kiln_array *array = (const kiln_array *)(block + 1);
        if (block->size < sizeof *array || !kiln_c99_graph_inert_array_storage(block, array) ||
            array->properties.tag != KILN_VALUE_UNDEFINED ||
            array->match_input.tag != KILN_VALUE_UNDEFINED ||
            array->match_groups.tag != KILN_VALUE_UNDEFINED || array->match_index != -1 ||
            array->frozen || array->not_extensible) return 0;
        return 1;
    }
    default:
        return 0;
    }
}

/* A candidate must still have its original contiguous bump layout. Promoted chunks can have
 * holes after a collection and need a different proof. Check the complete bitmap, rather than
 * trusting the chunk's counters or only the headers encountered by a forward walk. */
static int kiln_c99_chunk_sealed_for_move(const kiln_gc_nursery *nursery,
                                          const kiln_nursery_chunk *chunk) {
    const char *region = (const char *)chunk;
    const char *cursor;
    size_t bitmap_bytes;
    size_t count = 0;
    size_t bytes = 0;
    size_t bits = 0;
    size_t word;
    if (nursery->bitmap_words > (SIZE_MAX - 15) / sizeof(uint64_t)) return 0;
    bitmap_bytes = (nursery->bitmap_words * sizeof(uint64_t) + 15) & ~(size_t)15;
    if (KILN_NURSERY_HEADER > nursery->capacity ||
        bitmap_bytes > (nursery->capacity - KILN_NURSERY_HEADER) / 3 ||
        chunk->bitmap_words != nursery->bitmap_words ||
        chunk->bitmap != (const uint64_t *)(region + KILN_NURSERY_HEADER) ||
        chunk->marks != (const uint64_t *)(region + KILN_NURSERY_HEADER + bitmap_bytes) ||
        chunk->finals != (const uint64_t *)(region + KILN_NURSERY_HEADER + 2 * bitmap_bytes) ||
        chunk->base != region + KILN_NURSERY_HEADER + 3 * bitmap_bytes ||
        chunk->limit < chunk->base || chunk->limit > region + nursery->capacity) return 0;
    for (word = 0; word < nursery->bitmap_words; ++word) {
        if ((chunk->finals[word] & ~chunk->bitmap[word]) != 0) return 0;
        bits += (size_t)__builtin_popcountll(chunk->bitmap[word]);
    }
    for (cursor = chunk->base; cursor < chunk->limit;) {
        const kiln_gc_block *block;
        size_t offset = (size_t)(cursor - chunk->base);
        size_t bit = offset >> 4;
        size_t remaining = (size_t)(chunk->limit - cursor);
        size_t payload;
        size_t total;
        if ((offset & 15) != 0 || bit >> 6 >= nursery->bitmap_words ||
            (chunk->bitmap[bit >> 6] & (UINT64_C(1) << (bit & 63))) == 0 ||
            remaining < sizeof(kiln_gc_block)) return 0;
        block = (const kiln_gc_block *)cursor;
        if ((block->kind & (KILN_GC_FLAG_NURSERY | KILN_GC_FLAG_DEAD)) !=
                KILN_GC_FLAG_NURSERY) return 0;
        payload = block->size == 0 ? 1 : block->size;
        if (payload > remaining - sizeof(kiln_gc_block) || bytes > SIZE_MAX - block->size)
            return 0;
        total = (sizeof(kiln_gc_block) + payload + 15) & ~(size_t)15;
        if (total > remaining) return 0;
        {
            int final_bit = (chunk->finals[bit >> 6] & (UINT64_C(1) << (bit & 63))) != 0;
            uint32_t kind = block->kind & KILN_GC_KIND_MASK;
            if (final_bit && !kiln_c99_graph_inert_finalizer_block(block)) return 0;
            if (!final_bit && (kind == KILN_GC_OBJECT || kind == KILN_GC_ARRAY)) return 0;
        }
        cursor += total;
        bytes += block->size;
        count += 1;
    }
    return count != 0 && count == bits && count == chunk->alloc_count &&
           bytes == chunk->alloc_bytes;
}

static const kiln_symbol *kiln_c99_gc_symbol_for_key(const kiln_gc_heap *heap,
                                                      const kiln_string *key);

typedef struct kiln_c99_graph_walk {
    const kiln_gc_heap *heap;
    kiln_gc_ptrset regions;
    kiln_gc_ptrset reached;
    kiln_c99_graph_side_set sides;
    kiln_gc_block **queue;
    size_t queued;
    size_t capacity;
} kiln_c99_graph_walk;

/* Candidate chunks were checked as complete bump allocations before this walk. Their bitmap
 * already names every valid block start, so a second hash table of every block is unnecessary.
 * Large inline strings belong only when they are in this factory's separately checked list. */
static int kiln_c99_graph_is_member(const kiln_c99_graph_walk *walk,
                                     const kiln_gc_block *block) {
    const kiln_nursery_chunk *chunk;
    uintptr_t address;
    if (block == NULL) return 0;
    if ((block->kind & KILN_GC_FLAG_LARGE) != 0) {
        return kiln_c99_gc_ptrset_contains(&walk->sides.large, block);
    }
    chunk = kiln_c99_nursery_chunk_of(walk->heap, block);
    if (chunk != NULL && kiln_c99_gc_ptrset_contains(&walk->regions, chunk)) {
        size_t bit;
        address = (uintptr_t)block;
        if (address < (uintptr_t)chunk->base || address >= (uintptr_t)chunk->limit ||
            ((address - (uintptr_t)chunk->base) & 15) != 0) return 0;
        bit = (size_t)((address - (uintptr_t)chunk->base) >> 4);
        return bit >> 6 < chunk->bitmap_words &&
               (chunk->bitmap[bit >> 6] & (UINT64_C(1) << (bit & 63))) != 0;
    }
    return 0;
}

static int kiln_c99_graph_append_span(kiln_c99_graph_walk *walk, uintptr_t base,
                                      size_t bytes, const void *owner) {
    kiln_c99_graph_side_span *grown;
    if (bytes == 0 || base > UINTPTR_MAX - bytes) return 0;
    if (walk->sides.count == walk->sides.capacity) {
        size_t capacity = walk->sides.capacity == 0 ? 4 : walk->sides.capacity * 2;
        if (capacity < walk->sides.capacity ||
            capacity > SIZE_MAX / sizeof *grown) return 0;
        grown = (kiln_c99_graph_side_span *)realloc(
            walk->sides.spans, capacity * sizeof *grown);
        if (grown == NULL) return 0;
        walk->sides.spans = grown;
        walk->sides.capacity = capacity;
    }
    walk->sides.spans[walk->sides.count++] =
        (kiln_c99_graph_side_span){ base, base + bytes, owner };
    if (walk->sides.count == 1 || base < walk->sides.lo) walk->sides.lo = base;
    if (base + bytes > walk->sides.hi) walk->sides.hi = base + bytes;
    return 1;
}

static int kiln_c99_graph_add_side(kiln_c99_graph_walk *walk, uintptr_t base,
                                    size_t bytes, const void *owner) {
    if (bytes == 0 || base > UINTPTR_MAX - bytes ||
        kiln_c99_gc_lookup_interior(walk->heap, base) != NULL ||
        kiln_c99_gc_lookup_interior(walk->heap, base + bytes - 1) != NULL) return 0;
    return kiln_c99_graph_append_span(walk, base, bytes, owner);
}

static uint64_t kiln_c99_graph_block_candidate_id(const kiln_c99_graph_walk *walk,
                                                  const kiln_gc_block *block) {
    if ((block->kind & KILN_GC_FLAG_LARGE) != 0) {
        const kiln_c99_candidate_large *owner = kiln_c99_candidate_large_find(walk->heap, block);
        return owner == NULL ? 0 : owner->group->id;
    }
    const kiln_nursery_chunk *chunk = kiln_c99_nursery_chunk_of(walk->heap, block);
    return chunk == NULL ? 0 : chunk->candidate_id;
}

static int kiln_c99_graph_add_array_side(kiln_c99_graph_walk *walk,
                                         const kiln_gc_block *block, const kiln_array *array) {
    const kiln_c99_candidate_array_elements *header = kiln_c99_array_candidate_header(array);
    uint64_t candidate_id = kiln_c99_graph_block_candidate_id(walk, block);
    if (candidate_id == 0 || header->candidate_id != candidate_id ||
        header->owner != array || header->capacity != array->capacity ||
        header->generation == 0 ||
        array->capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val)) return 0;
    return kiln_c99_graph_add_side(walk, (uintptr_t)header,
        sizeof *header + array->capacity * sizeof(kiln_val), array);
}

static int kiln_c99_graph_add_object_side(kiln_c99_graph_walk *walk,
                                          const kiln_gc_block *block,
                                          const kiln_object *object) {
    const kiln_c99_candidate_object_properties *header =
        kiln_c99_object_candidate_header(object);
    uint64_t candidate_id = kiln_c99_graph_block_candidate_id(walk, block);
    if (candidate_id == 0 || header->candidate_id != candidate_id ||
        header->owner != object || header->capacity != object->capacity ||
        header->generation == 0 ||
        object->capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_property)) return 0;
    return kiln_c99_graph_add_side(walk, (uintptr_t)header,
        sizeof *header + object->capacity * sizeof(kiln_property), object);
}

static int kiln_c99_graph_add_object_slots_side(kiln_c99_graph_walk *walk,
                                                const kiln_gc_block *block,
                                                const kiln_object *object) {
    const kiln_c99_candidate_object_slots *header =
        kiln_c99_object_candidate_slots_header(object);
    uint64_t candidate_id = kiln_c99_graph_block_candidate_id(walk, block);
    if (candidate_id == 0 || header->candidate_id != candidate_id ||
        header->owner != object || header->capacity != object->slot_capacity ||
        header->generation == 0 ||
        object->slot_capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val)) return 0;
    return kiln_c99_graph_add_side(walk, (uintptr_t)header,
        sizeof *header + object->slot_capacity * sizeof(kiln_val), object);
}

static int kiln_c99_graph_add_object_elements_side(kiln_c99_graph_walk *walk,
                                                   const kiln_gc_block *block,
                                                   const kiln_object *object) {
    const kiln_c99_candidate_object_elements *header =
        kiln_c99_object_candidate_elements_header(object);
    uint64_t candidate_id = kiln_c99_graph_block_candidate_id(walk, block);
    if (candidate_id == 0 || header->candidate_id != candidate_id ||
        header->owner != object || header->capacity != object->element_capacity ||
        header->generation == 0 ||
        object->element_capacity > (SIZE_MAX - sizeof *header) / sizeof(kiln_val)) return 0;
    return kiln_c99_graph_add_side(walk, (uintptr_t)header,
        sizeof *header + object->element_capacity * sizeof(kiln_val), object);
}

static int kiln_c99_graph_side_span_compare(const void *left, const void *right) {
    uintptr_t a = ((const kiln_c99_graph_side_span *)left)->base;
    uintptr_t b = ((const kiln_c99_graph_side_span *)right)->base;
    return a < b ? -1 : a > b ? 1 : 0;
}

/* Every reference must name the start of a block in the same candidate. A static or process-wide
 * pointer is refused too: this narrow walker has no certificate that it is shared immutable
 * storage, and a raw pointer outside the sender heap is not proof of that fact. */
static int kiln_c99_graph_follow(kiln_c99_graph_walk *walk, const void *payload) {
    kiln_gc_block *block = kiln_c99_gc_lookup(walk->heap, payload);
    if (block == NULL || (const void *)(block + 1) != payload ||
        !kiln_c99_graph_is_member(walk, block)) return 0;
    if (kiln_c99_gc_ptrset_contains(&walk->reached, block)) return 1;
    if (walk->queued == walk->capacity ||
        kiln_c99_gc_ptrset_insert(&walk->reached, block) != KILN_C99_OK) return 0;
    walk->queue[walk->queued++] = block;
    return 1;
}

static int kiln_c99_graph_pointer_names_member(const kiln_c99_graph_walk *walk,
                                                const void *pointer) {
    const kiln_gc_block *block = kiln_c99_gc_lookup_interior(walk->heap, (uintptr_t)pointer);
    return kiln_c99_graph_is_member(walk, block);
}

static int kiln_c99_graph_follow_val(kiln_c99_graph_walk *walk, kiln_val value) {
    const void *payload;
    const kiln_gc_block *block;
    uint32_t kind;
    if (KILN_VAL_IS_NUMBER(value) || KILN_VAL_IS_IMMEDIATE(value) ||
        KILN_VAL_IS_BUILTIN(value) || KILN_VAL_IS_METHOD(value)) return 1;
    if ((value & KILN_TAG_MASK) != KILN_TAG_STRING_PREFIX &&
        (value & KILN_TAG_MASK) != KILN_TAG_CELL_PREFIX &&
        (value & KILN_TAG_MASK) != KILN_TAG_OBJECT_PREFIX &&
        (value & KILN_TAG_MASK) != KILN_TAG_BUFFER_PREFIX) return 0;
    payload = KILN_VAL_PTR(value);
    block = kiln_c99_gc_lookup(walk->heap, payload);
    if (block == NULL || (const void *)(block + 1) != payload) return 0;
    kind = block->kind & KILN_GC_KIND_MASK;
    if ((value & KILN_TAG_MASK) == KILN_TAG_STRING_PREFIX) {
        if (kind != KILN_GC_STRING && kind != KILN_GC_SLICED_STRING) return 0;
    } else {
        switch (value & KILN_TAG_MASK) {
        case KILN_TAG_CELL_PREFIX:
            if (kind != KILN_GC_CELL) return 0;
            break;
        case KILN_TAG_OBJECT_PREFIX:
            if (kind != KILN_GC_OBJECT && kind != KILN_GC_DATE) return 0;
            break;
        case KILN_TAG_BUFFER_PREFIX:
            if (kind != KILN_GC_ARRAY) return 0;
            break;
        default:
            return 0;
        }
    }
    return kiln_c99_graph_follow(walk, payload);
}

static int kiln_c99_graph_inline_string(const kiln_gc_block *block) {
    const kiln_string *string = (const kiln_string *)(block + 1);
    size_t width;
    if (block->size < sizeof *string || (string->flags & KILN_STRING_SLICED) != 0 ||
        (string->flags & ~(KILN_STRING_UTF16 | KILN_STRING_SYMBOL_KEY |
                           KILN_STRING_PRIVATE_KEY)) != 0 ||
        string->data != (const void *)(string + 1)) return 0;
    width = (string->flags & KILN_STRING_UTF16) != 0 ? 2 : 1;
    return string->unit_len <= (block->size - sizeof *string) / width;
}

/* This deliberately supports only layouts whose every edge and owner allocation is understood.
 * It is independent of the collector's tracer: the tracer may silently ignore an external
 * pointer, whereas an ownership proof must reject it. New GC kinds are refused by default. */
static int kiln_c99_graph_walk_block(kiln_c99_graph_walk *walk, kiln_gc_block *block) {
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_STRING:
        return kiln_c99_graph_inline_string(block);
    case KILN_GC_BYTES:
        return 1;
    case KILN_GC_SLICED_STRING: {
        const kiln_sliced_string *string;
        const kiln_gc_block *parent_block;
        const kiln_string *parent;
        uintptr_t start;
        uintptr_t data;
        size_t width;
        size_t parent_bytes;
        size_t slice_bytes;
        if (block->size < sizeof(kiln_sliced_string)) return 0;
        string = (const kiln_sliced_string *)(block + 1);
        if (string->parent == NULL ||
            (string->base.flags & KILN_STRING_SLICED) == 0) return 0;
        parent_block = kiln_c99_gc_lookup(walk->heap, string->parent);
        if (parent_block == NULL ||
            (parent_block->kind & KILN_GC_KIND_MASK) != KILN_GC_STRING ||
            !kiln_c99_graph_inline_string(parent_block)) return 0;
        parent = string->parent;
        if ((string->base.flags & KILN_STRING_UTF16) !=
            (parent->flags & KILN_STRING_UTF16)) return 0;
        width = (parent->flags & KILN_STRING_UTF16) != 0 ? 2 : 1;
        parent_bytes = (size_t)parent->unit_len * width;
        slice_bytes = (size_t)string->base.unit_len * width;
        start = (uintptr_t)string->base.data;
        data = (uintptr_t)parent->data;
        if (start < data || start - data > parent_bytes ||
            (start - data) % width != 0 || slice_bytes > parent_bytes - (start - data)) return 0;
        return kiln_c99_graph_follow(walk, string->parent);
    }
    case KILN_GC_CELL:
        if (block->size < sizeof(kiln_val)) return 0;
        return kiln_c99_graph_follow_val(walk, *(const kiln_val *)(block + 1));
    case KILN_GC_DATE: {
        const kiln_date *date = (const kiln_date *)(block + 1);
        return block->size == sizeof *date &&
               kiln_c99_graph_follow_val(walk, kiln_val_from_boundary(date->properties));
    }
    case KILN_GC_OBJECT: {
        const kiln_object *object = (const kiln_object *)(block + 1);
        size_t index;
        if (!kiln_c99_graph_inert_finalizer_block(block)) return 0;
        if (object->candidate_elements != 0 &&
            !kiln_c99_graph_add_object_elements_side(walk, block, object)) return 0;
        for (index = 0; index < object->element_count; ++index) {
            if (object->elements[index] == KILN_VAL_HOLE) continue;
            if (!kiln_c99_graph_follow_val(walk, object->elements[index])) return 0;
        }
        if (object->shape == NULL) {
            if (object->count != 0) walk->sides.foreign_dictionary_keys = 1;
            if (object->candidate_properties != 0 &&
                !kiln_c99_graph_add_object_side(walk, block, object)) return 0;
            for (index = 0; index < object->count; ++index) {
                const kiln_property *property = &object->properties[index];
                const kiln_symbol *key_symbol =
                    kiln_c99_gc_symbol_for_key(walk->heap, property->key);
                if ((!kiln_c99_key_is_shared(property->key) &&
                     !kiln_c99_graph_follow(walk, property->key)) ||
                    (key_symbol != NULL && !kiln_c99_graph_follow(walk, key_symbol)) ||
                    !kiln_c99_graph_follow_val(walk,
                                               kiln_val_from_boundary(property->value))) return 0;
            }
            return 1;
        }
        if (kiln_c99_graph_pointer_names_member(walk, object->shape)) return 0;
        if (object->candidate_slots != 0 &&
            !kiln_c99_graph_add_object_slots_side(walk, block, object)) return 0;
        for (index = 0; index < object->shape->property_count; ++index) {
            const kiln_symbol *key_symbol =
                kiln_c99_gc_symbol_for_key(walk->heap, object->shape->keys[index]);
            if (kiln_c99_graph_pointer_names_member(walk, object->shape->keys[index])) return 0;
            if (key_symbol != NULL && !kiln_c99_graph_follow(walk, key_symbol)) return 0;
            if (!kiln_c99_graph_follow_val(walk, object->slots[index])) return 0;
        }
        return 1;
    }
    case KILN_GC_ARRAY: {
        const kiln_array *array = (const kiln_array *)(block + 1);
        size_t index;
        if (!kiln_c99_graph_inert_finalizer_block(block)) return 0;
        if (array->candidate_elements != 0 &&
            !kiln_c99_graph_add_array_side(walk, block, array)) return 0;
        for (index = 0; index < array->count; ++index) {
            /* An absent array index carries no graph edge. Keep other tagged values subject
             * to the exact outgoing-ownership check below. */
            if (array->elements[index] == KILN_VAL_HOLE) continue;
            if (!kiln_c99_graph_follow_val(walk, array->elements[index])) return 0;
        }
        return 1;
    }
    default:
        return 0;
    }
}

/* Prove the candidate's exact *outgoing* closure without changing GC marks. Every block is
 * validated through its entire edge and side-storage layout, including unreachable factory
 * temporaries that travel as garbage. Root-reachable blocks are walked first; then every
 * remaining block is an additional walk root. Incoming C/provider/machine/outside scans still
 * cover the entire chunk. This proof alone never grants permission to move. */
static int kiln_c99_graph_candidate_closed_with_sides(
    const kiln_gc_heap *heap, kiln_nursery_chunk *const *chunks, size_t count,
    const void *root, kiln_c99_graph_side_set *sides, const kiln_gc_block *large_head) {
    kiln_c99_graph_walk walk = {0};
    size_t index;
    size_t seen = 0;
    size_t rooted_count;
    uint64_t id = 0;
    const kiln_c99_candidate_group *group = NULL;
    int result = 0;
    if (heap == NULL || chunks == NULL || root == NULL || heap->collecting) return 0;
    if (count == 0) {
        kiln_gc_block *root_block = kiln_c99_gc_lookup(heap, root);
        kiln_c99_candidate_large *owner;
        if (root_block == NULL ||
            (owner = kiln_c99_candidate_large_find(heap, root_block)) == NULL ||
            owner->epoch != heap->epoch || owner->group->count != 0) return 0;
        group = owner->group;
        id = group->id;
    }
    walk.heap = heap;
    if (kiln_c99_gc_ptrset_grow(&walk.regions, count) != KILN_C99_OK) goto done;
    for (index = 0; index < count; ++index) {
        const kiln_nursery_chunk *chunk = chunks[index];
        int found = 0;
        if (chunk == NULL || chunk->candidate_id == 0 || chunk->candidate_epoch != heap->epoch ||
            (id != 0 && chunk->candidate_id != id) ||
            !kiln_c99_gc_ptrset_contains(&heap->nursery.regions, chunk) ||
            !kiln_c99_chunk_sealed_for_move(&heap->nursery, chunk)) goto done;
        id = chunk->candidate_id;
        if (group == NULL) group = chunk->candidate_group;
        if (!kiln_c99_candidate_group_contains(group, chunk) || group->count != count ||
            kiln_c99_gc_ptrset_contains(&walk.regions, chunk) ||
            kiln_c99_gc_ptrset_insert(&walk.regions, chunk) != KILN_C99_OK) goto done;
        found = kiln_c99_nursery_full_contains(&heap->nursery, chunk);
        if (!found || seen > SIZE_MAX - chunk->alloc_count) goto done;
        seen += chunk->alloc_count;
    }
    {
        const kiln_c99_candidate_large *entry = group->large_head;
        const kiln_gc_block *private_large = large_head;
        size_t large_count = group->large_count;
        size_t large_index = 0;
        if (entry != NULL && private_large != NULL) goto done;
        for (const kiln_gc_block *it = private_large; it != NULL; it = it->next) {
            if (large_count == SIZE_MAX) goto done;
            ++large_count;
        }
        if (large_count > SIZE_MAX / sizeof *walk.sides.large_blocks ||
            large_count > SIZE_MAX - seen) goto done;
        if (large_count != 0) {
            walk.sides.large_blocks = (kiln_gc_block **)malloc(
                large_count * sizeof *walk.sides.large_blocks);
            if (walk.sides.large_blocks == NULL ||
                kiln_c99_gc_ptrset_grow(&walk.sides.large, large_count) != KILN_C99_OK)
                goto done;
        }
        while (entry != NULL || private_large != NULL) {
            const kiln_gc_block *large = entry != NULL ? entry->block : private_large;
            if (large_index == large_count ||
                (large->kind & KILN_GC_FLAG_LARGE) == 0 ||
                !kiln_c99_gc_ptrset_contains(&heap->large_table, large) ||
                (entry == NULL && ((large->kind & KILN_GC_KIND_MASK) != KILN_GC_STRING ||
                                    !kiln_c99_graph_inline_string(large))) ||
                (entry != NULL && (entry->group != group || entry->epoch != heap->epoch ||
                    kiln_c99_candidate_large_find(heap, large) != entry)) ||
                kiln_c99_gc_ptrset_contains(&walk.sides.large, large) ||
                kiln_c99_gc_ptrset_insert(&walk.sides.large, large) != KILN_C99_OK ||
                large->size > SIZE_MAX - sizeof *large ||
                !kiln_c99_graph_append_span(&walk, (uintptr_t)large,
                                           sizeof *large + large->size, large)) goto done;
            walk.sides.large_blocks[large_index++] = (kiln_gc_block *)large;
            if (entry != NULL) entry = entry->next;
            else private_large = private_large->next;
        }
        if (large_index != large_count) goto done;
        walk.sides.large_count = large_count;
        seen += large_count;
    }
    if (seen > SIZE_MAX / sizeof *walk.queue || seen > SIZE_MAX / 2 ||
        kiln_c99_gc_ptrset_grow(&walk.reached, seen) != KILN_C99_OK) goto done;
    walk.queue = (kiln_gc_block **)malloc(seen * sizeof *walk.queue);
    if (walk.queue == NULL) goto done;
    walk.capacity = seen;
    if (!kiln_c99_graph_follow(&walk, root)) goto done;
    for (index = 0; index < walk.queued; ++index) {
        if (!kiln_c99_graph_walk_block(&walk, walk.queue[index])) goto done;
    }
    rooted_count = walk.queued;
    for (index = 0; index < count; ++index) {
        const kiln_nursery_chunk *chunk = chunks[index];
        const char *cursor;
        for (cursor = chunk->base; cursor < chunk->limit;) {
            const kiln_gc_block *block = (const kiln_gc_block *)cursor;
            size_t payload = block->size == 0 ? 1 : block->size;
            if (!kiln_c99_gc_ptrset_contains(&walk.reached, block)) {
                if (!kiln_c99_graph_follow(&walk, block + 1)) goto done;
            }
            cursor += (sizeof(kiln_gc_block) + payload + 15) & ~(size_t)15;
        }
    }
    for (index = 0; index < walk.sides.large_count; ++index) {
        kiln_gc_block *large = walk.sides.large_blocks[index];
        if (!kiln_c99_gc_ptrset_contains(&walk.reached, large) &&
            !kiln_c99_graph_follow(&walk, large + 1)) goto done;
    }
    for (index = rooted_count; index < walk.queued; ++index) {
        if (!kiln_c99_graph_walk_block(&walk, walk.queue[index])) goto done;
    }
    for (index = 0; index < walk.sides.large_count; ++index)
        if (!kiln_c99_gc_ptrset_contains(&walk.reached, walk.sides.large_blocks[index])) goto done;
    if (walk.queued != seen) goto done;
    /* A valid malloc allocation cannot overlap another live allocation. Check the complete
     * certificate once, then retain this order for binary searches in the incoming proof. */
    if (walk.sides.count > 1)
        qsort(walk.sides.spans, walk.sides.count, sizeof *walk.sides.spans,
              kiln_c99_graph_side_span_compare);
    for (index = 1; index < walk.sides.count; ++index) {
        if (walk.sides.spans[index - 1].limit > walk.sides.spans[index].base) goto done;
    }
    result = 1;
done:
    if (result && sides != NULL) {
        *sides = walk.sides;
        sides->regions = walk.regions;
        sides->region_source = chunks;
        memset(&walk.sides, 0, sizeof walk.sides);
        memset(&walk.regions, 0, sizeof walk.regions);
    }
    kiln_c99_graph_side_set_dispose(&walk.sides);
    free(walk.queue);
    free(walk.regions.table);
    free(walk.reached.table);
    return result;
}

static int kiln_c99_graph_candidate_closed(const kiln_gc_heap *heap,
                                           kiln_nursery_chunk *const *chunks, size_t count,
                                           const void *root) {
    return kiln_c99_graph_candidate_closed_with_sides(heap, chunks, count, root, NULL, NULL);
}

typedef enum kiln_c99_graph_incoming_result {
    KILN_C99_GRAPH_OUTGOING_UNPROVEN = 0,
    KILN_C99_GRAPH_C_ROOT_ALIAS = 1,
    KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS = 2,
    KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED = 3,
    KILN_C99_GRAPH_ROOTS_UNENUMERATED = 4,
    KILN_C99_GRAPH_PROVIDER_ROOT_ALIAS = 5,
    KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED = 6,
    KILN_C99_GRAPH_MACHINE_ALIAS = 7,
    KILN_C99_GRAPH_MACHINE_BOUNDARY_UNAVAILABLE = 8,
    KILN_C99_GRAPH_CLEAR = 9
} kiln_c99_graph_incoming_result;

/* A root can hold an interior address, including a string's inline bytes. The collector accepts
 * those from machine registers, so an incoming check must not accept only exact payload starts. */
static int kiln_c99_graph_in_batch(const kiln_gc_heap *heap,
                                   kiln_nursery_chunk *const *chunks, size_t count,
                                   const void *pointer) {
    uintptr_t address = (uintptr_t)pointer;
    const kiln_c99_graph_side_set *sides = heap->graph_proof_sides;
    if (heap->graph_pointer_observer != NULL) {
        heap->graph_pointer_observer(heap->graph_pointer_observer_context, pointer);
        /* Never short-circuit an OR of fields while observing an owner's full layout. */
        return 0;
    }
    kiln_gc_block *block;
    kiln_nursery_chunk *chunk;
    size_t index;
    int indexed = sides != NULL && sides->region_source == chunks &&
                  sides->regions.count == count;
    if (sides != NULL && sides->count != 0 &&
        address >= sides->lo && address < sides->hi) {
        size_t low = 0, high = sides->count;
        while (low < high) {
            size_t middle = low + (high - low) / 2;
            if (sides->spans[middle].base <= address) low = middle + 1;
            else high = middle;
        }
        if (low != 0 && address < sides->spans[low - 1].limit) return 1;
    }
    if (address < heap->lo || address >= heap->hi) return 0;
    if (indexed) {
        /* Masking does not dereference this untrusted machine word. Most outside edges do
         * not enter a candidate region; reject them before the collector's interior lookup. */
        chunk = kiln_c99_nursery_chunk_of(heap, (const kiln_gc_block *)pointer);
        if (!kiln_c99_gc_ptrset_contains(&sides->regions, chunk)) return 0;
    }
    block = kiln_c99_gc_lookup_interior(heap, address);
    if (block == NULL || (block->kind & KILN_GC_FLAG_NURSERY) == 0) return 0;
    chunk = kiln_c99_nursery_chunk_of(heap, block);
    if (indexed) return kiln_c99_gc_ptrset_contains(&sides->regions, chunk);
    for (index = 0; index < count; ++index) {
        if (chunks[index] == chunk) return 1;
    }
    return 0;
}

/* Classify a caller-supplied snapshot of machine words without touching the collector's marks.
 * A word can be a raw/interior pointer or a tagged kiln_val, just as in the mid-turn GC scan.
 * This deliberately does not select a stack boundary or capture registers. A clear span is not
 * an ownership certificate: the actor safepoint must prove that it included all older frames
 * and callee-saved registers while excluding only the proof's own temporary candidate values. */
static int kiln_c99_graph_machine_words_alias(const kiln_gc_heap *heap,
                                               kiln_nursery_chunk *const *chunks, size_t count,
                                               const uintptr_t *words, size_t word_count) {
    size_t index;
    for (index = 0; index < word_count; ++index) {
        uintptr_t word = words[index];
        if (kiln_c99_graph_in_batch(heap, chunks, count, (const void *)word)) return 1;
        if ((word & KILN_TAG_MASK) >= KILN_TAG_STRING_PREFIX &&
            kiln_c99_graph_in_batch(heap, chunks, count,
                                    (const void *)(word & KILN_PAYLOAD_MASK))) return 1;
    }
    return 0;
}

static int kiln_c99_graph_val_in_batch(const kiln_gc_heap *heap,
                                       kiln_nursery_chunk *const *chunks, size_t count,
                                       kiln_val value) {
    if (KILN_VAL_IS_NUMBER(value) || value < KILN_TAG_STRING_PREFIX) return 0;
    return kiln_c99_graph_in_batch(heap, chunks, count, KILN_VAL_PTR(value));
}

static int kiln_c99_graph_value_in_batch(const kiln_gc_heap *heap,
                                         kiln_nursery_chunk *const *chunks, size_t count,
                                         const kiln_value *value) {
    if (value->tag == KILN_VALUE_SYMBOL)
        return kiln_c99_graph_in_batch(heap, chunks, count,
                                       kiln_c99_gc_value_pointer(*value));
    return value->tag >= KILN_VALUE_STRING &&
           kiln_c99_graph_in_batch(heap, chunks, count, (const void *)(uintptr_t)value->bits);
}

static int kiln_c99_graph_symbol_key_in_batch(const kiln_gc_heap *heap,
                                               kiln_nursery_chunk *const *chunks,
                                               size_t count, const kiln_string *key) {
    const kiln_symbol *symbol = kiln_c99_gc_symbol_for_key(heap, key);
    return symbol != NULL && kiln_c99_graph_in_batch(heap, chunks, count, symbol);
}

/* These are storage primitives for a future actor transaction, not transfer admission.
 * Only a turn-safepoint coordinator may use them: no JS, GC or collection mutation may run
 * between begin and rollback. Keep the original weak-map entry allocated, moving only its
 * value into one explicit realm root. Restoring it then needs no allocator and cannot fail.
 * There is deliberately no commit operation until the receiver publication protocol exists. */
static int kiln_c99_actor_isolated_escrow_begin(kiln_gc_heap *heap, kiln_value handle) {
    kiln_c99_realm_state *realm;
    kiln_collection *registry;
    kiln_entry *entry;
    double at;
    if (heap == NULL || handle.tag != KILN_VALUE_OBJECT) return 0;
    realm = &heap->realm;
    if (realm->isolated_escrow_entry != NULL ||
        realm->isolated_graphs.tag != KILN_VALUE_COLLECTION) return 0;
    registry = kiln_c99_collection_of(realm->isolated_graphs);
    at = kiln_c99_collection_find(registry, handle);
    if (at < 0.0 || at >= (double)registry->count) return 0;
    entry = &registry->entries[(size_t)at];
    if (entry->dead) return 0;
    realm->isolated_escrow_key = entry->key;
    realm->isolated_escrow_value = entry->value;
    realm->isolated_escrow_entry = entry;
    entry->value = kiln_c99_undefined();
    return 1;
}

static int kiln_c99_actor_isolated_escrow_rollback(kiln_gc_heap *heap) {
    kiln_c99_realm_state *realm;
    if (heap == NULL) return 0;
    realm = &heap->realm;
    if (realm->isolated_escrow_entry == NULL) return 0;
    realm->isolated_escrow_entry->value = realm->isolated_escrow_value;
    realm->isolated_escrow_entry = NULL;
    realm->isolated_escrow_key = kiln_c99_undefined();
    realm->isolated_escrow_value = kiln_c99_undefined();
    return 1;
}

/* Queue one handle by realm-local opaque ID, without ever placing the graph pointer in a
 * scheduler request. Resolving the ID at a sender turn boundary rechecks the live weak-key
 * entry: collection, cancellation, or replacement can only make escrow refuse. No caller may
 * treat this request as transfer admission until a real scheduler safepoint owns both realms. */
static int kiln_c99_actor_isolated_request_begin(kiln_gc_heap *heap, kiln_value handle,
                                                  uint64_t *out_id) {
    kiln_c99_realm_state *realm;
    kiln_collection *registry;
    double at;
    int prepared;
    if (heap == NULL || out_id == NULL || handle.tag != KILN_VALUE_OBJECT) return 0;
    realm = &heap->realm;
    if (realm->isolated_request_id != 0 || realm->isolated_escrow_entry != NULL ||
        realm->isolated_request_next_id == UINT64_MAX) return 0;
    prepared = kiln_c99_prepared_graph_has(heap, handle);
    if (!prepared) {
        if (realm->isolated_graphs.tag != KILN_VALUE_COLLECTION) return 0;
        registry = kiln_c99_collection_of(realm->isolated_graphs);
        at = kiln_c99_collection_find(registry, handle);
        if (at < 0.0 || at >= (double)registry->count ||
            registry->entries[(size_t)at].dead) return 0;
    }
    realm->isolated_request_id = ++realm->isolated_request_next_id;
    realm->isolated_request_handle = handle;
    realm->isolated_request_invalidated = 0;
    *out_id = realm->isolated_request_id;
    return 1;
}

static int kiln_c99_actor_isolated_request_escrow(kiln_gc_heap *heap, uint64_t id) {
    kiln_c99_realm_state *realm;
    if (heap == NULL || id == 0) return 0;
    realm = &heap->realm;
    if (realm->isolated_request_id != id || realm->isolated_request_invalidated ||
        realm->isolated_escrow_entry != NULL) return 0;
    return kiln_c99_actor_isolated_escrow_begin(heap, realm->isolated_request_handle);
}

/* Refusal restores the graph before releasing the request's handle root. Nothing is allocated
 * in this path, and a stale or mismatched ID cannot cancel a newer request. */
static int kiln_c99_actor_isolated_request_rollback(kiln_gc_heap *heap, uint64_t id) {
    kiln_c99_realm_state *realm;
    if (heap == NULL || id == 0) return 0;
    realm = &heap->realm;
    if (realm->isolated_request_id != id) return 0;
    if (realm->isolated_escrow_entry != NULL &&
        !kiln_c99_actor_isolated_escrow_rollback(heap)) return 0;
    realm->isolated_request_handle = kiln_c99_undefined();
    realm->isolated_request_id = 0;
    realm->isolated_request_invalidated = 0;
    return 1;
}

/* Called only after receiver adoption has published the native inbox root while both realms
 * remain stopped. No error may return from this point: an unlinked capsule cannot be restored
 * to a running sender. Delete exactly the entry that escrow exchanged, without allocation. */
static void kiln_c99_actor_isolated_request_commit_published(kiln_gc_heap *heap, uint64_t id,
                                                               kiln_value published_root) {
    kiln_c99_realm_state *realm;
    kiln_collection *registry;
    kiln_entry *entry;
    size_t index;
    if (heap == NULL || id == 0) abort();
    realm = &heap->realm;
    entry = realm->isolated_escrow_entry;
    if (realm->isolated_request_id != id || realm->isolated_request_invalidated ||
        realm->isolated_graphs.tag != KILN_VALUE_COLLECTION || entry == NULL ||
        realm->isolated_escrow_value.tag != published_root.tag ||
        realm->isolated_escrow_value.bits != published_root.bits ||
        realm->isolated_escrow_value.ptr != published_root.ptr ||
        realm->isolated_escrow_value.len != published_root.len ||
        realm->isolated_request_handle.tag != realm->isolated_escrow_key.tag ||
        realm->isolated_request_handle.bits != realm->isolated_escrow_key.bits) abort();
    registry = kiln_c99_collection_of(realm->isolated_graphs);
    if (registry->entries == NULL || registry->live == 0) abort();
    for (index = 0; index < registry->count && &registry->entries[index] != entry; ++index) {}
    if (index == registry->count || entry->dead ||
        entry->key.tag != realm->isolated_escrow_key.tag ||
        entry->key.bits != realm->isolated_escrow_key.bits ||
        entry->value.tag != KILN_VALUE_UNDEFINED) abort();
    kiln_c99_collection_hash_remove(registry, index, entry->key);
    entry->dead = 1;
    entry->key = kiln_c99_undefined();
    entry->value = kiln_c99_undefined();
    registry->live -= 1;
    realm->isolated_escrow_entry = NULL;
    realm->isolated_escrow_key = kiln_c99_undefined();
    realm->isolated_escrow_value = kiln_c99_undefined();
    realm->isolated_request_handle = kiln_c99_undefined();
    realm->isolated_request_id = 0;
    realm->isolated_request_invalidated = 0;
}

/* A single read-only enumeration of the realm context's strong C roots. GC marking and the
 * graph incoming proof must see the same slots: adding a realm field to only one of those walks
 * could either collect a live value or move a graph still held by the sender. The two value
 * layouts are distinct (`kiln_value` and the generated units' compact `kiln_val`). A visitor
 * may stop early; the only excluded slot is the active graph request's escrow value. */
typedef struct kiln_c99_realm_root_visitor {
    void *context;
    int (*value)(void *context, const kiln_value *slot);
    int (*val)(void *context, kiln_val value);
} kiln_c99_realm_root_visitor;

static int kiln_c99_gc_visit_realm_roots(const kiln_gc_heap *heap,
                                         const kiln_c99_realm_root_visitor *visitor,
                                         const kiln_value *excluded_escrow_slot) {
    const kiln_c99_realm_state *realm = &heap->realm;
    size_t index, slot;
#define KILN_VISIT_REALM_VALUE(address)                                                        \
    do {                                                                                        \
        if ((address) != excluded_escrow_slot &&                                              \
            visitor->value(visitor->context, (address))) return 1;                             \
    } while (0)
    for (index = 0; index < realm->symbol_count; ++index) {
        const kiln_symbol *symbol = (const kiln_symbol *)realm->symbols[index].ptr;
        /* Symbol.for registrations are rooted for the realm. Unique symbols remain a weak
         * identity index; values and symbol property keys provide their actual roots. */
        if (symbol != NULL && symbol->registry_id != 0)
            KILN_VISIT_REALM_VALUE(&realm->symbols[index]);
    }
    KILN_VISIT_REALM_VALUE(&realm->isolated_graphs);
    KILN_VISIT_REALM_VALUE(&realm->http_client_requests);
    KILN_VISIT_REALM_VALUE(&realm->isolated_request_handle);
    KILN_VISIT_REALM_VALUE(&realm->isolated_escrow_key);
    KILN_VISIT_REALM_VALUE(&realm->isolated_escrow_value);
    KILN_VISIT_REALM_VALUE(&realm->graph_inbox_root);
    KILN_VISIT_REALM_VALUE(&realm->array_unscopables);
    KILN_VISIT_REALM_VALUE(&realm->array_iterator_prototype);
    KILN_VISIT_REALM_VALUE(&realm->string_iterator_prototype);
    KILN_VISIT_REALM_VALUE(&realm->regexp_string_iterator_prototype);
    KILN_VISIT_REALM_VALUE(&realm->regexp_legacy_empty);
    KILN_VISIT_REALM_VALUE(&realm->regexp_legacy_subject);
    KILN_VISIT_REALM_VALUE(&realm->typed_array_iterator_prototype);
    KILN_VISIT_REALM_VALUE(&realm->map_iterator_prototype);
    KILN_VISIT_REALM_VALUE(&realm->set_iterator_prototype);
    for (index = 0; index < sizeof realm->function_kind_prototypes / sizeof *realm->function_kind_prototypes;
         ++index)
        KILN_VISIT_REALM_VALUE(&realm->function_kind_prototypes[index]);
    for (index = 0; index < sizeof realm->generator_object_prototypes / sizeof *realm->generator_object_prototypes;
         ++index)
        KILN_VISIT_REALM_VALUE(&realm->generator_object_prototypes[index]);
    for (index = 0; index < sizeof realm->native_prototypes / sizeof *realm->native_prototypes;
         ++index)
        KILN_VISIT_REALM_VALUE(&realm->native_prototypes[index]);
    for (index = 0; index < realm->unit_capacity; ++index) {
        const kiln_c99_unit_block *unit = realm->units[index];
        if (unit == NULL) continue;
        for (slot = 0; slot < unit->val_count; ++slot)
            if (visitor->val(visitor->context, unit->vals[slot])) return 1;
    }
    for (index = 0; index < realm->region_own_buckets; ++index) {
        const kiln_c99_region_own *entry;
        for (entry = realm->region_own[index]; entry != NULL; entry = entry->next) {
            KILN_VISIT_REALM_VALUE(&entry->prototype);
            KILN_VISIT_REALM_VALUE(&entry->store);
        }
    }
#undef KILN_VISIT_REALM_VALUE
    return 0;
}

typedef struct kiln_c99_graph_root_alias_visitor {
    const kiln_gc_heap *heap;
    kiln_nursery_chunk *const *chunks;
    size_t count;
} kiln_c99_graph_root_alias_visitor;

static int kiln_c99_graph_realm_value_alias(void *opaque, const kiln_value *slot) {
    const kiln_c99_graph_root_alias_visitor *visit =
        (const kiln_c99_graph_root_alias_visitor *)opaque;
    return kiln_c99_graph_value_in_batch(visit->heap, visit->chunks, visit->count, slot);
}

static int kiln_c99_graph_realm_val_alias(void *opaque, kiln_val value) {
    const kiln_c99_graph_root_alias_visitor *visit =
        (const kiln_c99_graph_root_alias_visitor *)opaque;
    return kiln_c99_graph_val_in_batch(visit->heap, visit->chunks, visit->count, value);
}

/* Inspect C-owned direct roots and key-table side pointers. Unpinned canonical keys are weak
 * for GC, but they still contain an address until purge and cannot point into a moved chunk.
 * This is a useful negative witness, not a complete root enumeration: the provider, generated
 * modules, host handles, conservative stack and edges from outside blocks remain separate. */
static int kiln_c99_graph_ledger_enabled(void);
static int kiln_c99_graph_ledger_audit_enabled(void);

static int kiln_c99_graph_has_c_root_alias_except(const kiln_gc_heap *heap,
                                                   kiln_nursery_chunk *const *chunks,
                                                   size_t count,
                                                   const kiln_value *excluded_escrow_slot, int trusted_key_metadata) {
    kiln_c99_graph_root_alias_visitor context = { heap, chunks, count };
    kiln_c99_realm_root_visitor visitor = {
        &context, kiln_c99_graph_realm_value_alias, kiln_c99_graph_realm_val_alias
    };
    size_t index;
    if (kiln_c99_gc_visit_realm_roots(heap, &visitor, excluded_escrow_slot)) return 1;
    /* The realm symbol array is a weak identity index, not a root, but it still contains raw
     * pointers used by reflection. Never transfer one of those symbols away from its index. */
    for (index = 0; index < heap->realm.symbol_count; ++index)
        if (kiln_c99_graph_in_batch(heap, chunks, count, heap->realm.symbols[index].ptr))
            return 1;
    for (index = heap->job_head; index < heap->job_tail; ++index) {
        const kiln_microtask *job = &heap->jobs[index & heap->job_mask];
        if (kiln_c99_graph_value_in_batch(heap, chunks, count, &job->handler) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &job->argument) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &job->derived)) return 1;
    }
    /* Production keys are fresh canonical allocations outside candidate ownership.
     * Keep the full key/hot-key walk for the independent oracle and legacy probes. */
    if (!trusted_key_metadata) {
        for (index = 0; index < heap->keys.capacity; ++index) {
            const kiln_key_entry *entry = &heap->keys.entries[index];
            if (kiln_c99_graph_in_batch(heap, chunks, count, entry->string)) return 1;
        }
        for (index = 0; index < KILN_C99_HOT_KEY_COUNT; ++index) {
            if (kiln_c99_graph_in_batch(heap, chunks, count, heap->hot_keys[index])) return 1;
        }
    }
    /* Weak resource handles are not GC roots, but their stored addresses must remain in
     * this heap until the registration is removed. Native disposal may still inspect them. */
    for (const kiln_gc_weak_resource *entry = heap->weak_resources;
         entry != NULL; entry = entry->next) {
        if (kiln_c99_graph_value_in_batch(heap, chunks, count, &entry->handle) ||
            kiln_c99_graph_in_batch(heap, chunks, count, entry->payload) ||
            (entry->aliases != NULL && entry->aliases(heap, chunks, count, entry->payload)))
            return 1;
    }
    for (size_t keep = 0; keep < heap->weak_keep_count; ++keep)
        if (kiln_c99_graph_value_in_batch(heap, chunks, count,
                                          &heap->weak_keep_during_job[keep])) return 1;
    return kiln_c99_graph_in_batch(heap, chunks, count, heap->object_prototype_store) ||
           kiln_c99_graph_in_batch(heap, chunks, count, heap->shared_facades) ||
           kiln_c99_graph_in_batch(heap, chunks, count, heap->weak_collections);
}

static int kiln_c99_graph_has_c_root_alias(const kiln_gc_heap *heap,
                                           kiln_nursery_chunk *const *chunks, size_t count) {
    return kiln_c99_graph_has_c_root_alias_except(heap, chunks, count, NULL, 0);
}

/* Only the active request's one escrow slot can be excluded. A second C root containing an
 * identical pointer still counts as an alias; passing a graph value never grants exclusion. */
static int kiln_c99_graph_has_c_root_alias_for_request(const kiln_gc_heap *heap,
                                                       kiln_nursery_chunk *const *chunks,
                                                       size_t count, uint64_t request_id) {
    const kiln_c99_realm_state *realm = &heap->realm;
    if (request_id == 0 || realm->isolated_request_id != request_id ||
        realm->isolated_escrow_entry == NULL) return 1;
    return kiln_c99_graph_has_c_root_alias_except(heap, chunks, count,
                                                   &realm->isolated_escrow_value,
        kiln_c99_graph_ledger_enabled() && !kiln_c99_graph_ledger_audit_enabled());
}

typedef struct kiln_c99_graph_root_visit {
    const kiln_gc_heap *heap;
    kiln_nursery_chunk *const *chunks;
    size_t count;
    int alias;
    int invalid;
} kiln_c99_graph_root_visit;

static int kiln_c99_graph_visit_root(void *opaque, const void *pointer,
                                               uint32_t kind) {
    kiln_c99_graph_root_visit *visit = (kiln_c99_graph_root_visit *)opaque;
    if (kind == KILN_GC_VISIT_BLOCK) {
        visit->alias |= kiln_c99_graph_in_batch(visit->heap, visit->chunks, visit->count, pointer);
    } else if (kind == KILN_GC_VISIT_VALUE && pointer != NULL) {
        visit->alias |= kiln_c99_graph_in_batch(visit->heap, visit->chunks, visit->count, pointer);
        visit->alias |= kiln_c99_graph_value_in_batch(visit->heap, visit->chunks, visit->count,
                                                      (const kiln_value *)pointer);
    } else {
        visit->invalid = 1;
        return 1;
    }
    return 0;
}


/* A separate, read-only walk for edges *into* the candidate. The collector's tracer is not
 * suitable here: it mutates mark epochs and deliberately ignores non-heap pointers. Until each
 * kind's complete pointer layout is audited, an unhandled kind means "unknown", never "clear". */
/* One host-reference inventory feeds collection, incoming proof and publication recording.
 * Host handles, event buffers and malloc-owned transport bytes are not language heap edges.
 * V is a wide value, P a GC pointer and A a malloc-owned array of wide values. */
#define KILN_C99_HOST_SERVER_FIELDS(V, P, A) \
    V(object) V(connection_listener) P(sockets)
#define KILN_C99_HOST_SOCKET_FIELDS(V, P, A) \
    V(object) V(write_callbacks) V(end_callbacks) V(upgrade_write_callbacks) P(next) P(server) P(client_requests) P(responses)
#define KILN_C99_HOST_REQUEST_FIELDS(V, P, A) \
    V(object) V(callback) V(response) P(socket) P(next)
#define KILN_C99_HOST_RESPONSE_FIELDS(V, P, A) \
    V(request) V(object) V(url_object) V(search_params_object) \
    V(pathname_val) V(query_val) V(param_val) P(socket) P(next)
#define KILN_C99_HOST_COMPLETION_FIELDS(V, P, A) \
    V(write_callbacks) P(server) P(socket) P(client_request)
#define KILN_C99_HOST_FS_FIELDS(V, P, A) V(promise)
#define KILN_C99_HOST_PROCESS_FIELDS(V, P, A) \
    V(promise) V(result) V(stdout_value) V(stderr_value)
#define KILN_C99_HOST_CALLBACK_FIELDS(V, P, A) V(callback) A(args, argc)
#define KILN_C99_HOST_LAYOUTS(CASE) \
    CASE(KILN_GC_NODE_SERVER, kiln_node_server, KILN_C99_HOST_SERVER_FIELDS) \
    CASE(KILN_GC_NODE_SOCKET, kiln_node_socket, KILN_C99_HOST_SOCKET_FIELDS) \
    CASE(KILN_GC_NODE_CLIENT_REQUEST, kiln_node_client_request, KILN_C99_HOST_REQUEST_FIELDS) \
    CASE(KILN_GC_NODE_RESPONSE, kiln_node_response, KILN_C99_HOST_RESPONSE_FIELDS) \
    CASE(KILN_GC_NODE_COMPLETION, kiln_node_completion, KILN_C99_HOST_COMPLETION_FIELDS) \
    CASE(KILN_GC_NODE_FS_COMPLETION, kiln_node_fs_completion, KILN_C99_HOST_FS_FIELDS) \
    CASE(KILN_GC_NODE_PROCESS_COMPLETION, kiln_node_process_completion, KILN_C99_HOST_PROCESS_FIELDS) \
    CASE(KILN_GC_NEXT_TICK, kiln_next_tick, KILN_C99_HOST_CALLBACK_FIELDS) \
    CASE(KILN_GC_TIMER, kiln_timer, KILN_C99_HOST_CALLBACK_FIELDS)

static kiln_c99_graph_incoming_result kiln_c99_graph_outside_block(
    const kiln_gc_heap *heap, kiln_nursery_chunk *const *chunks, size_t count,
    const kiln_gc_block *block) {
    size_t index;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_BYTES:
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    case KILN_GC_STRING: {
        const kiln_string *string = (const kiln_string *)(block + 1);
        if (block->size < sizeof *string) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, string->data))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        return kiln_c99_graph_inline_string(block) ? KILN_C99_GRAPH_ROOTS_UNENUMERATED
                                                   : KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
    }
    case KILN_GC_SLICED_STRING: {
        const kiln_sliced_string *string = (const kiln_sliced_string *)(block + 1);
        if (block->size < sizeof *string) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, string->parent) ||
            kiln_c99_graph_in_batch(heap, chunks, count, string->base.data))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        /* Validity of an outside slice's parent is the collector's concern. Both pointer
         * fields were inspected; no other field can name the candidate. */
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_CELL:
        if (block->size < sizeof(kiln_val)) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_val_in_batch(heap, chunks, count,
                                           *(const kiln_val *)(block + 1))
                   ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    case KILN_GC_SHARED_FACADE: {
        const kiln_shared_facade *facade = (const kiln_shared_facade *)(block + 1);
        if (block->size < sizeof *facade) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        /* The Rust graph lease and node IDs are numeric. The owner and identity-chain links
         * are the complete realm-local pointer layout, including finalizer side ownership. */
        if (kiln_c99_graph_in_batch(heap, chunks, count, facade->owner) ||
            kiln_c99_graph_in_batch(heap, chunks, count, facade->previous) ||
            kiln_c99_graph_in_batch(heap, chunks, count, facade->next) ||
            kiln_c99_graph_in_batch(heap, chunks, count, facade->hash_next))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_SYMBOL: {
        const kiln_symbol *symbol = (const kiln_symbol *)(block + 1);
        if (block->size < sizeof *symbol) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        /* The serial/registry ID and description length are numeric. These are exactly the
         * three pointers traced for a symbol by the collector. */
        if (kiln_c99_graph_in_batch(heap, chunks, count, symbol->key) ||
            kiln_c99_graph_in_batch(heap, chunks, count, symbol->description) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &symbol->description_value))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_CLOSURE: {
        const kiln_closure *closure = (const kiln_closure *)(block + 1);
        if (block->size < sizeof *closure || closure->region ||
            (closure->upvalue_count != 0 && closure->upvalues == NULL) ||
            (closure->upvalues == closure->inline_upvalues &&
             closure->upvalue_count > sizeof closure->inline_upvalues /
                                      sizeof *closure->inline_upvalues))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        /* Match the GC tracer's values and inspect its side allocation and immutable source
         * pointers too. The generated body points to executable code, not a heap block. */
        if (kiln_c99_graph_in_batch(heap, chunks, count, closure->upvalues) ||
            kiln_c99_graph_in_batch(heap, chunks, count, closure->name) ||
            kiln_c99_graph_in_batch(heap, chunks, count, closure->source) ||
            kiln_c99_graph_in_batch(heap, chunks, count, closure->diagnostic_source) ||
            kiln_c99_graph_val_in_batch(heap, chunks, count, closure->captured_this) ||
            kiln_c99_graph_val_in_batch(heap, chunks, count, closure->captured_new_target) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &closure->captured_arguments) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &closure->own_prototype) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &closure->own_store) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &closure->source_value))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        for (index = 0; index < closure->upvalue_count; ++index) {
            if (kiln_c99_graph_val_in_batch(heap, chunks, count, closure->upvalues[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_PROXY: {
        const kiln_proxy *proxy = (const kiln_proxy *)(block + 1);
        if (block->size < sizeof *proxy) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_value_in_batch(heap, chunks, count, &proxy->target) ||
               kiln_c99_graph_value_in_batch(heap, chunks, count, &proxy->handler)
                   ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_PROMISE: {
        const kiln_promise *promise = (const kiln_promise *)(block + 1);
        if (block->size < sizeof *promise || promise->count > promise->capacity ||
            (promise->count != 0 && promise->reactions == NULL) ||
            (promise->reactions == &promise->inline_reaction && promise->count > 1))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, promise->reactions) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &promise->value) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &promise->properties))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        for (index = 0; index < promise->count; ++index) {
            const kiln_reaction *reaction = &promise->reactions[index];
            if (kiln_c99_graph_value_in_batch(heap, chunks, count, &reaction->on_fulfilled) ||
                kiln_c99_graph_value_in_batch(heap, chunks, count, &reaction->on_rejected) ||
                kiln_c99_graph_value_in_batch(heap, chunks, count, &reaction->derived))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_SETTLER: {
        const kiln_settler *settler = (const kiln_settler *)(block + 1);
        if (block->size < sizeof *settler) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_value_in_batch(heap, chunks, count, &settler->promise)
                   ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_GENERATOR: {
        const kiln_generator *frame = (const kiln_generator *)(block + 1);
        const kiln_closure *closure;
        if (block->size < sizeof *frame || frame->queue_count > frame->queue_capacity ||
            (frame->queue_count != 0 &&
             (frame->queue_promises == NULL || frame->queue_sent == NULL)))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->properties) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->prototype) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->function) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->receiver) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->promise) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->sent) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->delegate) ||
            kiln_c99_graph_in_batch(heap, chunks, count, frame->locals) ||
            kiln_c99_graph_in_batch(heap, chunks, count, frame->stack) ||
            kiln_c99_graph_in_batch(heap, chunks, count, frame->handlers) ||
            kiln_c99_graph_in_batch(heap, chunks, count, frame->handler_stack) ||
            kiln_c99_graph_in_batch(heap, chunks, count, frame->queue_promises) ||
            kiln_c99_graph_in_batch(heap, chunks, count, frame->queue_sent))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        if (frame->function.tag != KILN_VALUE_FUNCTION)
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        closure = (const kiln_closure *)(uintptr_t)frame->function.bits;
        if (closure == NULL || (closure->local_count != 0 && frame->locals == NULL) ||
            (closure->stack_size != 0 && frame->stack == NULL))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        for (index = 0; index < closure->local_count; ++index) {
            if (kiln_c99_graph_val_in_batch(heap, chunks, count, frame->locals[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        for (index = 0; index < closure->stack_size; ++index) {
            if (kiln_c99_graph_val_in_batch(heap, chunks, count, frame->stack[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        for (index = 0; index < frame->queue_count; ++index) {
            if (kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->queue_promises[index]) ||
                kiln_c99_graph_value_in_batch(heap, chunks, count, &frame->queue_sent[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_BOUND: {
        const kiln_bound *bound = (const kiln_bound *)(block + 1);
        if (block->size < sizeof *bound || (bound->count != 0 && bound->arguments == NULL))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_value_in_batch(heap, chunks, count, &bound->target) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &bound->receiver) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &bound->properties) ||
            kiln_c99_graph_in_batch(heap, chunks, count, bound->arguments))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        for (index = 0; index < bound->count; ++index) {
            if (kiln_c99_graph_value_in_batch(heap, chunks, count, &bound->arguments[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
#define KILN_C99_HOST_INCOMING_VALUE(field) \
    if (kiln_c99_graph_value_in_batch(heap, chunks, count, &owner->field)) \
        return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
#define KILN_C99_HOST_INCOMING_POINTER(field) \
    if (kiln_c99_graph_in_batch(heap, chunks, count, owner->field)) \
        return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
#define KILN_C99_HOST_INCOMING_ARRAY(field, length) \
    if (owner->length != 0 && owner->field == NULL) \
        return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED; \
    KILN_C99_HOST_INCOMING_POINTER(field) \
    for (index = 0; index < owner->length; ++index) { \
        if (kiln_c99_graph_value_in_batch(heap, chunks, count, &owner->field[index])) \
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS; \
    }
#define KILN_C99_HOST_INCOMING_CASE(kind, type, fields) \
    case kind: { \
        const type *owner = (const type *)(block + 1); \
        if (block->size < sizeof *owner) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED; \
        fields(KILN_C99_HOST_INCOMING_VALUE, KILN_C99_HOST_INCOMING_POINTER, \
               KILN_C99_HOST_INCOMING_ARRAY) \
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED; \
    }
    KILN_C99_HOST_LAYOUTS(KILN_C99_HOST_INCOMING_CASE)
#undef KILN_C99_HOST_INCOMING_CASE
#undef KILN_C99_HOST_INCOMING_ARRAY
#undef KILN_C99_HOST_INCOMING_POINTER
#undef KILN_C99_HOST_INCOMING_VALUE
    case KILN_GC_COMMONJS_REQUIRE: {
        const kiln_commonjs_require *require = (const kiln_commonjs_require *)(block + 1);
        if (block->size < sizeof *require || require->call == NULL)
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, require->context) ||
            kiln_c99_graph_in_batch(heap, chunks, count, require->root_context))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        if (kiln_c99_graph_value_in_batch(heap, chunks, count, &require->properties))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        if (require->visit_context == NULL)
            return require->context == NULL && require->root_context == NULL
                ? KILN_C99_GRAPH_ROOTS_UNENUMERATED : KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (require->root_context == NULL) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        /* The context's values belong to the independently enumerated registered roots,
         * not to this GC block. Validate the live authority here; the final provider-root
         * pass then visits it once, alongside all other registered and provider roots. */
        return kiln_realm_gc_has_root_visitor((void *)heap, require->visit_context,
                                              require->root_context) == 1
            ? KILN_C99_GRAPH_ROOTS_UNENUMERATED : KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
    }
    case KILN_GC_METHOD_STORE: {
        const kiln_method_store *store = (const kiln_method_store *)(block + 1);
        if (block->size < sizeof *store) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_value_in_batch(heap, chunks, count, &store->properties) ||
               kiln_c99_graph_in_batch(heap, chunks, count, store->next)
                   ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_DATE: {
        const kiln_date *date = (const kiln_date *)(block + 1);
        if (block->size != sizeof *date) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_value_in_batch(heap, chunks, count, &date->properties)
            ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_REGEXP: {
        const kiln_regexp *regexp = (const kiln_regexp *)(block + 1);
        if (block->size < sizeof *regexp || regexp->cached_group_count > 32)
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, regexp->handle) ||
            kiln_c99_graph_in_batch(heap, chunks, count, regexp->source) ||
            kiln_c99_graph_in_batch(heap, chunks, count, regexp->flags) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &regexp->properties) ||
            kiln_c99_graph_in_batch(heap, chunks, count, regexp->cached_subject_ptr))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        for (index = 0; index < regexp->cached_group_count; ++index) {
            if (kiln_c99_graph_val_in_batch(heap, chunks, count, regexp->cached_elements[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_TYPED_BUFFER: {
        const kiln_typed_buffer *buffer = (const kiln_typed_buffer *)(block + 1);
        if (block->size < sizeof *buffer) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_in_batch(heap, chunks, count, buffer->bytes) ||
               kiln_c99_graph_in_batch(heap, chunks, count, buffer->shared)
                   ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_TYPED_ARRAY: {
        const kiln_typed_array *array = (const kiln_typed_array *)(block + 1);
        if (block->size < sizeof *array) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_in_batch(heap, chunks, count, array->buffer) ||
               kiln_c99_graph_value_in_batch(heap, chunks, count, &array->properties)
                   ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_DATA_VIEW: {
        const kiln_data_view *view = (const kiln_data_view *)(block + 1);
        if (block->size < sizeof *view) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        return kiln_c99_graph_in_batch(heap, chunks, count, view->buffer)
                   ? KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS : KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_COLLECTION: {
        const kiln_collection *collection = (const kiln_collection *)(block + 1);
        if (block->size < sizeof *collection) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, collection->entries) ||
            kiln_c99_graph_in_batch(heap, chunks, count, collection->hash_table) ||
            kiln_c99_graph_in_batch(heap, chunks, count, collection->weak_previous) ||
            kiln_c99_graph_in_batch(heap, chunks, count, collection->weak_next) ||
            kiln_c99_graph_in_batch(heap, chunks, count, collection->weak_owner) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &collection->properties))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        if (collection->live > collection->count || collection->count > collection->capacity ||
            (collection->count != 0 && collection->entries == NULL) ||
            (collection->hash_capacity != 0 && collection->hash_table == NULL))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        /* Weak entries are not ordinary GC roots, but their key/value bytes can still be
         * observed by ephemeron processing or a later mutation. Inspect dead slots too: a
         * stale address in an outside side allocation cannot safely follow a moved chunk. */
        for (index = 0; index < collection->count; ++index) {
            const kiln_entry *entry = &collection->entries[index];
            if (kiln_c99_graph_value_in_batch(heap, chunks, count, &entry->key) ||
                kiln_c99_graph_value_in_batch(heap, chunks, count, &entry->value))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_ARRAY: {
        const kiln_array *array = (const kiln_array *)(block + 1);
        if (block->size < sizeof *array) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, array->elements) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &array->properties) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &array->match_input) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &array->match_groups))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        if (array->count > array->capacity || (array->count != 0 && array->elements == NULL))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        for (index = 0; index < array->count; ++index) {
            if (kiln_c99_graph_val_in_batch(heap, chunks, count, array->elements[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    case KILN_GC_OBJECT: {
        const kiln_object *object = (const kiln_object *)(block + 1);
        if (block->size < sizeof *object) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, object->properties) ||
            kiln_c99_graph_in_batch(heap, chunks, count, object->shape) ||
            kiln_c99_graph_in_batch(heap, chunks, count, object->slots) ||
            kiln_c99_graph_in_batch(heap, chunks, count, object->elements) ||
            kiln_c99_graph_in_batch(heap, chunks, count, object->sealed) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &object->prototype) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &object->boxed_primitive))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        if (object->count > object->capacity || object->element_count > object->element_capacity ||
            (object->count != 0 && object->properties == NULL) ||
            (object->element_count != 0 && object->elements == NULL) ||
            (object->sealed != NULL &&
             (object->slots == NULL || object->slot_capacity < object->sealed->count)))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (object->shape != NULL) {
            const kiln_shape *shape = object->shape;
            /* The side table scan covers published local shapes. A shape reached only
             * through this object is outside that proof and must still refuse. */
            if ((shape->shared & 1u) == 0) {
                const kiln_shape *published = heap->local_shapes;
                size_t visited = 0;
                while (published != NULL && visited < heap->local_shape_count &&
                       published != shape) {
                    published = published->owned_next;
                    ++visited;
                }
                if (published != shape) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
            }
            if (object->sealed != NULL ||
                shape->property_count > KILN_SHAPE_MAX_FAST_PROPERTIES ||
                (object->slots == object->inline_slots &&
                 shape->property_count > KILN_C99_INLINE_SLOTS) ||
                (shape->property_count != 0 &&
                 (object->slots == NULL || object->slot_capacity < shape->property_count)))
                return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
            for (index = 0; index < shape->property_count; ++index) {
                if (shape->keys[index] == NULL)
                    return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
                if (kiln_c99_graph_in_batch(heap, chunks, count, shape->keys[index]) ||
                    kiln_c99_graph_symbol_key_in_batch(heap, chunks, count,
                                                       shape->keys[index]) ||
                    kiln_c99_graph_val_in_batch(heap, chunks, count, object->slots[index]))
                    return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
            }
        }
        if (object->sealed != NULL) {
            for (index = 0; index < object->sealed->count; ++index) {
                if (kiln_c99_graph_symbol_key_in_batch(
                        heap, chunks, count, object->sealed->entries[index].key) ||
                    kiln_c99_graph_val_in_batch(heap, chunks, count, object->slots[index]))
                    return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
            }
        }
        for (index = 0; index < object->count; ++index) {
            const kiln_property *property = &object->properties[index];
            if (kiln_c99_graph_in_batch(heap, chunks, count, property->key) ||
                kiln_c99_graph_symbol_key_in_batch(heap, chunks, count, property->key) ||
                (property->accessor
                     ? kiln_c99_graph_value_in_batch(heap, chunks, count, &property->getter) ||
                       kiln_c99_graph_value_in_batch(heap, chunks, count, &property->setter)
                     : kiln_c99_graph_value_in_batch(heap, chunks, count, &property->value)))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        for (index = 0; index < object->element_count; ++index) {
            if (kiln_c99_graph_val_in_batch(heap, chunks, count, object->elements[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    default:
        return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
    }
}

/* Construction/publication uses the same field inventory as incoming certification.
 * This observer cannot run user code or collect, and is installed only on the heap's realm
 * thread after all published blocks are indexed. Unknown layouts invalidate ledger proof. */
typedef struct kiln_c99_candidate_owner_observer {
    kiln_gc_heap *heap;
    const void *owner;
} kiln_c99_candidate_owner_observer;

static void kiln_c99_candidate_observe_pointer(void *context, const void *pointer) {
    kiln_c99_candidate_owner_observer *observer = context;
    kiln_c99_candidate_record_pointer_store(observer->heap, observer->owner, pointer);
}

static void kiln_c99_candidate_record_owner_layout(kiln_gc_heap *heap,
                                                   const kiln_gc_block *block) {
    kiln_c99_candidate_owner_observer observer = {heap, block + 1};
    kiln_c99_graph_incoming_result result;
    if (heap == NULL || heap->candidate_ledger_groups == 0) return;
    if (heap->graph_pointer_observer != NULL || heap->graph_proof_sides != NULL) {
        heap->candidate_ledger_publication_failed = 1;
        return;
    }
    heap->graph_pointer_observer_context = &observer;
    heap->graph_pointer_observer = kiln_c99_candidate_observe_pointer;
    result = kiln_c99_graph_outside_block(heap, NULL, 0, block);
    heap->graph_pointer_observer = NULL;
    heap->graph_pointer_observer_context = NULL;
    if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED)
        heap->candidate_ledger_publication_failed = 1;
}

static int kiln_c99_candidate_context_publish(kiln_gc_heap *heap,
                                               kiln_commonjs_require *require) {
    if (heap == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (require->context == NULL && require->root_context == NULL) return KILN_C99_OK;
    if (kiln_c99_gc_ptrset_insert(&heap->candidate_context_owners,
                                  ((kiln_gc_block *)require) - 1) != KILN_C99_OK) {
        heap->candidate_context_inventory_failed = 1;
        return KILN_C99_OUT_OF_MEMORY;
    }
    return KILN_C99_OK;
}

/* Context authority is independent of ordinary store witnesses: it can expire without a
 * write to this GC object, and an opaque context may predate the first candidate. */
static kiln_c99_graph_incoming_result kiln_c99_graph_scan_context_owners(
    const kiln_gc_heap *heap, kiln_nursery_chunk *const *chunks, size_t count) {
    if (heap->candidate_context_inventory_failed) return KILN_C99_GRAPH_OUTGOING_UNPROVEN;
    for (size_t index = 0; index < heap->candidate_context_owners.capacity; ++index) {
        const void *witness = heap->candidate_context_owners.table[index];
        uintptr_t address = (uintptr_t)witness;
        kiln_gc_block *owner;
        kiln_c99_graph_incoming_result result;
        if (witness == NULL || address > UINTPTR_MAX - sizeof(kiln_gc_block)) continue;
        owner = kiln_c99_gc_lookup(heap, (const void *)(address + sizeof(kiln_gc_block)));
        if (owner == NULL || (owner->kind & KILN_GC_KIND_MASK) != KILN_GC_COMMONJS_REQUIRE)
            continue;
        result = kiln_c99_graph_outside_block(heap, chunks, count, owner);
        if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
    }
    return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
}

static void kiln_c99_candidate_record_chunk_layout(kiln_gc_heap *heap,
                                                   const kiln_nursery_chunk *chunk) {
    if (chunk->limit < chunk->base ||
        chunk->limit > (const char *)chunk + heap->nursery.capacity) {
        heap->candidate_ledger_publication_failed = 1;
        return;
    }
    for (size_t word = 0; word < chunk->bitmap_words; ++word) {
        uint64_t bits = chunk->bitmap[word];
        while (bits != 0) {
            size_t bit = (word << 6) + (size_t)__builtin_ctzll(bits);
            uintptr_t address = (uintptr_t)chunk->base + (bit << 4);
            const kiln_gc_block *block;
            bits &= bits - 1;
            if (address < (uintptr_t)chunk->base || address > (uintptr_t)chunk->limit ||
                (uintptr_t)chunk->limit - address < sizeof(kiln_gc_block)) {
                heap->candidate_ledger_publication_failed = 1;
                return;
            }
            block = (const kiln_gc_block *)address;
            if ((block->kind & KILN_GC_FLAG_DEAD) == 0)
                kiln_c99_candidate_record_owner_layout(heap, block);
        }
    }
}

static kiln_c99_graph_incoming_result kiln_c99_graph_scan_nursery_chunk(
    const kiln_gc_heap *heap, kiln_nursery_chunk *const *chunks, size_t count,
    const kiln_nursery_chunk *chunk, const char *limit) {
    size_t word;
    size_t index;
    for (index = 0; index < count; ++index) if (chunks[index] == chunk)
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    if (limit < chunk->base || limit > (const char *)chunk + heap->nursery.capacity)
        return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
    for (word = 0; word < chunk->bitmap_words; ++word) {
        uint64_t bits = chunk->bitmap[word];
        while (bits != 0) {
            size_t bit = (word << 6) + (size_t)__builtin_ctzll(bits);
            uintptr_t address = (uintptr_t)chunk->base + (bit << 4);
            const kiln_gc_block *block;
            kiln_c99_graph_incoming_result result;
            bits &= bits - 1;
            if (address < (uintptr_t)chunk->base || address > (uintptr_t)limit ||
                (uintptr_t)limit - address < sizeof(kiln_gc_block))
                return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
            block = (const kiln_gc_block *)address;
            if ((block->kind & KILN_GC_FLAG_DEAD) != 0) continue;
            result = kiln_c99_graph_outside_block(heap, chunks, count, block);
            if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
        }
    }
    return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
}

/* Realm-local shapes and the edge table out of shared shapes are malloc-owned side indexes,
 * not GC blocks. They can retain canonical keys even when no ordinary object uses a shape.
 * Inspect every published pointer before allowing an object/string candidate to leave. */
static kiln_c99_graph_incoming_result kiln_c99_graph_scan_local_shapes(
    const kiln_gc_heap *heap, kiln_nursery_chunk *const *chunks, size_t count) {
    const kiln_shape *shape;
    size_t seen_shapes = 0, index, seen_rows = 0;
    if (kiln_c99_graph_in_batch(heap, chunks, count, heap->local_shapes) ||
        kiln_c99_graph_in_batch(heap, chunks, count, heap->local_edges))
        return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
    if ((heap->local_shape_count != 0 && heap->local_shapes == NULL) ||
        (heap->local_edge_capacity != 0 && heap->local_edges == NULL) ||
        heap->local_edge_count > heap->local_edge_capacity)
        return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
    for (shape = heap->local_shapes; shape != NULL; shape = shape->owned_next) {
        const kiln_shape_transition *edge;
        size_t seen_edges = 0;
        if (++seen_shapes > heap->local_shape_count || shape->shared ||
            shape->property_count > KILN_SHAPE_MAX_FAST_PROPERTIES ||
            shape->num_transitions > 65536u)
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (kiln_c99_graph_in_batch(heap, chunks, count, shape->parent) ||
            kiln_c99_graph_in_batch(heap, chunks, count, shape->ikey) ||
            kiln_c99_graph_in_batch(heap, chunks, count, shape->owned_next) ||
            kiln_c99_graph_in_batch(heap, chunks, count, shape->transitions) ||
            kiln_c99_graph_in_batch(heap, chunks, count, shape->edge_table))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        for (index = 0; index < shape->property_count; ++index) {
            if (kiln_c99_graph_in_batch(heap, chunks, count, shape->keys[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        for (index = 0; index < shape->num_transitions && index < 2; ++index) {
            if (kiln_c99_graph_in_batch(heap, chunks, count, shape->inline_trans_ikey[index]) ||
                kiln_c99_graph_in_batch(heap, chunks, count, shape->inline_trans_next[index]))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        for (edge = shape->transitions; edge != NULL; edge = edge->next_transition) {
            if (++seen_edges > shape->num_transitions)
                return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
            if (kiln_c99_graph_in_batch(heap, chunks, count, edge->ikey) ||
                kiln_c99_graph_in_batch(heap, chunks, count, edge->next) ||
                kiln_c99_graph_in_batch(heap, chunks, count, edge->next_transition))
                return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
        }
        if (seen_edges != (shape->num_transitions > 2 ? shape->num_transitions - 2 : 0))
            return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
        if (shape->edge_table != NULL) {
            const kiln_shape_edge_table *table = shape->edge_table;
            if (table->mask > (1u << 20))
                return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
            for (index = 0; index <= table->mask; ++index) {
                if (kiln_c99_graph_in_batch(heap, chunks, count, table->edges[index]))
                    return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
            }
        }
    }
    if (seen_shapes != heap->local_shape_count) return KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
    for (index = 0; index < heap->local_edge_capacity; ++index) {
        const kiln_c99_local_edge *edge = &heap->local_edges[index];
        if (edge->parent == NULL) continue;
        ++seen_rows;
        if (kiln_c99_graph_in_batch(heap, chunks, count, edge->parent) ||
            kiln_c99_graph_in_batch(heap, chunks, count, edge->key) ||
            (edge->key != NULL && kiln_c99_graph_in_batch(heap, chunks, count, edge->child)))
            return KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS;
    }
    return seen_rows == heap->local_edge_count ? KILN_C99_GRAPH_ROOTS_UNENUMERATED
                                               : KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED;
}

/* After sweep, weak owner witnesses no longer resolve to reclaimed blocks. Prune them
 * without retaining slot addresses or tracing the owners. Rehash sparsely occupied tables so
 * a long-lived candidate does not retain the high-water allocation of transient holders. */
static void kiln_c99_candidate_prune_owners(kiln_gc_heap *heap) {
    kiln_c99_candidate_group *group;
    for (group = heap->candidate_ledger_head; group != NULL; group = group->ledger_next) {
        kiln_gc_ptrset *owners = &group->incoming_owners;
        size_t index = 0;
        while (index < owners->capacity) {
            const void *witness = owners->table[index];
            uintptr_t address = (uintptr_t)witness;
            if (witness != NULL && (address > UINTPTR_MAX - sizeof(kiln_gc_block) ||
                kiln_c99_gc_lookup(heap, (const void *)(address + sizeof(kiln_gc_block))) == NULL)) {
                /* Deletion may shift a later witness into this slot. Inspect it again. */
                kiln_c99_gc_ptrset_remove(owners, witness);
            } else ++index;
        }
        if (owners->count == 0) {
            free(owners->table);
            memset(owners, 0, sizeof *owners);
        } else if (owners->capacity > 64 && owners->count <= owners->capacity / 8) {
            /* Rehash allocation failure preserves the old complete table, so is harmless. */
            (void)kiln_c99_gc_ptrset_grow(owners, owners->count);
        }
    }
    kiln_gc_ptrset *contexts = &heap->candidate_context_owners;
    for (size_t index = 0; index < contexts->capacity; ) {
        const void *witness = contexts->table[index];
        uintptr_t address = (uintptr_t)witness;
        kiln_gc_block *owner = witness == NULL || address > UINTPTR_MAX - sizeof(kiln_gc_block)
            ? NULL : kiln_c99_gc_lookup(heap, (const void *)(address + sizeof(kiln_gc_block)));
        if (witness != NULL && (owner == NULL ||
            (owner->kind & KILN_GC_KIND_MASK) != KILN_GC_COMMONJS_REQUIRE))
            kiln_c99_gc_ptrset_remove(contexts, witness);
        else ++index;
    }
    if (contexts->count == 0) {
        free(contexts->table);
        memset(contexts, 0, sizeof *contexts);
    } else if (contexts->capacity > 64 && contexts->count <= contexts->capacity / 8)
        (void)kiln_c99_gc_ptrset_grow(contexts, contexts->count);

}

/* Inspect current outside-owner layouts rather than retained slot addresses. All producer,
 * mutation and emitted cache paths use the common reference authorities. Native opaque
 * contexts and non-heap roots have their own independent inventories. */
static kiln_c99_graph_incoming_result kiln_c99_graph_scan_candidate_owners(
    const kiln_gc_heap *heap, const kiln_c99_candidate_group *group,
    kiln_nursery_chunk *const *chunks, size_t count) {
    if (group == NULL || group->ledger_heap != heap || group->incoming_failed ||
        heap->candidate_ledger_publication_failed)
        return KILN_C99_GRAPH_OUTGOING_UNPROVEN;
    for (size_t index = 0; index < group->incoming_owners.capacity; ++index) {
        const void *witness = group->incoming_owners.table[index];
        uintptr_t address = (uintptr_t)witness;
        kiln_gc_block *owner;
        kiln_c99_graph_incoming_result result;
        if (witness == NULL || address > UINTPTR_MAX - sizeof(kiln_gc_block)) continue;
        owner = kiln_c99_gc_lookup(heap, (const void *)(address + sizeof(kiln_gc_block)));
        if (owner == NULL || kiln_c99_candidate_block_group(heap, owner) == group) continue;
        result = kiln_c99_graph_outside_block(heap, chunks, count, owner);
        if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
    }
    return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
}

static kiln_c99_graph_incoming_result kiln_c99_graph_scan_outside_heap(
    const kiln_gc_heap *heap, kiln_nursery_chunk *const *chunks, size_t count) {
    const kiln_gc_block *block;
    const kiln_gc_slab *slab;
    const kiln_nursery_chunk *chunk;
    kiln_c99_graph_incoming_result result;
    result = kiln_c99_graph_scan_local_shapes(heap, chunks, count);
    if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
    for (block = heap->head; block != NULL; block = block->next) {
        if (heap->graph_proof_sides != NULL &&
            kiln_c99_gc_ptrset_contains(&heap->graph_proof_sides->large, block)) continue;
        result = kiln_c99_graph_outside_block(heap, chunks, count, block);
        if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
    }
    for (slab = heap->all_slabs; slab != NULL; slab = slab->all_next) {
        uint32_t index;
        for (index = 0; index < slab->fresh; ++index) {
            block = kiln_c99_gc_slab_slot(slab, index);
            if (block->size == KILN_GC_SLOT_FREE) continue;
            result = kiln_c99_graph_outside_block(heap, chunks, count, block);
            if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
        }
    }
    if (heap->nursery.region != NULL && heap->nursery.bump > heap->nursery.base) {
        result = kiln_c99_graph_scan_nursery_chunk(heap, chunks, count,
            (const kiln_nursery_chunk *)heap->nursery.region, heap->nursery.bump);
        if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
    }
    for (chunk = heap->nursery.full; chunk != NULL; chunk = chunk->next) {
        result = kiln_c99_graph_scan_nursery_chunk(heap, chunks, count,
                                                   chunk, chunk->limit);
        if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
    }
    for (chunk = heap->nursery.promoted; chunk != NULL; chunk = chunk->next) {
        result = kiln_c99_graph_scan_nursery_chunk(heap, chunks, count,
                                                   chunk, chunk->limit);
        if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return result;
    }
    return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
}

/* Store authorities record potential outside owners; certification inspects their current
 * layouts, the independent opaque-context inventory, and the separately enumerated roots.
 * Canonical keys and native shape indexes cannot borrow candidate storage. The full sender
 * walk remains an explicit diagnostic oracle, never hidden inside ordinary admission. */
static kiln_c99_graph_incoming_result kiln_c99_graph_scan_incoming_heap_checked(
    const kiln_gc_heap *heap, const kiln_c99_candidate_group *group,
    kiln_nursery_chunk *const *chunks, size_t count, int audit) {
    kiln_c99_graph_incoming_result ledger, full;
    if (group == NULL) return kiln_c99_graph_scan_outside_heap(heap, chunks, count);
    ledger = kiln_c99_graph_scan_context_owners(heap, chunks, count);
    if (ledger == KILN_C99_GRAPH_ROOTS_UNENUMERATED)
        ledger = kiln_c99_graph_scan_candidate_owners(heap, group, chunks, count);
    if (!audit && kiln_c99_graph_ledger_enabled()) return ledger;
    full = kiln_c99_graph_scan_outside_heap(heap, chunks, count);
    if (audit && ledger != full) {
        fputs("kiln: candidate ledger disagrees with heap scan: ", stderr);
        fputs(ledger == KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS ? "ledger alias, " :
              ledger == KILN_C99_GRAPH_ROOTS_UNENUMERATED ? "ledger clear, " : "ledger unproven, ", stderr);
        fputs(full == KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS ? "heap alias\n" :
              full == KILN_C99_GRAPH_ROOTS_UNENUMERATED ? "heap clear\n" : "heap unproven\n", stderr);
        return KILN_C99_GRAPH_OUTGOING_UNPROVEN;
    }
    return kiln_c99_graph_ledger_enabled() ? ledger : full;
}

static int kiln_c99_graph_ledger_enabled(void) {
    static _Thread_local int initialized;
    static _Thread_local int enabled;
    if (!initialized) {
        const char *option = getenv("KILN_GRAPH_LEDGER");
        enabled = option == NULL || strcmp(option, "0") != 0;
        initialized = 1;
    }
    return enabled;
}

static int kiln_c99_graph_ledger_audit_enabled(void) {
    static _Thread_local int initialized;
    static _Thread_local int enabled;
    if (!initialized) {
        const char *option = getenv("KILN_GRAPH_LEDGER_AUDIT");
        enabled = option != NULL && strcmp(option, "1") == 0;
        initialized = 1;
    }
    return enabled;
}


/* A future actor transaction may call this at its sender turn safepoint, after its consuming
 * handle has been detached. Today even a candidate with no enumerated alias is refused:
 * generated/launcher root callbacks and the machine stack still need exact, non-mutating
 * enumeration while excluding only the proof's own frames. Returning a
 * positive ownership certificate here would make the current TypeScript handle an escape hatch. */
static kiln_c99_graph_incoming_result kiln_c99_graph_incoming_probe(
    const kiln_gc_heap *heap, kiln_nursery_chunk *const *chunks, size_t count, const void *root) {
    if (!kiln_c99_graph_candidate_closed(heap, chunks, count, root))
        return KILN_C99_GRAPH_OUTGOING_UNPROVEN;
    if (kiln_c99_graph_has_c_root_alias(heap, chunks, count))
        return KILN_C99_GRAPH_C_ROOT_ALIAS;
    {
        kiln_c99_graph_incoming_result outside =
            kiln_c99_graph_scan_outside_heap(heap, chunks, count);
        if (outside != KILN_C99_GRAPH_ROOTS_UNENUMERATED) return outside;
    }
    {
        kiln_c99_graph_root_visit visit = { heap, chunks, count, 0, 0 };
        int status = kiln_realm_gc_visit_provider_roots((void *)heap,
                                                        kiln_c99_graph_visit_root,
                                                        &visit);
        /* A visitor may report both an alias and incomplete coverage. Preserve the positive
         * alias witness, then refuse any legacy callback, invalid realm or visitor failure.
         * In particular, a future safepoint must never mistake a failed provider walk for a
         * fully enumerated sender. */
        if (visit.alias) return KILN_C99_GRAPH_PROVIDER_ROOT_ALIAS;
        if (status != KILN_C99_OK || visit.invalid) return KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED;
    }
    return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
}

/* The outer frame captures the machine before the opaque request ID is resolved into a graph
 * pointer. Its anchor stays live while the younger proof frames run; scanning from the anchor
 * through this thread's stack base therefore includes all older generated and scheduler frames,
 * but never treats the proof's own candidate pointers as sender aliases. A missing stack bound
 * refuses. The register image is taken at the same boundary, before escrow loads the root. */
__attribute__((noinline)) static kiln_c99_graph_incoming_result
kiln_c99_graph_incoming_request_probe(kiln_gc_heap *heap, uint64_t request_id);

/* A successful consumer may take the manifest only after every incoming authority has
 * passed. It must be read-only and non-allocating: both the closure and ownership proof refer
 * to this uninterrupted sender lease. Failure leaves manifest ownership with the proof. */
typedef int (*kiln_c99_graph_manifest_accept)(
    void *context, const kiln_gc_heap *heap, kiln_nursery_chunk **chunks, size_t count,
    kiln_value root, const kiln_c99_graph_side_set *proof);

__attribute__((noinline)) static kiln_c99_graph_incoming_result
kiln_c99_graph_incoming_request_below_boundary(kiln_gc_heap *heap, uint64_t request_id,
                                                const uintptr_t *stack_low,
                                                const uintptr_t *stack_high,
                                                const jmp_buf *registers,
                                                int retain_clear_escrow,
                                                kiln_c99_graph_manifest_accept accept,
                                                void *accept_context) {
    kiln_c99_realm_state *realm = &heap->realm;
    kiln_nursery_chunk *chunk;
    const kiln_c99_candidate_group *group;
    kiln_nursery_chunk **chunks = NULL;
    kiln_c99_graph_side_set sides = {0};
    kiln_gc_block *block;
    kiln_c99_graph_incoming_result result = KILN_C99_GRAPH_OUTGOING_UNPROVEN;
    uint64_t candidate_id;
    size_t count = 0, offset;
    uintptr_t word;
    int sides_installed = 0;
    if (!kiln_c99_actor_isolated_request_escrow(heap, request_id)) {
        kiln_c99_actor_isolated_request_rollback(heap, request_id);
        return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    if (realm->isolated_escrow_value.tag < KILN_VALUE_STRING ||
        realm->isolated_escrow_value.bits == 0) goto done;
    block = kiln_c99_gc_lookup(heap, (const void *)(uintptr_t)realm->isolated_escrow_value.bits);
    if (block == NULL) goto done;
    if ((block->kind & KILN_GC_FLAG_NURSERY) != 0) {
        chunk = kiln_c99_nursery_chunk_of(heap, block);
        group = chunk->candidate_group;
    } else {
        kiln_c99_candidate_large *owner = kiln_c99_candidate_large_find(heap, block);
        if (owner == NULL || owner->epoch != heap->epoch) goto done;
        group = owner->group;
    }
    if (group == NULL || group->builder) goto done;
    candidate_id = group->id;
    if (candidate_id == 0) goto done;
    if (!kiln_c99_candidate_group_snapshot_owner(heap, group, &chunks, &count) ||
        !kiln_c99_graph_candidate_closed_with_sides(heap, chunks, count, block + 1, &sides,
                                                     NULL))
        goto done;
    if (heap->graph_proof_sides != NULL) goto done;
    heap->graph_proof_sides = &sides;
    sides_installed = 1;
    if (kiln_c99_graph_has_c_root_alias_for_request(heap, chunks, count, request_id)) {
        result = KILN_C99_GRAPH_C_ROOT_ALIAS;
        goto done;
    }
    if (kiln_c99_graph_machine_words_alias(heap, chunks, count, stack_low,
                                           ((uintptr_t)stack_high - (uintptr_t)stack_low) /
                                               sizeof(uintptr_t))) {
        result = KILN_C99_GRAPH_MACHINE_ALIAS;
        goto done;
    }
    for (offset = 0; offset + sizeof word <= sizeof *registers; offset += sizeof word) {
        memcpy(&word, (const char *)registers + offset, sizeof word);
        if (kiln_c99_graph_machine_words_alias(heap, chunks, count, &word, 1)) {
            result = KILN_C99_GRAPH_MACHINE_ALIAS;
            goto done;
        }
    }
    result = kiln_c99_graph_scan_incoming_heap_checked(heap, group, chunks, count,
                                                       kiln_c99_graph_ledger_audit_enabled());
    if (result != KILN_C99_GRAPH_ROOTS_UNENUMERATED) goto done;
    {
        kiln_c99_graph_root_visit visit = { heap, chunks, count, 0, 0 };
        int status = kiln_realm_gc_visit_provider_roots((void *)heap,
                                                        kiln_c99_graph_visit_root,
                                                        &visit);
        if (visit.alias) result = KILN_C99_GRAPH_PROVIDER_ROOT_ALIAS;
        else if (status != KILN_C99_OK || visit.invalid) result = KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED;
        else result = KILN_C99_GRAPH_CLEAR;
    }
    if (result == KILN_C99_GRAPH_CLEAR && retain_clear_escrow && accept != NULL) {
        if (!accept(accept_context, heap, chunks, count, realm->isolated_escrow_value,
                    &sides)) {
            result = KILN_C99_GRAPH_OUTGOING_UNPROVEN;
        } else {
            chunks = NULL; /* The accepted capsule now owns these manifest vectors. */
            sides.large_blocks = NULL;
        }
    }
done:
    if (sides_installed) heap->graph_proof_sides = NULL;
    kiln_c99_graph_side_set_dispose(&sides);
    free(chunks);
    /* Only the explicit certificate path can retain escrow after full enumeration. The
     * diagnostic probe keeps its historical refusal and restores the sender graph. */
    if (result != KILN_C99_GRAPH_CLEAR || !retain_clear_escrow) {
        if (!kiln_c99_actor_isolated_request_rollback(heap, request_id))
            return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
        if (result == KILN_C99_GRAPH_CLEAR) return KILN_C99_GRAPH_ROOTS_UNENUMERATED;
    }
    return result;
}

__attribute__((noinline)) static kiln_c99_graph_incoming_result
kiln_c99_graph_incoming_request_scan_with_manifest(
    kiln_gc_heap *heap, uint64_t request_id, int retain_clear_escrow,
    kiln_c99_graph_manifest_accept accept, void *accept_context) {
    volatile uintptr_t anchor = 0;
    uintptr_t *low = (uintptr_t *)((uintptr_t)&anchor & ~(uintptr_t)7);
    uintptr_t *high = (uintptr_t *)kiln_c99_gc_stack_base();
    jmp_buf registers;
    if (heap == NULL || request_id == 0 || high == NULL || (uintptr_t)high <= (uintptr_t)low) {
        if (heap != NULL) kiln_c99_actor_isolated_request_rollback(heap, request_id);
        return KILN_C99_GRAPH_MACHINE_BOUNDARY_UNAVAILABLE;
    }
    /* `jmp_buf` may contain padding and unsaved signal-mask bytes. Clear those before
     * `setjmp` so stale words from a prior frame do not become permanent false aliases;
     * every register that `setjmp` actually saves is overwritten afterward. */
    memset(&registers, 0, sizeof registers);
#if defined(__GNUC__) || defined(__clang__)
    __builtin_unwind_init();
#endif
    if (setjmp(registers) != 0) {
        kiln_c99_actor_isolated_request_rollback(heap, request_id);
        return KILN_C99_GRAPH_MACHINE_BOUNDARY_UNAVAILABLE;
    }
    return kiln_c99_graph_incoming_request_below_boundary(heap, request_id, low, high, &registers,
                                                            retain_clear_escrow, accept,
                                                            accept_context);
}

static kiln_c99_graph_incoming_result kiln_c99_graph_incoming_request_scan(
    kiln_gc_heap *heap, uint64_t request_id, int retain_clear_escrow) {
    return kiln_c99_graph_incoming_request_scan_with_manifest(heap, request_id,
                                                               retain_clear_escrow, NULL, NULL);
}

__attribute__((noinline)) static kiln_c99_graph_incoming_result
kiln_c99_graph_incoming_request_probe(kiln_gc_heap *heap, uint64_t request_id) {
    return kiln_c99_graph_incoming_request_scan(heap, request_id, 0);
}

/* The inverse of adoption for chunks whose entire contents have already passed an ownership
 * proof. `chunks` is an array because `next` still links every candidate into the sender's
 * `full` list. Validate the complete batch and reserve the receiver's region set before changing
 * either heap. In particular, never transfer the current bump chunk: another allocation in the
 * sender could otherwise write into memory the receiver now owns.
 *
 * This is only a storage primitive. A caller must additionally prove that every allocated block
 * in these chunks belongs to one closed graph, that no sender root or outside object reaches it,
 * and that no block has realm-local side ownership (weak-list membership, external-byte balance,
 * provider/host handle, or mutable shape). It must publish the destination root with no mutator
 * or collection between proof and this call. A TypeScript ticket is not that proof. */
static int kiln_c99_heap_move_chunks(kiln_gc_heap *from, kiln_gc_heap *to,
                                     kiln_nursery_chunk *const *chunks, size_t count) {
    size_t index;
    size_t moved_count = 0;
    size_t moved_bytes = 0;
    size_t moved_filled = 0;
    size_t sender_full_bytes = 0;
    uint64_t candidate_id = 0;
    if (from == NULL || to == NULL || from == to || (count != 0 && chunks == NULL) ||
        from->collecting || to->collecting || from->parallel_paused || to->parallel_paused ||
        from->nursery.capacity != to->nursery.capacity ||
        from->nursery.bitmap_words != to->nursery.bitmap_words) return 0;
    if (count == 0) return 1;
    if (count > SIZE_MAX - to->nursery.regions.count) return 0;
    for (index = 0; index < count; ++index) {
        kiln_nursery_chunk *chunk = chunks[index];
        size_t prior;
        int found = 0;
        if (chunk == NULL || (char *)chunk == from->nursery.region ||
            !kiln_c99_gc_ptrset_contains(&from->nursery.regions, chunk) ||
            kiln_c99_gc_ptrset_contains(&to->nursery.regions, chunk) ||
            chunk->bitmap_words != from->nursery.bitmap_words ||
            chunk->limit < chunk->base ||
            chunk->limit > (char *)chunk + from->nursery.capacity) return 0;
        if (chunk->candidate_id == 0 || chunk->candidate_epoch != from->epoch) return 0;
        if (candidate_id != 0 && candidate_id != chunk->candidate_id) return 0;
        candidate_id = chunk->candidate_id;
        found = kiln_c99_nursery_full_contains(&from->nursery, chunk);
        if (found) {
            size_t filled = (size_t)(chunk->limit - chunk->base);
            if (sender_full_bytes > SIZE_MAX - filled) return 0;
            sender_full_bytes += filled;
        }
        if (!found || !kiln_c99_chunk_sealed_for_move(&from->nursery, chunk)) return 0;
        for (prior = 0; prior < index; ++prior) if (chunks[prior] == chunk) return 0;
        if (moved_count > SIZE_MAX - chunk->alloc_count ||
            moved_bytes > SIZE_MAX - chunk->alloc_bytes ||
            moved_filled > SIZE_MAX - (size_t)(chunk->limit - chunk->base)) return 0;
        moved_count += chunk->alloc_count;
        moved_bytes += chunk->alloc_bytes;
        moved_filled += (size_t)(chunk->limit - chunk->base);
    }
    if (moved_count > from->count || moved_bytes > from->bytes ||
        sender_full_bytes > from->nursery.full_bytes ||
        moved_count > SIZE_MAX - to->count || moved_bytes > SIZE_MAX - to->bytes ||
        moved_bytes > SIZE_MAX - to->since_collection ||
        moved_filled > SIZE_MAX - to->nursery.full_bytes ||
        (to->nursery.regions.count + count) > SIZE_MAX / 2) return 0;
    if ((to->nursery.regions.count + count) * 2 > to->nursery.regions.capacity &&
        kiln_c99_gc_ptrset_grow(&to->nursery.regions, to->nursery.regions.count + count) !=
            KILN_C99_OK) return 0;

    /* All validation and possible allocation have completed. The sender and receiver must both
     * be stopped at the caller's safepoint until the destination root is published. */
    for (index = 0; index < count; ++index) {
        kiln_nursery_chunk *chunk = chunks[index];
        kiln_c99_nursery_full_remove(&from->nursery, chunk);
        if (chunk->candidate_state != 0) {
            size_t filled = (size_t)(chunk->limit - chunk->base);
            if (filled > from->nursery.held_bytes) abort();
            from->nursery.held_bytes -= filled;
        }
        from->nursery.full_bytes -= (size_t)(chunk->limit - chunk->base);
        kiln_c99_gc_ptrset_remove(&from->nursery.regions, chunk);
        (void)kiln_c99_gc_ptrset_insert(&to->nursery.regions, chunk);
        if (to->nursery.lo == NULL || (char *)chunk < to->nursery.lo)
            to->nursery.lo = (char *)chunk;
        if (to->nursery.hi == NULL || (char *)chunk + to->nursery.capacity > to->nursery.hi)
            to->nursery.hi = (char *)chunk + to->nursery.capacity;
        kiln_c99_gc_bound(to, chunk, to->nursery.capacity);
        chunk->mark_epoch = 0;
        kiln_c99_candidate_chunk_clear(chunk);
        chunk->marked_count = 0;
        chunk->marked_bytes = 0;
        kiln_c99_nursery_full_push(&to->nursery, chunk);
    }
    from->count -= moved_count;
    from->bytes -= moved_bytes;
    to->count += moved_count;
    to->bytes += moved_bytes;
    to->since_collection += moved_bytes;
    to->nursery.full_bytes += moved_filled;
    return 1;
}

enum {
    KILN_C99_GRAPH_INBOX_EMPTY = 0,
    KILN_C99_GRAPH_INBOX_RESERVED = 1,
    KILN_C99_GRAPH_INBOX_PUBLISHED = 2
};

/* Reserve the receiver's native inbox slot and all region-set capacity before sender unlink.
 * This is one slot, distinct from the actor's JS mailbox grant. A future coordinator must
 * reserve both while the receiver mutator is paused, and keep it paused through publication. */
static int kiln_c99_graph_receiver_reserve_id(kiln_gc_heap *to, size_t chunk_count,
                                              uint64_t reservation_id) {
    kiln_c99_realm_state *realm;
    size_t wanted;
    if (to == NULL || reservation_id == 0 || to->collecting ||
        to->parallel_paused || chunk_count > SIZE_MAX - to->nursery.regions.count) return 0;
    realm = &to->realm;
    wanted = to->nursery.regions.count + chunk_count;
    if (realm->graph_inbox_state != KILN_C99_GRAPH_INBOX_EMPTY ||
        realm->graph_inbox_root.tag != KILN_VALUE_UNDEFINED ||
        wanted > SIZE_MAX / 2) return 0;
    if (wanted * 2 > to->nursery.regions.capacity &&
        kiln_c99_gc_ptrset_grow(&to->nursery.regions, wanted) != KILN_C99_OK) return 0;
    realm->graph_inbox_reservation_id = reservation_id;
    realm->graph_inbox_reserved_chunks = chunk_count;
    realm->graph_inbox_region_count = to->nursery.regions.count;
    realm->graph_inbox_epoch = to->epoch;
    realm->graph_inbox_root = kiln_c99_undefined();
    realm->graph_inbox_state = KILN_C99_GRAPH_INBOX_RESERVED;
    return 1;
}

static int kiln_c99_graph_receiver_reserve(kiln_gc_heap *to, size_t chunk_count,
                                           uint64_t *out_id) {
    uint64_t id;
    if (to == NULL || out_id == NULL || to->realm.graph_inbox_next_id == UINT64_MAX) return 0;
    id = to->realm.graph_inbox_next_id + 1;
    if (!kiln_c99_graph_receiver_reserve_id(to, chunk_count, id)) return 0;
    to->realm.graph_inbox_next_id = id;
    *out_id = id;
    return 1;
}

/* Refusal before unlink releases the reserved slot. A published root cannot be cleared by
 * rollback: the receiver must first install it in a durable actor inbox and own its lifetime. */
static int kiln_c99_graph_receiver_rollback(kiln_gc_heap *to, uint64_t id) {
    kiln_c99_realm_state *realm;
    if (to == NULL || id == 0) return 0;
    realm = &to->realm;
    if (realm->graph_inbox_state != KILN_C99_GRAPH_INBOX_RESERVED ||
        realm->graph_inbox_reservation_id != id) return 0;
    realm->graph_inbox_state = KILN_C99_GRAPH_INBOX_EMPTY;
    realm->graph_inbox_reservation_id = 0;
    realm->graph_inbox_reserved_chunks = 0;
    realm->graph_inbox_region_count = 0;
    realm->graph_inbox_epoch = 0;
    realm->graph_inbox_root = kiln_c99_undefined();
    return 1;
}

/* Implemented in graph_capsule.c after the heap's closure and reservation primitives. */
static int kiln_c99_graph_capsule_paused_control(uint32_t kind, uint64_t first,
                                                 uint64_t second, uint64_t third);

/* Called only through the claimed scheduler lane's control callback. Kinds 1 and 2 remain
 * refusal probes. Positive commands are host-only and carry authenticated numeric IDs; the
 * pair coordinator keeps both mutators stopped through sender unlink and receiver adoption. */
int kiln_c99_graph_paused_control(uint32_t kind, uint64_t first,
                                  uint64_t second, uint64_t third) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || first == 0) return 3;
    switch (kind) {
    case 1: {
        kiln_c99_graph_incoming_result result = kiln_c99_graph_incoming_request_probe(heap, first);
        switch (result) {
        case KILN_C99_GRAPH_C_ROOT_ALIAS:
        case KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS:
        case KILN_C99_GRAPH_PROVIDER_ROOT_ALIAS:
        case KILN_C99_GRAPH_MACHINE_ALIAS:
            return 1;
        case KILN_C99_GRAPH_MACHINE_BOUNDARY_UNAVAILABLE:
            return 3;
        default:
            return 2;
        }
    }
    case 2:
        if (!kiln_c99_graph_receiver_reserve_id(heap, 1, first)) return 3;
        return kiln_c99_graph_receiver_rollback(heap, first) ? 0 : 3;
    default:
        return kiln_c99_graph_capsule_paused_control(kind, first, second, third);
    }
}

/* Low-level publication primitive, not an ownership certificate. The caller must have already
 * proved zero sender aliases and paused both realm mutators. Every check and possible allocation
 * happens before `kiln_c99_heap_move_chunks` unlinks anything; after it returns true, storing
 * the root in the reserved slot is infallible and occurs before either realm may resume. */
static int kiln_c99_graph_receiver_move_and_publish(kiln_gc_heap *from, kiln_gc_heap *to,
    kiln_nursery_chunk *const *chunks, size_t count, kiln_value root, uint64_t reservation_id) {
    kiln_c99_realm_state *realm;
    const kiln_gc_block *block;
    const void *payload;
    uint32_t kind;
    if (from == NULL || to == NULL || chunks == NULL || count == 0 || reservation_id == 0)
        return 0;
    realm = &to->realm;
    if (realm->graph_inbox_state != KILN_C99_GRAPH_INBOX_RESERVED ||
        realm->graph_inbox_reservation_id != reservation_id ||
        realm->graph_inbox_root.tag != KILN_VALUE_UNDEFINED ||
        realm->graph_inbox_reserved_chunks != count ||
        realm->graph_inbox_region_count != to->nursery.regions.count ||
        realm->graph_inbox_epoch != to->epoch ||
        root.bits == 0 || !kiln_c99_graph_value_in_batch(from, chunks, count, &root)) return 0;
    block = kiln_c99_gc_lookup_interior(from, (uintptr_t)root.bits);
    if (block == NULL) return 0;
    payload = block + 1;
    kind = block->kind & KILN_GC_KIND_MASK;
    if (root.tag == KILN_VALUE_CELL) {
        if (kind != KILN_GC_CELL || payload != (const void *)(uintptr_t)root.bits) return 0;
    } else if (root.tag == KILN_VALUE_STRING) {
        const kiln_string *string = (const kiln_string *)payload;
        /* Boundary strings name their bytes, not the string header. A slice's bytes name its
         * parent, so admit only an inline root until slice identity has a separate mover. */
        if (kind != KILN_GC_STRING || !kiln_c99_graph_inline_string(block) ||
            string->data != (const void *)(uintptr_t)root.bits ||
            string->unit_len != root.len) return 0;
    } else {
        return 0;
    }
    if (!kiln_c99_graph_candidate_closed(from, chunks, count, payload)) return 0;
    if (!kiln_c99_heap_move_chunks(from, to, chunks, count)) return 0;
    realm->graph_inbox_root = root;
    realm->graph_inbox_state = KILN_C99_GRAPH_INBOX_PUBLISHED;
    return 1;
}

/* ---- Worker arenas (ADR 0069; parallel issue 08) --------------------------------------------
 *
 * A tier-3 callback allocates: an object literal, `{ ...r, total }`, an array literal, a string
 * built with `+` or a template. Its clone runs on the pool's threads while the realm thread is
 * blocked in the call, and the realm's allocator is the realm's -- its nursery pointers, its
 * counters, its collector -- so a worker never touches it. Each thread that runs a chunk of the
 * call claims an *arena* instead (`kiln_c99_par_arena_claim`, one per thread per call, found again
 * for the thread's next chunk), and bumps through chunks of the nursery's kind that it makes for
 * itself (`kiln_c99_par_alloc`). Every block it writes is laid out exactly as the nursery lays
 * one out; nothing in the realm's heap changes while the call runs.
 *
 * The collector is paused from `kiln_c99_par_arenas_open` to `kiln_c99_par_arenas_join` (the
 * call's own thread does not allocate in between, and a pool thread cannot reach the realm's
 * allocator at all; `kiln_c99_gc_collect` aborts if either were ever wrong). The join, on the
 * realm thread, seals each arena's last chunk and adopts every chunk into the heap in one step
 * (above). When the call is answered sequentially instead -- a worker declined, or an arena ran
 * out of its budget -- nothing the arenas hold is reachable from anything, and they are freed
 * without being adopted.
 *
 * An arena's budget (`KILN_PARALLEL_ARENA_KIB`, 256 MiB by default) bounds the chunks one thread
 * may take in one call. A worker that would pass it declines like any other worker, and the call
 * then runs the sequential method from its first element: memory pressure is never a failure,
 * and the answer is `map`'s by the same argument as every other decline (nothing the pool did is
 * observable). */
struct kiln_c99_par_arena {
    /* The claiming thread's anchor (the address of its `kiln_c99_no_write_depth`), NULL until
     * the slot is claimed. */
    const void *owner;
    /* The frozen heap: only its chunk size and bitmap width are read. */
    const kiln_gc_heap *heap;
    /* Sealed chunks, newest first, and the one being filled. */
    kiln_nursery_chunk *filled;
    kiln_nursery_chunk *current;
    char *bump;
    char *limit;
    size_t count;
    size_t bytes;
    /* Region bytes taken, against `budget`. */
    size_t taken;
    size_t budget;
    /* Zero for the parallel worker arena; otherwise all its chunks are one graph candidate. */
    uint64_t candidate_id;
    kiln_c99_candidate_group *candidate_group;
};

static uint64_t kiln_c99_graph_candidate_next_id;

static uint64_t kiln_c99_graph_candidate_new_id(void) {
    uint64_t previous = __atomic_load_n(&kiln_c99_graph_candidate_next_id, __ATOMIC_RELAXED);
    do {
        /* Never recycle an ID while an older candidate may still be live. */
        if (previous == UINT64_MAX) return 0;
    } while (!__atomic_compare_exchange_n(&kiln_c99_graph_candidate_next_id, &previous,
                                          previous + 1, 0, __ATOMIC_RELAXED, __ATOMIC_RELAXED));
    return previous + 1;
}

/* A dedicated candidate allocation stream. Its caller must keep collection paused while
 * building and publishing the graph into the sender heap. Nothing in this helper claims the
 * graph is closed or excludes sender roots; those checks precede a future move call. */
static int kiln_c99_graph_candidate_open(kiln_c99_par_arena *arena, const kiln_gc_heap *heap,
                                         size_t budget) {
    uint64_t id;
    if (arena == NULL || heap == NULL || heap->nursery.capacity == 0 || budget == 0) return 0;
    id = kiln_c99_graph_candidate_new_id();
    if (id == 0) return 0;
    memset(arena, 0, sizeof *arena);
    arena->candidate_group = kiln_c99_candidate_group_new(id);
    if (arena->candidate_group == NULL) return 0;
    arena->heap = heap;
    arena->budget = budget;
    arena->candidate_id = id;
    return 1;
}

/* Close the builder, not its storage. Sealed chunks retain the manifest until collection,
 * adoption or arena disposal removes them. No allocation may follow this boundary. */
static void kiln_c99_graph_candidate_close(kiln_c99_par_arena *arena) {
    if (arena->current != NULL) abort();
    kiln_c99_candidate_group_close(arena->candidate_group);
    arena->candidate_group = NULL;
}

/* A source-language factory can allocate through every C path, including the inlined hot bump
 * and the multi-string run. Give it the ordinary nursery's current chunk, but only after
 * sealing the preceding turn's allocations. This tags exact allocation provenance without
 * treating any escaped object as movable. Standalone blocks carry cold owner metadata;
 * slab and tenured allocations remain outside the candidate and fail outgoing closure.
 * A nested factory keeps working as JavaScript but cannot make a separate candidate. */
static int kiln_c99_actor_candidate_begin(kiln_gc_heap *heap) {
    uint64_t id;
    if (heap == NULL || heap->fallback || !heap->nursery.active || heap->nursery.region == NULL ||
        heap->actor_candidate_id != 0 || heap->actor_candidate_pending_id != 0) return 0;
    id = kiln_c99_graph_candidate_new_id();
    if (id == 0) return 0;
    heap->actor_candidate_group = kiln_c99_candidate_group_new(id);
    if (heap->actor_candidate_group == NULL) return 0;
    if (heap->candidate_ledger_groups == SIZE_MAX) {
        kiln_c99_candidate_group_close(heap->actor_candidate_group);
        heap->actor_candidate_group = NULL;
        return 0;
    }
    heap->actor_candidate_group->ledger_heap = heap;
    heap->actor_candidate_group->ledger_next = heap->candidate_ledger_head;
    if (heap->candidate_ledger_head != NULL)
        heap->candidate_ledger_head->ledger_previous = heap->actor_candidate_group;
    heap->candidate_ledger_head = heap->actor_candidate_group;
    ++heap->candidate_ledger_groups;
    heap->actor_candidate_id = id;
    heap->actor_candidate_collected = 0;
    if (heap->nursery.bump > heap->nursery.base) kiln_c99_nursery_turn_chunk(heap);
    if (!heap->nursery.active || heap->nursery.region == NULL) {
        heap->actor_candidate_id = 0;
        heap->actor_candidate_collected = 0;
        kiln_c99_candidate_group_close(heap->actor_candidate_group);
        heap->actor_candidate_group = NULL;
        return 0;
    }
    if (((kiln_nursery_chunk *)heap->nursery.region)->candidate_group == NULL)
        kiln_c99_candidate_group_add(heap->actor_candidate_group,
                                       (kiln_nursery_chunk *)heap->nursery.region);
    return 1;
}

/* Seal the factory's final chunk before any subsequent actor-registry allocation can enter it.
 * A throw abandons transfer eligibility, but cannot discard the blocks: user callbacks may
 * already have exposed some of them outside the factory. Collection may run during the scope
 * after the nursery runs out of chunks or exceeds its 64 MiB defer ceiling. Such a collection
 * invalidates the whole candidate ID, including promoted chunks, at this boundary. */
static void kiln_c99_actor_candidate_finish(kiln_gc_heap *heap, int sealed);

static int kiln_c99_actor_candidate_end(kiln_gc_heap *heap, int completed) {
    uint64_t id;
    kiln_nursery_chunk *chunk;
    kiln_c99_candidate_group *group;
    if (heap == NULL) return 0;
    if (heap->actor_candidate_id == 0) {
        if (!completed && heap->actor_candidate_pending_id != 0) {
            kiln_c99_actor_candidate_finish(heap, 0);
            return 1;
        }
        return 0;
    }
    id = heap->actor_candidate_id;
    group = heap->actor_candidate_group;
    if (heap->nursery.active && heap->nursery.bump > heap->nursery.base)
        kiln_c99_nursery_turn_chunk(heap);
    if (heap->nursery.region != NULL) {
        chunk = (kiln_nursery_chunk *)heap->nursery.region;
        if (chunk->candidate_id == id) kiln_c99_candidate_chunk_clear(chunk);
    }
    heap->actor_candidate_id = 0;
    if (!completed || heap->actor_candidate_collected) {
        while (group->head != NULL) kiln_c99_candidate_chunk_clear(group->head);
        while (group->large_head != NULL)
            kiln_c99_candidate_large_remove(heap, group->large_head->block);
        heap->actor_candidate_group = NULL;
        kiln_c99_candidate_group_close(group);
    } else {
        for (chunk = group->head; chunk != NULL; chunk = chunk->candidate_next) {
            if (!kiln_c99_nursery_full_contains(&heap->nursery, chunk)) abort();
            chunk->candidate_state = 1;
            heap->nursery.held_bytes += (size_t)(chunk->limit - chunk->base);
        }
        heap->actor_candidate_pending_id = id;
    }
    heap->actor_candidate_collected = 0;
    return 1;
}

/* The provisional hold spans only the generated C bridge from CANDIDATE_END to ISOLATE.
 * The latter either proves closure and seals it or releases it. A sealed group stays intact
 * only while the collector sees live blocks; an abandoned handle costs no permanent space. */
static void kiln_c99_actor_candidate_finish(kiln_gc_heap *heap, int sealed) {
    kiln_nursery_chunk *chunk;
    kiln_c99_candidate_group *group;
    uint64_t id;
    if (heap == NULL || (id = heap->actor_candidate_pending_id) == 0) return;
    group = heap->actor_candidate_group;
    if (group == NULL || group->id != id) abort();
    chunk = group->head;
    while (chunk != NULL) {
        kiln_nursery_chunk *next = chunk->candidate_next;
        if (chunk->candidate_state != 1) abort();
        if (!sealed) {
            size_t filled = (size_t)(chunk->limit - chunk->base);
            if (filled > heap->nursery.held_bytes) abort();
            heap->nursery.held_bytes -= filled;
            kiln_c99_candidate_chunk_clear(chunk);
        } else {
            chunk->candidate_state = 2;
        }
        chunk = next;
    }
    if (!sealed) {
        while (group->large_head != NULL)
            kiln_c99_candidate_large_remove(heap, group->large_head->block);
    }
    heap->actor_candidate_pending_id = 0;
    heap->actor_candidate_group = NULL;
    kiln_c99_candidate_group_close(group);
}

/* Read once per process: `KILN_PARALLEL_ARENA_KIB`, else 256 MiB. 0 not yet read. */
static size_t kiln_c99_par_arena_budget_bytes;

static size_t kiln_c99_par_arena_budget(void) {
    size_t budget = __atomic_load_n(&kiln_c99_par_arena_budget_bytes, __ATOMIC_RELAXED);
    if (KILN_UNLIKELY(budget == 0)) {
        const char *setting = getenv("KILN_PARALLEL_ARENA_KIB");
        budget = (size_t)256 * 1024 * 1024;
        if (setting != NULL && setting[0] >= '0' && setting[0] <= '9') {
            size_t kib = (size_t)strtoul(setting, NULL, 10);
            if (kib > 0 && kib <= (size_t)64 * 1024 * 1024) budget = kib * 1024;
        }
        __atomic_store_n(&kiln_c99_par_arena_budget_bytes, budget, __ATOMIC_RELAXED);
    }
    return budget;
}

/* Seal the chunk being filled onto `filled` (an empty one is simply freed). */
static void kiln_c99_par_arena_seal(kiln_c99_par_arena *arena) {
    kiln_nursery_chunk *chunk = arena->current;
    if (chunk == NULL) return;
    arena->current = NULL;
    if (arena->bump == chunk->base) {
        kiln_c99_candidate_group_remove(chunk);
        kiln_c99_gc_aligned_free(chunk);
        return;
    }
    chunk->limit = arena->bump;
    chunk->live_count = 0;
    chunk->alloc_count = arena->count;
    chunk->alloc_bytes = arena->bytes;
    chunk->next = arena->filled;
    arena->filled = chunk;
}

/* A new chunk to bump through, or 0 when the budget or memory says no. */
static int kiln_c99_par_arena_refill(kiln_c99_par_arena *arena) {
    size_t capacity = arena->heap->nursery.capacity;
    kiln_nursery_chunk *chunk;
    if (arena->candidate_id != 0 && arena->candidate_group == NULL) return 0;
    kiln_c99_par_arena_seal(arena);
    if (arena->taken + capacity > arena->budget) return 0;
    chunk = kiln_c99_arena_chunk(capacity, arena->heap->nursery.bitmap_words);
    if (chunk == NULL) return 0;
    if (arena->candidate_id != 0)
        kiln_c99_candidate_group_add(arena->candidate_group, chunk);
    arena->taken += capacity;
    arena->current = chunk;
    arena->bump = chunk->base;
    arena->limit = (char *)chunk + capacity;
    arena->count = 0;
    arena->bytes = 0;
    return 1;
}

/* A block of `kind` with room for `size` payload bytes, laid out as the nursery's bump lays one
 * out -- header, allocation bit, the finaliser bit a kind finalised from birth gets -- or NULL:
 * a block the nursery would not take (a quarter of a chunk or more), or the budget spent. The
 * payload is not zeroed; no collection can run before the builder has filled it. */
static void *kiln_c99_par_alloc(kiln_c99_par_arena *arena, uint32_t kind, size_t size) {
    size_t payload = size == 0 ? 1 : size;
    size_t total = (sizeof(kiln_gc_block) + payload + 15) & ~((size_t)15);
    kiln_nursery_chunk *chunk;
    kiln_gc_block *block;
    size_t bit_idx;
    if (arena == NULL || sizeof(kiln_gc_block) + payload > (arena->heap->nursery.capacity >> 2)) {
        return NULL;
    }
    if (KILN_UNLIKELY(arena->current == NULL || arena->bump + total > arena->limit)) {
        if (!kiln_c99_par_arena_refill(arena)) return NULL;
    }
    chunk = arena->current;
    block = (kiln_gc_block *)arena->bump;
    bit_idx = (size_t)(arena->bump - chunk->base) >> 4;
    chunk->bitmap[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
    if (kiln_c99_gc_kind_finalized_at_alloc(kind)) {
        chunk->finals[bit_idx >> 6] |= ((uint64_t)1 << (bit_idx & 63));
    }
    arena->bump += total;
    block->kind = kind | KILN_GC_FLAG_NURSERY;
    block->mark = 0;
    block->size = size;
    block->prev = NULL;
    block->next = NULL;
    arena->count += 1;
    arena->bytes += size;
    return block + 1;
}

/* On the realm thread, just before the pool: a slot per thread that can run a chunk (the pool's
 * and the calling thread, which runs the cost model's probe), and the heap paused. 0 when the heap
 * has no nursery to take chunks of its kind, or no memory: the site then runs sequentially. */
static int kiln_c99_par_arenas_open(kiln_c99_par_env *env, size_t threads) {
    kiln_gc_heap *heap = (kiln_gc_heap *)env->heap;
    size_t capacity = threads + 1;
    if (heap == NULL || heap->nursery.capacity == 0 ||
        heap->nursery.bitmap_words == 0) {
        return 0;
    }
    if (capacity < 2) capacity = 2;
    if (capacity > 4096) capacity = 4096;
    env->arenas = (struct kiln_c99_par_arena *)calloc(capacity, sizeof(struct kiln_c99_par_arena));
    if (env->arenas == NULL) return 0;
    env->arena_capacity = (uint32_t)capacity;
    env->arena_count = 0;
    heap->parallel_paused += 1;
    return 1;
}

/* On a pool thread, at the start of each chunk: this thread's arena for the call -- the one it
 * claimed at an earlier chunk, else a fresh slot. NULL when the site gave its workers no arenas,
 * or every slot is taken (a thread the table was not sized for); the worker then declines at its
 * first allocation. */
static kiln_c99_par_arena *kiln_c99_par_arena_claim(kiln_c99_par_env *env) {
    const void *me = (const void *)&kiln_c99_no_write_depth;
    uint32_t seen;
    uint32_t index;
    kiln_c99_par_arena *arena;
    if (env->arenas == NULL) return NULL;
    seen = __atomic_load_n(&env->arena_count, __ATOMIC_ACQUIRE);
    if (seen > env->arena_capacity) seen = env->arena_capacity;
    for (index = 0; index < seen; ++index) {
        if (__atomic_load_n(&env->arenas[index].owner, __ATOMIC_ACQUIRE) == me) {
            return &env->arenas[index];
        }
    }
    index = __atomic_fetch_add(&env->arena_count, 1, __ATOMIC_ACQ_REL);
    if (index >= env->arena_capacity) return NULL;
    arena = &env->arenas[index];
    arena->heap = (const kiln_gc_heap *)env->heap;
    arena->budget = kiln_c99_par_arena_budget();
    __atomic_store_n(&arena->owner, me, __ATOMIC_RELEASE);
    return arena;
}

/* `KILN_PARALLEL_TRACE=1`: one line on stderr per join -- how many arenas the call claimed, how
 * many chunks and bytes they held, whether the heap adopted them -- so a probe or a benchmark can
 * see which calls really ran in arenas. Read once: 0 not yet read, 1 off, 2 on. */
static int kiln_c99_par_trace_state;

static void kiln_c99_par_trace(uint32_t arenas, size_t chunks, size_t bytes, int kept, int adopted) {
    int state = __atomic_load_n(&kiln_c99_par_trace_state, __ATOMIC_RELAXED);
    if (KILN_UNLIKELY(state == 0)) {
        const char *setting = getenv("KILN_PARALLEL_TRACE");
        state = setting != NULL && setting[0] == '1' ? 2 : 1;
        __atomic_store_n(&kiln_c99_par_trace_state, state, __ATOMIC_RELAXED);
    }
    if (KILN_LIKELY(state != 2)) return;
    kiln_c99_gc_trace_text("kiln parallel: arenas ");
    kiln_c99_gc_trace_count(arenas);
    kiln_c99_gc_trace_text(" chunks ");
    kiln_c99_gc_trace_count(chunks);
    kiln_c99_gc_trace_text(" bytes ");
    kiln_c99_gc_trace_count(bytes);
    kiln_c99_gc_trace_text(adopted ? " adopted\n" : kept ? " adoption refused\n" : " freed (sequential)\n");
}

/* On the realm thread, once the pool has returned: every arena sealed, the heap unpaused, and
 * with `keep` every chunk adopted (1) -- the answer the workers built is the heap's. Without
 * `keep`, or when adoption cannot get memory, the chunks are freed and 0 says the site must not
 * publish what the workers wrote. */
static int kiln_c99_par_arenas_join(kiln_c99_par_env *env, int keep) {
    kiln_gc_heap *heap = (kiln_gc_heap *)env->heap;
    kiln_nursery_chunk *chunks = NULL;
    uint32_t count;
    uint32_t index;
    size_t chunk_count = 0;
    size_t bytes = 0;
    int adopted;
    if (env->arenas == NULL) return keep;
    count = env->arena_count < env->arena_capacity ? env->arena_count : env->arena_capacity;
    for (index = 0; index < count; ++index) {
        kiln_c99_par_arena *arena = &env->arenas[index];
        kiln_c99_par_arena_seal(arena);
        while (arena->filled != NULL) {
            kiln_nursery_chunk *chunk = arena->filled;
            arena->filled = chunk->next;
            chunk->next = chunks;
            chunks = chunk;
            chunk_count += 1;
            bytes += chunk->alloc_bytes;
        }
    }
    free(env->arenas);
    env->arenas = NULL;
    heap->parallel_paused -= 1;
    adopted = keep && kiln_c99_heap_adopt_chunks(heap, chunks);
    kiln_c99_par_trace(count, chunk_count, bytes, keep, adopted);
    if (adopted) return 1;
    kiln_c99_arena_chunks_free(chunks);
    return 0;
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
static const kiln_string *kiln_c99_intern_shared_key(const kiln_string *string);
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
    if (kiln_c99_key_is_symbol(string)) hash ^= UINT64_C(0x6a09e667f3bcc909);
    if (string->flags & KILN_STRING_PRIVATE_KEY) hash ^= UINT64_C(0xbb67ae8584caa73b);
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
        if (entry->hash == hash &&
            kiln_c99_key_domain(entry->string) == kiln_c99_key_domain(string) &&
            kiln_string_equal(entry->string, string))
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
    /* Constant keys live in the process table. An idle realm usually has only a few dynamic
     * keys; begin with room for 32 at the table's half-full growth threshold, then grow as
     * ordinary object/JSON workloads need it. */
    size_t capacity = old_capacity == 0 ? 64 : old_capacity * 2;
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
        if (entry->hash == hash &&
            kiln_c99_key_domain(candidate) == kiln_c99_key_domain(string) &&
            kiln_string_equal(candidate, string)) return candidate;
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
        /* Sequentially consistent, as every unpublish is (see `kiln_c99_slot_claim`); the old
         * table is freed once every thread that could be probing it has passed a turn boundary.
         * Lock-free probes happen only on threads running a realm: registration probes under
         * the lock, and refuses once a heap is live. */
        __atomic_store_n(&process->keys, grown, __ATOMIC_SEQ_CST);
        if (table != NULL) (void)kiln_realm_retire(table);
        table = grown;
    }
    /* `storage` counts the header; the bytes follow it, then a NUL for `%s` diagnostics. */
    copy = (kiln_string *)malloc(storage + 1);
    if (copy == NULL) return NULL;
    copy->unit_len = string->unit_len;
    copy->flags = enc | kiln_c99_key_domain(string);
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
    /* Canonical keys belong to the realm's independent key table, never to an exclusive
     * factory graph. This applies to large keys too, not just slab-sized strings. Shape
     * metadata retains only these canonical keys and native shape allocations. */
    copy = (kiln_string *)kiln_c99_gc_alloc_tenured_ex(KILN_GC_STRING, total, 0);
    if (copy == NULL) return NULL;
    copy->unit_len = view->unit_len;
    copy->flags = enc | kiln_c99_key_domain(view);
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
    kiln_string view = {(uint32_t)len, kiln_c99_fixed_key_flags(bytes, len), bytes};
    return kiln_c99_interned_key_in(heap, &view);
}

static const kiln_string *kiln_c99_interned(const char *bytes, size_t len) {
    return kiln_c99_interned_in(kiln_c99_gc_heap(), bytes, len);
}

static const kiln_string *kiln_c99_intern_static(const char *bytes, size_t len) {
    if (len > UINT32_MAX) return NULL;
    /* Fixed runtime names use the same key domain when stored and when cached. In particular,
     * the HTTP listener fast path caches @@node:events and must find the private property that
     * EventEmitter.on created, or it will silently drop response end events. */
    kiln_string view = {(uint32_t)len, kiln_c99_fixed_key_flags(bytes, len), bytes};
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
    if (kiln_c99_no_write()) return kiln_c99_interned_key_in(kiln_c99_gc_heap(), string);
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
    if (kiln_c99_no_write()) return kiln_c99_interned_key_in(kiln_c99_gc_heap(), string);
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

/* A key that means the same in every realm although no unit names it: a registered symbol's
 * (`Symbol.for`, spelt `@@symbol-for:` and the registry key). It joins the shared table whatever
 * other realms are alive, which is sound where `kiln_c99_intern_static_key` below is not because
 * no realm makes this spelling any other way: only `Symbol.for` spells a `@@symbol-for:` key, and
 * it spells it here. The calling realm's own table is still consulted first, so a realm that holds
 * the spelling already (a program string that happened to spell one) keeps its own pointer. The
 * table only grows, like the registries it mirrors, which live as long as their realms. */
const kiln_string *kiln_c99_intern_shared_key(const kiln_string *string) {
    kiln_gc_heap *heap;
    uint64_t hash;
    const kiln_string *shared;
    kiln_shared_state *process;
    if (string == NULL || string->unit_len == 0) return kiln_c99_key_empty;
    if (kiln_c99_no_write()) return kiln_c99_interned_key_in(kiln_c99_gc_heap(), string);
    hash = kiln_c99_key_hash(string);
    shared = kiln_c99_shared_key_probe(string, hash);
    if (shared != NULL) return shared;
    heap = kiln_c99_gc_heap();
    if (heap == NULL) return NULL;
    if (kiln_c99_key_probe(&heap->keys, string, hash) == NULL) {
        process = kiln_c99_process();
        kiln_c99_process_lock(process);
        shared = kiln_c99_shared_key_probe(string, hash);
        if (shared == NULL) shared = kiln_c99_shared_key_insert(process, string, hash);
        kiln_c99_process_unlock(process);
        if (shared != NULL) return shared;
    }
    return kiln_c99_intern_pinned_key(string);
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
    if (kiln_c99_no_write()) return kiln_c99_interned_key_in(kiln_c99_gc_heap(), string);
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
    if (!kiln_c99_register_static_keys_locked(process, &key, NULL, 1)) return 0;
    /* The C byte-name adapters classify fixed @@ names as either private slots or well-known
     * symbols, while generated TypeScript can also use the same spelling as an ordinary string
     * key. Register both domains before any realm exists. They remain distinct canonical keys;
     * registering only the ordinary spelling made the C adapter build a realm-local shape and
     * every later property of that bootstrap object stayed local too. */
    view.flags = kiln_c99_fixed_key_flags(bytes, len);
    return view.flags == KILN_STRING_LATIN1 ||
           kiln_c99_register_static_keys_locked(process, &key, NULL, 1);
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
        if (block == NULL || !kiln_c99_gc_is_marked(heap, block)) {
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

static void kiln_c99_gc_finalize(kiln_gc_heap *heap, kiln_gc_block *block);

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
            kiln_c99_gc_finalize(heap, block);
            block->kind |= KILN_GC_FLAG_DEAD;
            kiln_c99_nursery_note_release(heap, block);
            heap->freed_blocks += 1;
            heap->freed_bytes += block->size;
            heap->count -= 1;
            heap->bytes -= block->size;
        }
        return;
    }
    kiln_c99_gc_finalize(heap, block);
    heap->freed_blocks += 1;
    heap->freed_bytes += block->size;
    if (block->kind & KILN_GC_FLAG_LARGE) {
        kiln_c99_gc_unlink(heap, block);
        kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        kiln_c99_candidate_large_remove(heap, block);
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
/* A typed buffer's backing block follows its wrapper. The sender loses its moved-byte balance;
 * the receiver gains it. A buffer allocated locally retains the ordinary allocation-debt rule
 * in the receiving heap. No other block kind owns external bytes here. */
static void kiln_c99_gc_splice_external(kiln_gc_heap *from_heap, kiln_gc_heap *to_heap,
                                       kiln_gc_block *block) {
    kiln_typed_buffer *buffer;
    size_t length;
    if ((block->kind & KILN_GC_KIND_MASK) != KILN_GC_TYPED_BUFFER) return;
    buffer = (kiln_typed_buffer *)(block + 1);
    if (!buffer->owns_bytes) return;
    length = buffer->byte_length;
    if (buffer->moved_external) {
        if (length > from_heap->moved_external_current ||
            length > SIZE_MAX - to_heap->moved_external_current) abort();
        from_heap->moved_external_current -= length;
        to_heap->moved_external_current += length;
    } else {
        if (length > SIZE_MAX - to_heap->since_collection) abort();
        to_heap->since_collection += length;
    }
}

/* Weak collections are indexed by their owning heap so the ephemeron pass can find them
 * without retaining them. Rehome that index when a collection moves with a transferred graph. */
static void kiln_c99_gc_rehome_weak_collection(kiln_gc_heap *to_heap,
                                                kiln_collection *source,
                                                kiln_collection *destination) {
    kiln_gc_heap *from_heap = source->weak_owner;
    if (from_heap == NULL) return;
    if (source->weak_previous != NULL) source->weak_previous->weak_next = source->weak_next;
    else from_heap->weak_collections = source->weak_next;
    if (source->weak_next != NULL) source->weak_next->weak_previous = source->weak_previous;
    source->weak_owner = NULL;
    source->weak_previous = NULL;
    source->weak_next = NULL;
    destination->weak_owner = to_heap;
    destination->weak_previous = NULL;
    destination->weak_next = to_heap->weak_collections;
    if (destination->weak_next != NULL) destination->weak_next->weak_previous = destination;
    to_heap->weak_collections = destination;
}

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
        kiln_c99_gc_splice_external(from_heap, to_heap, cur);
        if ((cur->kind & KILN_GC_KIND_MASK) == KILN_GC_COLLECTION) {
            kiln_collection *collection = (kiln_collection *)(cur + 1);
            if (collection->is_weak)
                kiln_c99_gc_rehome_weak_collection(to_heap, collection, collection);
        }
        kiln_c99_candidate_large_remove(from_heap, cur);
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
    /* All moved owners must be indexed before their initialized fields are published. The
     * destination may contain live candidates unrelated to the source realm's current TLS. */
    if (to_heap->candidate_ledger_groups != 0) {
        for (cur = first; cur != NULL; cur = cur->next) {
            kiln_c99_candidate_record_owner_layout(to_heap, cur);
            if (cur == last) break;
        }
    }
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
    /* Two kinds keep storage inside their own block and point at it: a promise's first reaction
     * and a generator's locals and stack. The copy's pointers must name the copy. */
    switch (root_block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_PROMISE: {
        kiln_promise *copy = (kiln_promise *)(promoted + 1);
        if (copy->reactions == &((kiln_promise *)root_ptr)->inline_reaction) {
            kiln_c99_heap_store_pointer(copy, &copy->reactions, &copy->inline_reaction);
        }
        break;
    }
    case KILN_GC_GENERATOR: {
        kiln_generator *copy = (kiln_generator *)(promoted + 1);
        if (copy->locals == (kiln_val *)((kiln_generator *)root_ptr + 1)) {
            kiln_c99_heap_store_pointer(copy, &copy->stack,
                                         (kiln_val *)(copy + 1) + (copy->stack - copy->locals));
            kiln_c99_heap_store_pointer(copy, &copy->locals, (kiln_val *)(copy + 1));
        }
        break;
    }
    default:
        break;
    }
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
    kiln_c99_candidate_record_owner_layout(to_heap, promoted);

    /* The copied wrapper now owns any external store. Clear the source before releasing its
     * nursery/slab cell so its finalizer neither frees that store nor drops its shared handle. */
    if ((root_block->kind & KILN_GC_KIND_MASK) == KILN_GC_TYPED_BUFFER) {
        kiln_typed_buffer *source = (kiln_typed_buffer *)root_ptr;
        kiln_c99_gc_splice_external(from_heap, to_heap, root_block);
        source->bytes = NULL;
        source->byte_length = 0;
        source->owns_bytes = 0;
        source->moved_external = 0;
        source->shared = NULL;
    }
    if ((root_block->kind & KILN_GC_KIND_MASK) == KILN_GC_COLLECTION) {
        kiln_collection *source = (kiln_collection *)root_ptr;
        kiln_collection *copy = (kiln_collection *)(promoted + 1);
        if (source->is_weak) kiln_c99_gc_rehome_weak_collection(to_heap, source, copy);
        source->entries = NULL;
        source->hash_table = NULL;
    }

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
            kiln_c99_gc_finalize(heap, block);
            block->kind |= KILN_GC_FLAG_DEAD;
            kiln_c99_nursery_note_release(heap, block);
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
    kiln_c99_gc_finalize(heap, block);
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
        kiln_c99_candidate_large_remove(heap, block);
        free(block);
    }
}

static void kiln_c99_gc_mark_found(kiln_gc_heap *heap, kiln_gc_block *block) {
    if (block == NULL || kiln_c99_gc_is_marked(heap, block)) return;
    block->mark = heap->epoch;
    heap->marked_blocks += 1;
    if ((block->kind & KILN_GC_FLAG_NURSERY) != 0) kiln_c99_nursery_note_mark(heap, block);
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

static void kiln_c99_gc_mark_now(kiln_gc_heap *heap, const void *payload);

/* Marking a large live heap is a chain of cache misses: every candidate edge is looked up in its
 * chunk's bitmap and its header read for the mark, and the tracer stalls on each in turn. So an
 * edge is not looked up where it is found. Its header is prefetched and it waits in a short FIFO,
 * and the oldest waiting edge is looked up once the queue is full -- by then its header has had
 * fifteen other edges' worth of time to arrive. `kiln_c99_gc_drain` empties the queue before it
 * returns, so nothing reads a mark before every edge is decided.
 *
 * The prefetch is only a hint: it reads nothing and faults on nothing, so an edge that is not a
 * heap pointer at all costs one wasted hint and is still rejected by the lookup as before --
 * marking never trusts a pointer (ADR 0041). */
static void kiln_c99_gc_mark_block(kiln_gc_heap *heap, const void *payload) {
    uint32_t slot;
    if (payload == NULL) return;
    __builtin_prefetch((const char *)payload - sizeof(kiln_gc_block), 1);
    if (heap->pending_len == KILN_GC_PENDING_EDGES) {
        const void *oldest = heap->pending[heap->pending_head];
        heap->pending[heap->pending_head] = payload;
        heap->pending_head = (heap->pending_head + 1) & (KILN_GC_PENDING_EDGES - 1);
        kiln_c99_gc_mark_now(heap, oldest);
        return;
    }
    slot = (heap->pending_head + heap->pending_len) & (KILN_GC_PENDING_EDGES - 1);
    heap->pending[slot] = payload;
    heap->pending_len += 1;
}

static void kiln_c99_gc_mark_now(kiln_gc_heap *heap, const void *payload) {
    kiln_c99_gc_mark_found(heap, kiln_c99_gc_lookup(heap, payload));
}

static void kiln_c99_gc_mark_value(kiln_gc_heap *heap, const kiln_value *value) {
    /* Symbols are the exception to the shared-word layout: their identity pointer is in ptr;
     * every other payload-carrying tag uses bits. Non-heap constants are ignored by lookup. */
    if (value == NULL) return;
    if (value->tag == KILN_VALUE_SYMBOL) {
        kiln_c99_gc_mark_block(heap, kiln_c99_gc_value_pointer(*value));
        return;
    }
    if (value->tag < KILN_VALUE_STRING) return;
    kiln_c99_gc_mark_block(heap, (const void *)(uintptr_t)value->bits);
}

/* Symbol properties store the canonical key string, not the symbol pointer. The realm's
 * symbol array is an identity index; a reachable property key is the strong edge that keeps
 * its dynamic symbol alive. */
static const kiln_symbol *kiln_c99_gc_symbol_for_key(const kiln_gc_heap *heap,
                                                      const kiln_string *key) {
    size_t index;
    if (heap == NULL || key == NULL || !kiln_c99_key_is_symbol(key) ||
        !kiln_c99_key_has_prefix(key, "@@symbol:"))
        return NULL;
    for (index = 0; index < heap->realm.symbol_count; ++index) {
        const kiln_value *value = &heap->realm.symbols[index];
        const kiln_symbol *symbol = (const kiln_symbol *)value->ptr;
        if (symbol != NULL && symbol->key == key) return symbol;
    }
    return NULL;
}

void kiln_c99_gc_mark_symbol_key(struct kiln_gc_heap *opaque, const kiln_string *key) {
    kiln_gc_heap *heap = (kiln_gc_heap *)opaque;
    const kiln_symbol *symbol = kiln_c99_gc_symbol_for_key(heap, key);
    if (symbol != NULL) kiln_c99_gc_mark_block(heap, symbol);
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
/* Property storage supplies the edges for every ordinary-object representation. It must not
 * allocate or materialize a dictionary: collection can arrive inside a property write. */
static void kiln_c99_gc_trace_object_properties(kiln_gc_heap *heap,
                                                 const kiln_object *object);

static void kiln_c99_gc_trace(kiln_gc_heap *heap, kiln_gc_block *block) {
    void *payload = block + 1;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_OBJECT: {
        kiln_object *object = (kiln_object *)payload;
        kiln_c99_gc_trace_object_properties(heap, object);
        kiln_c99_gc_mark_value(heap, &object->prototype);
        kiln_c99_gc_mark_value(heap, &object->boxed_primitive);
        break;
    }
    case KILN_GC_COMMONJS_REQUIRE: {
        kiln_commonjs_require *require = (kiln_commonjs_require *)payload;
        kiln_c99_gc_mark_value(heap, &require->properties);
        break;
    }
    case KILN_GC_CLOSURE: {
        kiln_closure *closure = (kiln_closure *)payload;
        kiln_c99_gc_mark_vals(heap, closure->upvalues, closure->upvalue_count);
        kiln_c99_gc_mark_val(heap, closure->captured_this);
        kiln_c99_gc_mark_val(heap, closure->captured_new_target);
        kiln_c99_gc_mark_value(heap, &closure->captured_arguments);
        /* A region closure is never reached here (no heap holds its block); an ordinary one
         * keeps its prototype and store in these fields. */
        kiln_c99_gc_mark_value(heap, &closure->own_prototype);
        kiln_c99_gc_mark_value(heap, &closure->own_store);
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
        kiln_c99_gc_mark_value(heap, &frame->properties);
        kiln_c99_gc_mark_value(heap, &frame->prototype);
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
        if (!collection->is_weak) {
            for (index = 0; index < collection->count; ++index) {
                if (collection->entries[index].dead) continue;
                kiln_c99_gc_mark_value(heap, &collection->entries[index].key);
                kiln_c99_gc_mark_value(heap, &collection->entries[index].value);
            }
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
        kiln_c99_gc_mark_value(heap, &((kiln_typed_array *)payload)->properties);
        break;
    case KILN_GC_DATA_VIEW:
        kiln_c99_gc_mark_block(heap, ((kiln_data_view *)payload)->buffer);
        break;
    case KILN_GC_METHOD_STORE:
        kiln_c99_gc_mark_value(heap, &((kiln_method_store *)payload)->properties);
        kiln_c99_gc_mark_block(heap, ((kiln_method_store *)payload)->next);
        break;
#define KILN_C99_HOST_MARK_VALUE(field) kiln_c99_gc_mark_value(heap, &owner->field);
#define KILN_C99_HOST_MARK_POINTER(field) kiln_c99_gc_mark_block(heap, owner->field);
#define KILN_C99_HOST_MARK_ARRAY(field, length) \
    kiln_c99_gc_mark_values(heap, owner->field, owner->length);
#define KILN_C99_HOST_MARK_CASE(kind, type, fields) \
    case kind: { \
        type *owner = (type *)payload; \
        fields(KILN_C99_HOST_MARK_VALUE, KILN_C99_HOST_MARK_POINTER, KILN_C99_HOST_MARK_ARRAY) \
        break; \
    }
    KILN_C99_HOST_LAYOUTS(KILN_C99_HOST_MARK_CASE)
#undef KILN_C99_HOST_MARK_CASE
#undef KILN_C99_HOST_MARK_ARRAY
#undef KILN_C99_HOST_MARK_POINTER
#undef KILN_C99_HOST_MARK_VALUE
    case KILN_GC_REGEXP: {
        /* A pattern borrows the bytes it was compiled from, which may be a unit constant or a
         * string the program built; the block table decides which. The provider handle is not a
         * block and is released by the finaliser instead. */
        kiln_regexp *regexp = (kiln_regexp *)payload;
        size_t ci;
        kiln_c99_gc_mark_block(heap, regexp->source);
        kiln_c99_gc_mark_block(heap, regexp->flags);
        kiln_c99_gc_mark_value(heap, &regexp->properties);
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
    case KILN_GC_DATE:
        kiln_c99_gc_mark_value(heap, &((kiln_date *)payload)->properties);
        break;
    case KILN_GC_STRING:
    case KILN_GC_BYTES:
    case KILN_GC_TYPED_BUFFER:
    default:
        break;
    }
}

/* The facade index is weak just like its ownership chain. It remembers identity within one
 * realm, never keeps a graph or facade alive, and finalization removes the exact entry before
 * the GC block may be reused. A hash lookup avoids walking every live node of a large graph on
 * each child read. The realm's exclusive turn lease protects the table. */
static size_t kiln_c99_shared_facade_hash(uint32_t graph, uint32_t generation, uint32_t node) {
    uint64_t hash = ((uint64_t)graph << 32) | generation;
    hash ^= (uint64_t)node * UINT64_C(0x9e3779b97f4a7c15);
    hash ^= hash >> 33;
    hash *= UINT64_C(0xff51afd7ed558ccd);
    hash ^= hash >> 33;
    return (size_t)hash;
}

static kiln_shared_facade *kiln_c99_shared_facade_find(kiln_gc_heap *heap, uint32_t graph,
                                                        uint32_t generation, uint32_t node) {
    kiln_shared_facade *facade;
    size_t bucket;
    if (heap->shared_facade_capacity == 0) return NULL;
    bucket = kiln_c99_shared_facade_hash(graph, generation, node) &
             (heap->shared_facade_capacity - 1);
    for (facade = heap->shared_facade_buckets[bucket]; facade != NULL;
         facade = facade->hash_next) {
        if (facade->graph == graph && facade->generation == generation && facade->node == node)
            return facade;
    }
    return NULL;
}

static int kiln_c99_shared_facade_reserve(kiln_gc_heap *heap) {
    kiln_shared_facade **buckets;
    kiln_shared_facade *facade;
    size_t capacity = heap->shared_facade_capacity;
    if (capacity != 0 && heap->shared_facade_count + 1 <= capacity - capacity / 4)
        return KILN_C99_OK;
    if (capacity > SIZE_MAX / 2) return KILN_C99_OUT_OF_MEMORY;
    capacity = capacity == 0 ? 16 : capacity * 2;
    if (capacity > SIZE_MAX / sizeof *buckets) return KILN_C99_OUT_OF_MEMORY;
    buckets = (kiln_shared_facade **)calloc(capacity, sizeof *buckets);
    if (buckets == NULL) return KILN_C99_OUT_OF_MEMORY;
    for (facade = heap->shared_facades; facade != NULL; facade = facade->next) {
        size_t bucket = kiln_c99_shared_facade_hash(facade->graph, facade->generation,
                                                     facade->node) & (capacity - 1);
        kiln_c99_heap_store_pointer(facade, &facade->hash_next, buckets[bucket]);
        buckets[bucket] = facade;
    }
    free(heap->shared_facade_buckets);
    heap->shared_facade_buckets = buckets;
    heap->shared_facade_capacity = capacity;
    return KILN_C99_OK;
}

static void kiln_c99_shared_facade_insert(kiln_gc_heap *heap, kiln_shared_facade *facade) {
    size_t bucket = kiln_c99_shared_facade_hash(facade->graph, facade->generation,
                                                 facade->node) & (heap->shared_facade_capacity - 1);
    kiln_c99_heap_store_pointer(facade, &facade->hash_next, heap->shared_facade_buckets[bucket]);
    heap->shared_facade_buckets[bucket] = facade;
    heap->shared_facade_count++;
}

static void kiln_c99_shared_facade_remove(kiln_gc_heap *heap, kiln_shared_facade *facade) {
    kiln_shared_facade **at;
    kiln_shared_facade *previous = NULL;
    size_t bucket;
    if (heap->shared_facade_capacity == 0) return;
    bucket = kiln_c99_shared_facade_hash(facade->graph, facade->generation, facade->node) &
             (heap->shared_facade_capacity - 1);
    for (at = &heap->shared_facade_buckets[bucket]; *at != NULL;
         previous = *at, at = &(*at)->hash_next) {
        if (*at == facade) {
            if (previous != NULL)
                kiln_c99_heap_store_pointer(previous, &previous->hash_next, facade->hash_next);
            else *at = facade->hash_next; /* The independently owned realm bucket head. */
            kiln_c99_heap_store_pointer(facade, &facade->hash_next, NULL);
            heap->shared_facade_count--;
            return;
        }
    }
}

/* Inner growth buffers are the owner's, never a value's, so they are released here and only
 * here. A kind whose payload owns a provider handle releases it here too. */
static void kiln_c99_gc_finalize(kiln_gc_heap *heap, kiln_gc_block *block) {
    void *payload = block + 1;
    switch (block->kind & KILN_GC_KIND_MASK) {
    case KILN_GC_STRING:
    case KILN_GC_SLICED_STRING:
    case KILN_GC_BYTES:
        break;
    case KILN_GC_SHARED_FACADE:
    {
        kiln_shared_facade *facade = (kiln_shared_facade *)payload;
        kiln_gc_heap *owner = (kiln_gc_heap *)facade->owner;
        if (owner != NULL) {
            kiln_c99_shared_facade_remove(owner, facade);
            if (facade->previous != NULL) kiln_c99_heap_store_pointer(facade->previous, &facade->previous->next, facade->next);
            else owner->shared_facades = facade->next;
            if (facade->next != NULL) kiln_c99_heap_store_pointer(facade->next, &facade->next->previous, facade->previous);
            kiln_c99_heap_store_pointer(facade, &facade->owner, NULL);
        }
        if (facade->lease != 0) {
            (void)kiln_shared_graph_release(facade->lease);
            facade->lease = 0;
        }
        facade->arena.base = NULL;
        facade->arena.length = 0;
        break;
    }
    case KILN_GC_OBJECT:
        kiln_c99_object_properties_free((kiln_object *)payload);
        if (((kiln_object *)payload)->sealed != NULL) {
            free(kiln_c99_sealed_values_of((kiln_object *)payload));
        } else kiln_c99_object_slots_free((kiln_object *)payload);
        kiln_c99_object_elements_free((kiln_object *)payload);
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
            free(arr->candidate_elements ? (void *)kiln_c99_array_candidate_header(arr)
                                         : (void *)arr->elements);
        }
        break;
    }
    case KILN_GC_GENERATOR: {
        kiln_generator *frame = (kiln_generator *)payload;
        /* The locals and stack are inside the frame's own block (`kiln_c99_new_generator`). */
        if (frame->locals != (kiln_val *)(frame + 1)) {
            free(frame->locals);
            free(frame->stack);
        }
        free(frame->handlers);
        free(frame->handler_stack);
        free(frame->queue_promises);
        free(frame->queue_sent);
        break;
    }
    case KILN_GC_PROMISE: {
        kiln_promise *promise = (kiln_promise *)payload;
        if (promise->reactions != &promise->inline_reaction) free(promise->reactions);
        break;
    }
    case KILN_GC_BOUND:
        free(((kiln_bound *)payload)->arguments);
        break;
    case KILN_GC_COLLECTION:
    {
        kiln_collection *collection = (kiln_collection *)payload;
        kiln_gc_heap *owner = collection->weak_owner;
        if (owner != NULL) {
            if (collection->weak_previous != NULL) collection->weak_previous->weak_next = collection->weak_next;
            else owner->weak_collections = collection->weak_next;
            if (collection->weak_next != NULL) collection->weak_next->weak_previous = collection->weak_previous;
            collection->weak_owner = NULL;
        }
        free(collection->entries);
        free(collection->hash_table);
        break;
    }
    case KILN_GC_TYPED_BUFFER: {
        kiln_typed_buffer *buffer = (kiln_typed_buffer *)payload;
        if (buffer->owns_bytes) {
            if (buffer->moved_external) {
                if (buffer->byte_length > heap->moved_external_current) abort();
                heap->moved_external_current -= buffer->byte_length;
            }
            kiln_c99_bytes_retire(buffer->bytes, buffer->byte_length);
        }
        /* A `SharedArrayBuffer`'s store outlives this realm's buffer while another realm holds
         * it; this buffer's reference is handed back. */
        if (buffer->shared != NULL) kiln_shared_release(buffer->shared);
        break;
    }
    case KILN_GC_REGEXP:
        /* The compiled pattern is the provider's, and this is the only place it is handed back. */
        kiln_regexp_free(((kiln_regexp *)payload)->handle);
        break;
    case KILN_GC_NODE_SERVER:
        free(((kiln_node_server *)payload)->address);
        break;
    case KILN_GC_NODE_SOCKET:
        free(((kiln_node_socket *)payload)->pending_write_bytes);
        free(((kiln_node_socket *)payload)->corked_write_bytes);
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
    for (;;) {
        while (heap->stack_len != 0 && !heap->mark_failed) {
            kiln_gc_block *block = heap->stack[--heap->stack_len];
            kiln_c99_gc_trace(heap, block);
        }
        if (heap->pending_len == 0 || heap->mark_failed) break;
        {
            const void *oldest = heap->pending[heap->pending_head];
            heap->pending_head = (heap->pending_head + 1) & (KILN_GC_PENDING_EDGES - 1);
            heap->pending_len -= 1;
            kiln_c99_gc_mark_now(heap, oldest);
        }
    }
    heap->pending_head = 0;
    heap->pending_len = 0;
}

/* A weak key is live only when another root reached its GC block. Process-owned immutable
 * values have no block in this heap and cannot be collected here. This lookup is performed
 * before sweep, while every weak entry still points to allocated storage. */
static int kiln_c99_gc_weak_key_live(kiln_gc_heap *heap, const kiln_value *key) {
    kiln_gc_block *block;
    if (key->tag == KILN_VALUE_SYMBOL) {
        block = kiln_c99_gc_lookup(heap, kiln_c99_gc_value_pointer(*key));
        return block == NULL || kiln_c99_gc_is_marked(heap, block);
    }
    if (key->tag < KILN_VALUE_STRING) return 1;
    block = kiln_c99_gc_lookup(heap, (const void *)(uintptr_t)key->bits);
    return block == NULL || kiln_c99_gc_is_marked(heap, block);
}

static void kiln_c99_gc_prune_unique_symbols(kiln_gc_heap *heap) {
    kiln_c99_realm_state *realm = &heap->realm;
    size_t read_at, write_at = 0;
    for (read_at = 0; read_at < realm->symbol_count; ++read_at) {
        kiln_value value = realm->symbols[read_at];
        kiln_symbol *symbol = (kiln_symbol *)value.ptr;
        kiln_gc_block *block;
        if (symbol == NULL || symbol->registry_id != 0) {
            realm->symbols[write_at++] = value;
            continue;
        }
        block = kiln_c99_gc_lookup(heap, symbol);
        if (block != NULL && kiln_c99_gc_is_marked(heap, block))
            realm->symbols[write_at++] = value;
    }
    realm->symbol_count = write_at;
    if (write_at == 0) {
        free(realm->symbols);
        realm->symbols = NULL;
        realm->symbol_capacity = 0;
    } else if (realm->symbol_capacity > 64 && write_at < realm->symbol_capacity / 4) {
        size_t capacity = write_at * 2;
        kiln_value *smaller =
            (kiln_value *)realloc(realm->symbols, capacity * sizeof *smaller);
        if (smaller != NULL) {
            realm->symbols = smaller;
            realm->symbol_capacity = capacity;
        }
    }
}

static kiln_gc_weak_resource *kiln_c99_gc_weak_resource_find(kiln_gc_heap *heap,
                                                               kiln_value handle) {
    kiln_gc_weak_resource *entry;
    if (heap == NULL || handle.tag != KILN_VALUE_OBJECT) return NULL;
    for (entry = heap->weak_resources; entry != NULL; entry = entry->next)
        if (entry->handle.bits == handle.bits) return entry;
    return NULL;
}

/* Registration is fallible and occurs only after the private resource has been built. A handle
 * cannot name a foreign or interior block; its identity remains stable in this nonmoving heap. */
static int kiln_c99_gc_weak_resource_register(kiln_gc_heap *heap, kiln_value handle,
                                                void *payload, void (*dispose)(void *)) {
    kiln_gc_weak_resource *entry;
    kiln_gc_block *block;
    if (heap == NULL || handle.tag != KILN_VALUE_OBJECT || payload == NULL || dispose == NULL ||
        kiln_c99_gc_weak_resource_find(heap, handle) != NULL) return 0;
    block = kiln_c99_gc_lookup(heap, (const void *)(uintptr_t)handle.bits);
    if (block == NULL || (const void *)(block + 1) != (const void *)(uintptr_t)handle.bits ||
        (block->kind & KILN_GC_KIND_MASK) != KILN_GC_OBJECT) return 0;
    entry = (kiln_gc_weak_resource *)malloc(sizeof *entry);
    if (entry == NULL) return 0;
    entry->handle = handle;
    entry->payload = payload;
    entry->dispose = dispose;
    entry->mark = NULL;
    entry->process = NULL;
    entry->aliases = NULL;
    entry->next = heap->weak_resources;
    heap->weak_resources = entry;
    return 1;
}

static int kiln_c99_gc_weak_resource_register_managed(
    kiln_gc_heap *heap, kiln_value handle, void *payload, void (*dispose)(void *),
    void (*mark)(kiln_gc_heap *, void *), int (*process)(kiln_gc_heap *, void *),
    int (*aliases)(const kiln_gc_heap *, kiln_nursery_chunk *const *, size_t, void *)) {
    kiln_gc_weak_resource *entry;
    if (!kiln_c99_gc_weak_resource_register(heap, handle, payload, dispose)) return 0;
    entry = kiln_c99_gc_weak_resource_find(heap, handle);
    if (entry == NULL) return 0;
    entry->mark = mark;
    entry->process = process;
    entry->aliases = aliases;
    return 1;
}

static void kiln_c99_gc_weak_resource_release(kiln_gc_heap *heap, kiln_value handle) {
    kiln_gc_weak_resource **link;
    if (heap == NULL) return;
    link = &heap->weak_resources;
    while (*link != NULL) {
        kiln_gc_weak_resource *entry = *link;
        if (entry->handle.tag == handle.tag && entry->handle.bits == handle.bits) {
            *link = entry->next;
            entry->dispose(entry->payload);
            free(entry);
            return;
        }
        link = &entry->next;
    }
}

static void kiln_c99_gc_weak_resources_sweep(kiln_gc_heap *heap) {
    kiln_gc_weak_resource **link = &heap->weak_resources;
    while (*link != NULL) {
        kiln_gc_weak_resource *entry = *link;
        kiln_gc_block *block = kiln_c99_gc_lookup(heap,
                                    (const void *)(uintptr_t)entry->handle.bits);
        if (block != NULL && kiln_c99_gc_is_marked(heap, block)) {
            link = &entry->next;
            continue;
        }
        *link = entry->next;
        entry->dispose(entry->payload);
        free(entry);
    }
}

static void kiln_c99_gc_weak_resources_mark(kiln_gc_heap *heap) {
    kiln_gc_weak_resource *entry;
    for (entry = heap->weak_resources; entry != NULL && !heap->mark_failed;
         entry = entry->next) {
        kiln_gc_block *block = kiln_c99_gc_lookup(heap,
                                    (const void *)(uintptr_t)entry->handle.bits);
        if (block != NULL && kiln_c99_gc_is_marked(heap, block) && entry->mark != NULL)
            entry->mark(heap, entry->payload);
    }
}

static void kiln_c99_gc_weak_resources_process(kiln_gc_heap *heap) {
    kiln_gc_weak_resource *entry;
    for (entry = heap->weak_resources; entry != NULL && !heap->mark_failed;
         entry = entry->next) {
        kiln_gc_block *block = kiln_c99_gc_lookup(heap,
                                    (const void *)(uintptr_t)entry->handle.bits);
        if (block != NULL && kiln_c99_gc_is_marked(heap, block) && entry->process != NULL &&
            entry->process(heap, entry->payload) != KILN_C99_OK)
            heap->mark_failed = 1;
    }
}

/* Heap weak-resource processing precedes the promise implementation in the prelude. */
extern int kiln_c99_enqueue_finalizer(kiln_value callback, kiln_value held_value);
extern int kiln_c99_gc_keep_during_job(kiln_value value);

typedef struct kiln_gc_finalization_cell {
    struct kiln_gc_finalization_cell *next;
    kiln_value target;
    kiln_value held;
    kiln_value token;
} kiln_gc_finalization_cell;

typedef struct kiln_gc_weak_reference {
    kiln_value target;
} kiln_gc_weak_reference;

typedef struct kiln_gc_finalization_registry {
    kiln_value callback;
    kiln_gc_finalization_cell *cells;
} kiln_gc_finalization_registry;

static int kiln_c99_gc_same_value_identity(kiln_value left, kiln_value right) {
    if (left.tag != right.tag) return 0;
    if (left.tag == KILN_VALUE_SYMBOL)
        return kiln_c99_gc_value_pointer(left) == kiln_c99_gc_value_pointer(right);
    return left.bits == right.bits;
}

static int kiln_c99_gc_weak_reference_aliases(const kiln_gc_heap *heap,
                                               kiln_nursery_chunk *const *chunks,
                                               size_t count, void *payload) {
    kiln_gc_weak_reference *cell = (kiln_gc_weak_reference *)payload;
    return kiln_c99_graph_value_in_batch(heap, chunks, count, &cell->target);
}

static int kiln_c99_gc_finalization_registry_aliases(const kiln_gc_heap *heap,
                                                       kiln_nursery_chunk *const *chunks,
                                                       size_t count, void *payload) {
    kiln_gc_finalization_registry *registry = (kiln_gc_finalization_registry *)payload;
    kiln_gc_finalization_cell *cell;
    if (kiln_c99_graph_value_in_batch(heap, chunks, count, &registry->callback)) return 1;
    for (cell = registry->cells; cell != NULL; cell = cell->next) {
        if (kiln_c99_graph_value_in_batch(heap, chunks, count, &cell->target) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &cell->held) ||
            kiln_c99_graph_value_in_batch(heap, chunks, count, &cell->token)) return 1;
    }
    return 0;
}

static void kiln_c99_gc_weak_reference_dispose(void *payload) { free(payload); }

static int kiln_c99_gc_weak_reference_process(kiln_gc_heap *heap, void *payload) {
    kiln_gc_weak_reference *cell = (kiln_gc_weak_reference *)payload;
    if (!kiln_c99_gc_weak_key_live(heap, &cell->target)) cell->target = kiln_c99_undefined();
    return KILN_C99_OK;
}

static void kiln_c99_gc_finalization_registry_dispose(void *payload) {
    kiln_gc_finalization_registry *registry = (kiln_gc_finalization_registry *)payload;
    kiln_gc_finalization_cell *cell = registry->cells;
    while (cell != NULL) {
        kiln_gc_finalization_cell *next = cell->next;
        free(cell);
        cell = next;
    }
    free(registry);
}

static void kiln_c99_gc_finalization_registry_mark(kiln_gc_heap *heap, void *payload) {
    kiln_gc_finalization_registry *registry = (kiln_gc_finalization_registry *)payload;
    kiln_gc_finalization_cell *cell;
    kiln_c99_gc_mark_value(heap, &registry->callback);
    for (cell = registry->cells; cell != NULL; cell = cell->next)
        kiln_c99_gc_mark_value(heap, &cell->held);
}

static int kiln_c99_gc_finalization_registry_process(kiln_gc_heap *heap, void *payload) {
    kiln_gc_finalization_registry *registry = (kiln_gc_finalization_registry *)payload;
    kiln_gc_finalization_cell **link = &registry->cells;
    while (*link != NULL) {
        kiln_gc_finalization_cell *cell = *link;
        if (!kiln_c99_gc_weak_key_live(heap, &cell->target)) {
            int status = kiln_c99_enqueue_finalizer(registry->callback, cell->held);
            if (status != KILN_C99_OK) return status;
            kiln_c99_gc_mark_value(heap, &registry->callback);
            kiln_c99_gc_mark_value(heap, &cell->held);
            *link = cell->next;
            free(cell);
            continue;
        }
        if (cell->token.tag != KILN_VALUE_UNDEFINED &&
            !kiln_c99_gc_weak_key_live(heap, &cell->token))
            cell->token = kiln_c99_undefined();
        link = &cell->next;
    }
    return KILN_C99_OK;
}

int kiln_c99_weak_reference_create(kiln_value owner, kiln_value target) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_gc_weak_reference *cell = (kiln_gc_weak_reference *)malloc(sizeof *cell);
    if (cell == NULL) return KILN_C99_OUT_OF_MEMORY;
    cell->target = target;
    if (!kiln_c99_gc_weak_resource_register_managed(
            heap, owner, cell, kiln_c99_gc_weak_reference_dispose, NULL,
            kiln_c99_gc_weak_reference_process, kiln_c99_gc_weak_reference_aliases)) {
        free(cell);
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (kiln_c99_gc_keep_during_job(target) != KILN_C99_OK) return KILN_C99_OUT_OF_MEMORY;
    return KILN_C99_OK;
}

int kiln_c99_weak_reference_deref(kiln_value owner, kiln_value *out) {
    kiln_gc_weak_resource *entry = kiln_c99_gc_weak_resource_find(kiln_c99_gc_heap(), owner);
    if (entry == NULL || entry->process != kiln_c99_gc_weak_reference_process)
        return KILN_C99_INVALID_ARGUMENT;
    *out = ((kiln_gc_weak_reference *)entry->payload)->target;
    if (out->tag != KILN_VALUE_UNDEFINED && kiln_c99_gc_keep_during_job(*out) != KILN_C99_OK)
        return KILN_C99_OUT_OF_MEMORY;
    return KILN_C99_OK;
}

int kiln_c99_gc_keep_during_job(kiln_value value) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    size_t index;
    kiln_value *grown;
    if (heap == NULL) return KILN_C99_INVALID_ARGUMENT;
    for (index = 0; index < heap->weak_keep_count; ++index) {
        if (kiln_c99_gc_same_value_identity(heap->weak_keep_during_job[index], value))
            return KILN_C99_OK;
    }
    if (heap->weak_keep_count == heap->weak_keep_capacity) {
        size_t capacity = heap->weak_keep_capacity == 0 ? 8 : heap->weak_keep_capacity * 2;
        if (capacity < heap->weak_keep_capacity || capacity > SIZE_MAX / sizeof *grown)
            return KILN_C99_OUT_OF_MEMORY;
        grown = (kiln_value *)realloc(heap->weak_keep_during_job, capacity * sizeof *grown);
        if (grown == NULL) return KILN_C99_OUT_OF_MEMORY;
        heap->weak_keep_during_job = grown;
        heap->weak_keep_capacity = capacity;
    }
    heap->weak_keep_during_job[heap->weak_keep_count++] = value;
    return KILN_C99_OK;
}

void kiln_c99_gc_enter_job(void) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap != NULL && heap->weak_job_depth != UINT32_MAX) heap->weak_job_depth++;
}

int kiln_c99_gc_enter_job_if_idle(void) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || heap->weak_job_depth != 0) return 0;
    heap->weak_job_depth = 1;
    return 1;
}

void kiln_c99_gc_leave_job(void) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    if (heap == NULL || heap->weak_job_depth == 0) return;
    heap->weak_job_depth--;
    if (heap->weak_job_depth == 0) heap->weak_keep_count = 0;
}

int kiln_c99_finalization_registry_create(kiln_value owner, kiln_value callback) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_gc_finalization_registry *registry =
        (kiln_gc_finalization_registry *)calloc(1, sizeof *registry);
    if (registry == NULL) return KILN_C99_OUT_OF_MEMORY;
    registry->callback = callback;
    if (!kiln_c99_gc_weak_resource_register_managed(
            heap, owner, registry, kiln_c99_gc_finalization_registry_dispose,
            kiln_c99_gc_finalization_registry_mark,
            kiln_c99_gc_finalization_registry_process,
            kiln_c99_gc_finalization_registry_aliases)) {
        free(registry);
        return KILN_C99_INVALID_ARGUMENT;
    }
    return KILN_C99_OK;
}

int kiln_c99_finalization_registry_register(kiln_value owner, kiln_value target,
                                             kiln_value held, kiln_value token) {
    kiln_gc_weak_resource *entry = kiln_c99_gc_weak_resource_find(kiln_c99_gc_heap(), owner);
    kiln_gc_finalization_registry *registry;
    kiln_gc_finalization_cell *cell;
    if (entry == NULL || entry->process != kiln_c99_gc_finalization_registry_process)
        return KILN_C99_INVALID_ARGUMENT;
    cell = (kiln_gc_finalization_cell *)malloc(sizeof *cell);
    if (cell == NULL) return KILN_C99_OUT_OF_MEMORY;
    registry = (kiln_gc_finalization_registry *)entry->payload;
    cell->target = target;
    cell->held = held;
    cell->token = token;
    cell->next = registry->cells;
    registry->cells = cell;
    return KILN_C99_OK;
}

int kiln_c99_finalization_registry_unregister(kiln_value owner, kiln_value token,
                                               kiln_value *out) {
    kiln_gc_weak_resource *entry = kiln_c99_gc_weak_resource_find(kiln_c99_gc_heap(), owner);
    kiln_gc_finalization_registry *registry;
    kiln_gc_finalization_cell **link;
    int removed = 0;
    if (entry == NULL || entry->process != kiln_c99_gc_finalization_registry_process)
        return KILN_C99_INVALID_ARGUMENT;
    registry = (kiln_gc_finalization_registry *)entry->payload;
    link = &registry->cells;
    while (*link != NULL) {
        kiln_gc_finalization_cell *cell = *link;
        if (kiln_c99_gc_same_value_identity(cell->token, token)) {
            *link = cell->next;
            free(cell);
            removed = 1;
        } else link = &cell->next;
    }
    *out = kiln_c99_boolean(removed);
    return KILN_C99_OK;
}

static void kiln_c99_gc_weak_resources_dispose_all(kiln_gc_heap *heap) {
    kiln_gc_weak_resource *entry = heap->weak_resources;
    heap->weak_resources = NULL;
    while (entry != NULL) {
        kiln_gc_weak_resource *next = entry->next;
        entry->dispose(entry->payload);
        free(entry);
        entry = next;
    }
}

/* WeakMap entries are ephemerons: a live key makes its value live, and that value may in turn
 * make another weak key live. Repeat to a fixed point before clearing dead entries or sweeping.
 * WeakSet stores its key as its value; marking it after the key is proved live changes nothing. */
static void kiln_c99_gc_weak_ephemerons(kiln_gc_heap *heap) {
    size_t before;
    kiln_collection *collection;
    do {
        before = heap->marked_blocks;
        for (collection = heap->weak_collections; collection != NULL;
             collection = collection->weak_next) {
            kiln_gc_block *block = kiln_c99_gc_lookup(heap, collection);
            size_t index;
            if (block == NULL || !kiln_c99_gc_is_marked(heap, block)) continue;
            for (index = 0; index < collection->count; ++index) {
                kiln_entry *entry = &collection->entries[index];
                if (!entry->dead && kiln_c99_gc_weak_key_live(heap, &entry->key))
                    kiln_c99_gc_mark_value(heap, &entry->value);
            }
        }
        kiln_c99_gc_drain(heap);
    } while (!heap->mark_failed && heap->marked_blocks != before);
    if (heap->mark_failed) return;
    for (collection = heap->weak_collections; collection != NULL;
         collection = collection->weak_next) {
        kiln_gc_block *block = kiln_c99_gc_lookup(heap, collection);
        size_t index;
        if (block == NULL || !kiln_c99_gc_is_marked(heap, block)) continue;
        for (index = 0; index < collection->count; ++index) {
            kiln_entry *entry = &collection->entries[index];
            if (entry->dead || kiln_c99_gc_weak_key_live(heap, &entry->key)) continue;
            kiln_c99_collection_hash_remove(collection, index, entry->key);
            entry->dead = 1;
            entry->key = kiln_c99_undefined();
            entry->value = kiln_c99_undefined();
            collection->live -= 1;
        }
    }
}

/* The granule words a chunk has handed out, from its base to `limit`. */
static inline size_t kiln_c99_nursery_used_words(const kiln_nursery_chunk *chunk, const char *limit) {
    size_t words = ((size_t)(limit - chunk->base) / 16 + 63) / 64;
    return words > chunk->bitmap_words ? chunk->bitmap_words : words;
}

static void kiln_c99_gc_begin(kiln_gc_heap *heap) {
    ++heap->epoch;
    if (heap->epoch == 0) ++heap->epoch;
    heap->stack_len = 0;
    heap->pending_head = 0;
    heap->pending_len = 0;
    heap->marked_blocks = 0;
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
static void kiln_c99_nursery_spare(kiln_gc_heap *heap, kiln_nursery_chunk *chunk);
static size_t kiln_c99_nursery_settle(kiln_gc_heap *heap, kiln_nursery_chunk *chunk, int orphan);

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
        if (!kiln_c99_gc_is_marked(heap, block)) {
            kiln_c99_gc_unlink(heap, block);
            kiln_c99_gc_finalize(heap, block);
            heap->freed_blocks += 1;
            heap->freed_bytes += block->size;
            kiln_c99_gc_ptrset_remove(&heap->large_table, block);
            kiln_c99_candidate_large_remove(heap, block);
            free(block);
        } else if (orphan_marked) {
            kiln_c99_candidate_large_remove(heap, block);
            kiln_c99_gc_retain_survivor(block);
            kiln_c99_gc_unlink(heap, block);
            kiln_c99_gc_ptrset_remove(&heap->large_table, block);
        } else {
            if ((block->kind & KILN_GC_FLAG_CANDIDATE_LARGE) != 0) {
                kiln_c99_candidate_large *entry = kiln_c99_candidate_large_find(heap, block);
                if (entry == NULL) abort();
                entry->epoch = heap->epoch;
            }
            survived += block->size;
        }
        block = next;
    }
    while (slab != NULL) {
        kiln_gc_slab *next_slab = slab->all_next;
        uint32_t index;
        int had_survivor = 0;
        for (index = 0; index < slab->fresh; ++index) {
            kiln_gc_block *slot = kiln_c99_gc_slab_slot(slab, index);
            if (slot->size == KILN_GC_SLOT_FREE) continue;
            if (!kiln_c99_gc_is_marked(heap, slot)) {
                if (kiln_c99_gc_kind_finalized(slot->kind)) kiln_c99_gc_finalize(heap, slot);
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
            size_t before = chunk->alloc_count;
            size_t live = kiln_c99_nursery_settle(heap, chunk, orphan_marked);
            survived += chunk->alloc_bytes;
            chunk->live_count = live;
            if (orphan_marked) {
                *pchunk = chunk->next;
                kiln_c99_nursery_free_region(&heap->nursery, chunk);
            } else if (live == 0) {
                *pchunk = chunk->next;
                kiln_c99_nursery_spare(heap, chunk);
            } else {
                if (live < before) kiln_c99_nursery_reclaim_pages(chunk);
                pchunk = &chunk->next;
            }
        }
    }
    heap->survived = survived;
    heap->moved_external_baseline = heap->moved_external_current;
    heap->since_collection = 0;
}

/* Settle one nursery chunk after a mark: finalise what died, and say how many blocks survived.
 *
 * Bitmaps only (see `kiln_nursery_chunk`): a word of allocated blocks and a word of marked ones
 * give the survivors and the dead sixty-four granules at a time, and the one header this reads
 * is a dead block's whose finaliser has work. What the chunk frees is its allocated count and
 * bytes less what the marking found in it. Settling a chunk twice in one collection -- a chunk
 * settled here and then swept as promoted -- finds nothing dead the second time.
 *
 * `orphan` is teardown's: every block still allocated is finalised, marked or not, as the
 * header walk this replaced did before the chunk is freed. */
static size_t kiln_c99_nursery_settle(kiln_gc_heap *heap, kiln_nursery_chunk *chunk, int orphan) {
    size_t words = kiln_c99_nursery_used_words(chunk, chunk->limit);
    size_t word_index;
    size_t dead_count;
    size_t dead_bytes;
    int marked_now = !orphan && chunk->mark_epoch == heap->epoch;
    size_t marked_count = marked_now ? chunk->marked_count : 0;
    size_t marked_bytes = marked_now ? chunk->marked_bytes : 0;
    /* Nothing died here: nothing to finalise and no bit to clear. A chunk settled twice in one
     * collection always takes this branch the second time. */
    if (!orphan && marked_count == chunk->alloc_count) return marked_count;
    for (word_index = 0; word_index < words; ++word_index) {
        uint64_t allocated = chunk->bitmap[word_index];
        uint64_t live;
        uint64_t finals;
        if (allocated == 0) continue;
        live = marked_now ? allocated & chunk->marks[word_index] : 0;
        finals = allocated & ~live & chunk->finals[word_index];
        while (finals != 0) {
            size_t bit_idx = word_index * 64 + (size_t)__builtin_ctzll(finals);
            kiln_gc_block *b = (kiln_gc_block *)(chunk->base + bit_idx * 16);
            finals &= finals - 1;
            if ((b->kind & KILN_GC_FLAG_DEAD) == 0) {
                kiln_c99_gc_finalize(heap, b);
                b->kind |= KILN_GC_FLAG_DEAD;
            }
        }
        chunk->bitmap[word_index] = live;
        chunk->finals[word_index] &= live;
    }
    if (orphan) return 0;
    dead_count = chunk->alloc_count - marked_count;
    dead_bytes = chunk->alloc_bytes - marked_bytes;
    heap->count -= dead_count;
    heap->bytes -= dead_bytes;
    heap->freed_blocks += dead_count;
    heap->freed_bytes += dead_bytes;
    if (heap->since_collection >= dead_bytes) {
        heap->since_collection -= dead_bytes;
    } else {
        heap->since_collection = 0;
    }
    chunk->alloc_count = marked_count;
    chunk->alloc_bytes = marked_bytes;
    return marked_count;
}

/* An emptied chunk goes back to the spares, up to a budget's worth plus as many chunks as the
 * last collection found live; beyond that it is freed.
 *
 * The second term is for a large live heap, where the collection that empties a promoted chunk
 * is the same one that sets how much the next cycle will allocate -- about what survived. A
 * chunk freed here was mapped again within the cycle, and every page of it faulted in and was
 * zeroed by the kernel a second time: with a hundred thousand tasks sleeping, that was a tenth of
 * the run. Keeping them costs no peak, because the next cycle fills them before it would have
 * mapped new ones. */
static void kiln_c99_nursery_spare(kiln_gc_heap *heap, kiln_nursery_chunk *chunk) {
    kiln_gc_nursery *nursery = &heap->nursery;
    size_t used_words = kiln_c99_nursery_used_words(chunk, chunk->limit);
    memset(chunk->bitmap, 0, used_words * sizeof(uint64_t));
    memset(chunk->marks, 0, used_words * sizeof(uint64_t));
    memset(chunk->finals, 0, used_words * sizeof(uint64_t));
    if (nursery->spare_count >=
        nursery->budget / nursery->capacity + 2 + heap->survived / nursery->capacity) {
        kiln_c99_nursery_free_region(nursery, chunk);
        return;
    }
    chunk->limit = chunk->base;
    chunk->live_count = 0;
    chunk->alloc_count = 0;
    chunk->alloc_bytes = 0;
    chunk->marked_count = 0;
    chunk->marked_bytes = 0;
    chunk->mark_epoch = 0;
    kiln_c99_candidate_chunk_clear(chunk);
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

static int kiln_c99_gc_mark_realm_value(void *opaque, const kiln_value *slot) {
    kiln_c99_gc_mark_value((kiln_gc_heap *)opaque, slot);
    return 0;
}

static int kiln_c99_gc_mark_realm_val(void *opaque, kiln_val value) {
    kiln_c99_gc_mark_val((kiln_gc_heap *)opaque, value);
    return 0;
}

/* The realm context's roots: its symbols, its built-in objects, and every unit's module block.
 * Not marked at teardown: nothing here outlives the realm. */
static void kiln_c99_gc_mark_realm(kiln_gc_heap *heap) {
    kiln_c99_realm_root_visitor visitor = {
        heap, kiln_c99_gc_mark_realm_value, kiln_c99_gc_mark_realm_val
    };
    (void)kiln_c99_gc_visit_realm_roots(heap, &visitor, NULL);
    kiln_c99_gc_mark_values(heap, heap->weak_keep_during_job, heap->weak_keep_count);
}

/* Free what the realm context owns outside the collector, with the heap (teardown). */
static void kiln_c99_realm_free(kiln_c99_realm_state *realm) {
    size_t index;
    free(realm->symbols);
    realm->symbols = NULL;
    realm->symbol_count = realm->symbol_capacity = 0;
    while (realm->ffi_functions != NULL) {
        kiln_ffi_function_meta *next = realm->ffi_functions->next;
        free(realm->ffi_functions->arguments);
        free(realm->ffi_functions);
        realm->ffi_functions = next;
    }
    /* The provider released the closures and their roots before the heap was torn down. */
    while (realm->ffi_callbacks != NULL) {
        kiln_ffi_callback_meta *next = realm->ffi_callbacks->next;
        free(realm->ffi_callbacks);
        realm->ffi_callbacks = next;
    }
    for (index = 0; index < realm->unit_capacity; ++index) free(realm->units[index]);
    free(realm->units);
    realm->units = NULL;
    realm->unit_capacity = 0;
    for (index = 0; index < realm->region_own_buckets; ++index) {
        kiln_c99_region_own *entry = realm->region_own[index];
        while (entry != NULL) {
            kiln_c99_region_own *next = entry->next;
            free(entry);
            entry = next;
        }
    }
    free(realm->region_own);
    realm->region_own = NULL;
    realm->region_own_buckets = 0;
    realm->region_own_count = 0;
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
    if (kiln_c99_actor_candidate_defer_gc(heap)) return;
    if (!heap->enabled && !heap->stress) return;
    if (heap->actor_candidate_id != 0) heap->actor_candidate_collected = 1;
    if (KILN_UNLIKELY(heap->parallel_paused != 0)) {
        fputs("kiln: a collection was due while a parallel call's worker arenas were open\n",
              stderr);
        abort();
    }

    nursery_has_allocs = heap->nursery.full != NULL ||
                         (heap->nursery.active && heap->nursery.bump > heap->nursery.base);
    threshold = heap->floor > heap->survived ? heap->floor : heap->survived;
    tenured_due = force || heap->stress || (heap->since_collection >= threshold) ||
                  (heap->moved_external_current > heap->moved_external_baseline &&
                   heap->moved_external_current - heap->moved_external_baseline >= threshold);

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
    kiln_c99_gc_mark_realm(heap);
    kiln_c99_key_mark_pinned(heap);
    kiln_c99_gc_mark_promise_jobs(heap);
    if (heap->mid_turn) kiln_c99_gc_scan_stack(heap);
    kiln_c99_gc_drain(heap);
    if (!heap->mark_failed) {
        size_t marked_before;
        do {
            marked_before = heap->marked_blocks;
            kiln_c99_gc_weak_ephemerons(heap);
            if (!heap->mark_failed) kiln_c99_gc_weak_resources_mark(heap);
            kiln_c99_gc_drain(heap);
            if (!heap->mark_failed) kiln_c99_gc_weak_resources_process(heap);
            kiln_c99_gc_drain(heap);
        } while (!heap->mark_failed && heap->marked_blocks != marked_before);
        if (!heap->mark_failed) {
            kiln_c99_gc_prune_unique_symbols(heap);
            kiln_c99_gc_weak_resources_sweep(heap);
            kiln_c99_gc_drain(heap);
            if (!heap->mark_failed) kiln_c99_gc_weak_ephemerons(heap);
        }
    }

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
            heap->nursery.held_bytes = 0;
            while (chunk != NULL) {
                kiln_nursery_chunk *next = chunk->next;
                size_t filled = (size_t)(chunk->limit - chunk->base);
                chunk->in_full = 0;
                chunk->full_prev = NULL;
                if (chunk->candidate_state != 0 && chunk->mark_epoch == heap->epoch &&
                    chunk->marked_count != 0) {
                    /* Preserve the exact allocation layout until the graph is moved or dies.
                     * Marking has already traced the reachable blocks. */
                    chunk->candidate_epoch = heap->epoch;
                    kiln_c99_nursery_full_push(&heap->nursery, chunk);
                    heap->nursery.full_bytes += filled;
                    heap->nursery.held_bytes += filled;
                    chunk = next;
                    continue;
                }
                kiln_c99_candidate_chunk_clear(chunk);
                size_t survivors = kiln_c99_nursery_settle(heap, chunk, 0);
                if (survivors == 0) {
                    reset_bytes += filled;
                    heap->nursery.resets++;
                    kiln_c99_nursery_spare(heap, chunk);
                } else {
                    promoted_bytes += filled;
                    chunk->live_count = survivors;
                    kiln_c99_nursery_reclaim_pages(chunk);
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
                /* The current chunk is settled through its header like a sealed one: its fill
                 * and counts go there first, and come back if it stays current. */
                kiln_nursery_chunk *current = (kiln_nursery_chunk *)heap->nursery.region;
                size_t survivors;
                current->limit = heap->nursery.bump;
                current->alloc_count = (size_t)heap->nursery.count;
                current->alloc_bytes = (size_t)heap->nursery.bytes;
                kiln_c99_candidate_chunk_clear(current);
                survivors = kiln_c99_nursery_settle(heap, current, 0);
                heap->nursery.count = current->alloc_count;
                heap->nursery.bytes = current->alloc_bytes;
                if (survivors == 0) {
                    size_t used_words = kiln_c99_nursery_used_words(current, heap->nursery.bump);
                    memset(current->bitmap, 0, used_words * sizeof(uint64_t));
                    memset(current->marks, 0, used_words * sizeof(uint64_t));
                    memset(current->finals, 0, used_words * sizeof(uint64_t));
                    heap->nursery.bump = heap->nursery.base;
                    heap->nursery.count = 0;
                    heap->nursery.bytes = 0;
                    heap->nursery.resets++;
                } else {
                    kiln_nursery_chunk *promoted = kiln_c99_nursery_seal(&heap->nursery);
                    promoted->live_count = survivors;
                    kiln_c99_nursery_reclaim_pages(promoted);
                    promoted->next = heap->nursery.promoted;
                    heap->nursery.promoted = promoted;
                    heap->nursery.promotions += survivors;
                    (void)kiln_c99_nursery_fresh_chunk(heap);
                }
            }
        }

        if (tenured_due || (heap->nursery.promoted != NULL)) {
            kiln_c99_gc_sweep(heap, 0);
            heap->collections += 1;
        }
        kiln_c99_candidate_prune_owners(heap);
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
        /* A local shape's index; its replaced predecessors were freed as it grew. */
        free(shape->edge_table);
        shape->edge_table = NULL;
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
    heap->local_shape_count = 0;
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
    /* First, while this realm's keys and shapes are all still allocated: no site may name one. */
    kiln_c99_gc_forget_realm_local(heap);
    kiln_c99_gc_weak_resources_dispose_all(heap);
    free(heap->weak_keep_during_job);
    heap->weak_keep_during_job = NULL;
    heap->weak_keep_count = heap->weak_keep_capacity = 0;
    /* A realm that was building the shared region and never sealed it: nobody will use it. */
    kiln_c99_region_abandon(heap);
    /* The realm's modifications of built-ins end with it (`kiln_c99_builtin_store_epoch`). */
    __atomic_sub_fetch(&kiln_c99_builtin_store_epoch, heap->builtin_stores, __ATOMIC_RELAXED);
    heap->builtin_stores = 0;
    kiln_c99_gc_drain(heap);
    if (heap->mark_failed) {
        /* Orphan everything rather than free a block a survivor root still names. */
        ++heap->epoch;
        if (heap->epoch == 0) ++heap->epoch;
        for (kiln_gc_block *block = heap->head; block != NULL; block = block->next)
            block->mark = heap->epoch;
        for (kiln_gc_slab *slab = heap->all_slabs; slab != NULL; slab = slab->all_next) {
            uint32_t index;
            for (index = 0; index < slab->fresh; ++index) {
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
                kiln_c99_gc_finalize(heap, b);
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
                    kiln_c99_gc_finalize(heap, b);
                    b->kind |= KILN_GC_FLAG_DEAD;
                }
                curr += total;
            }
        }
    }
    if (heap->candidate_large != NULL) abort();
    kiln_c99_nursery_destroy(&heap->nursery);
    kiln_c99_candidate_group_close(heap->actor_candidate_group);
    heap->actor_candidate_group = NULL;
    if (heap->candidate_ledger_groups != 0 || heap->candidate_ledger_head != NULL) abort();
    /* A result retained outside its dead realm may have survived the final sweep. It cannot
     * name a usable realm-local facade after teardown, so release its lease here. */
    for (kiln_shared_facade *facade = heap->shared_facades; facade != NULL; ) {
        kiln_shared_facade *next = facade->next;
        if (facade->lease != 0) (void)kiln_shared_graph_release(facade->lease);
        facade->lease = 0;
        facade->arena.base = NULL;
        facade->arena.length = 0;
        kiln_c99_heap_store_pointer(facade, &facade->owner, NULL);
        kiln_c99_heap_store_pointer(facade, &facade->previous, NULL);
        kiln_c99_heap_store_pointer(facade, &facade->next, NULL);
        kiln_c99_heap_store_pointer(facade, &facade->hash_next, NULL);
        facade = next;
    }
    heap->shared_facades = NULL;
    free(heap->shared_facade_buckets);
    heap->shared_facade_buckets = NULL;
    heap->shared_facade_capacity = 0;
    heap->shared_facade_count = 0;
    free(heap->candidate_context_owners.table);
    free(heap->large_table.table);
    free(heap->slab_table.table);
    free(heap->stack);
    free(heap->keys.entries);
    kiln_c99_gc_free_local_shapes(heap);
    kiln_c99_gc_heap_unregister(heap);
    /* Teardown does not run what is left in the lane: a realm being destroyed has no turn to run
     * a job in, and the values are about to be freed with the rest of the heap. */
    free(heap->jobs);
    kiln_c99_realm_free(&heap->realm);
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
    if (phase == KILN_GC_PHASE_RESET || phase == KILN_GC_PHASE_LANE_RESET || context == NULL) return;
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
        /* The same ceiling the general path applies, including direct actor candidates. */
        if (!kiln_c99_nursery_accepts_block(heap, sizeof(kiln_gc_block) + payload)) return 0;
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
