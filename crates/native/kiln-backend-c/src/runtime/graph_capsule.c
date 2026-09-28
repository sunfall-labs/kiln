/* Detached storage for a future actor graph transaction. This is a one-chunk primitive, not
 * transfer admission. The caller must hold both realm turns paused, prove the sender has no
 * aliases except its escrow, and secure host/JS publication before detaching. No collector or
 * mutator may run between detach and adoption. A positive ownership certificate and the
 * opaque-ID registry that will carry this capsule across lanes do not exist yet. */

typedef struct kiln_c99_graph_capsule_requirements {
    size_t regions;
    size_t objects;
    size_t bytes;
    size_t filled;
    size_t region_capacity;
    size_t bitmap_words;
} kiln_c99_graph_capsule_requirements;

typedef struct kiln_c99_graph_capsule {
    kiln_nursery_chunk *chunk;
    kiln_value root;
    kiln_c99_graph_capsule_requirements needs;
    uint64_t candidate_id;
    uint64_t sender_epoch;
    int detached;
} kiln_c99_graph_capsule;

/* All fallible closure and root-kind checks run on the sender before its ownership proof. Only
 * exact cell payloads and inline string bytes are supported; the returned requirements contain
 * numbers only and may be copied to the receiver lane before either heap is changed. */
static int kiln_c99_graph_capsule_prepare(const kiln_gc_heap *from, kiln_nursery_chunk *chunk,
                                          kiln_value root, kiln_c99_graph_capsule *out) {
    kiln_nursery_chunk *it;
    const kiln_gc_block *block;
    const void *payload;
    uint32_t kind;
    if (from == NULL || chunk == NULL || out == NULL || from->collecting || from->parallel_paused ||
        chunk == (kiln_nursery_chunk *)from->nursery.region ||
        !kiln_c99_gc_ptrset_contains(&from->nursery.regions, chunk) ||
        chunk->bitmap_words != from->nursery.bitmap_words ||
        chunk->candidate_id == 0 || chunk->candidate_epoch != from->epoch ||
        !kiln_c99_chunk_sealed_for_move(&from->nursery, chunk)) return 0;
    for (it = from->nursery.full; it != NULL && it != chunk; it = it->next) {}
    if (it == NULL || root.bits == 0 ||
        !kiln_c99_graph_value_in_batch(from, &chunk, 1, &root)) return 0;
    block = kiln_c99_gc_lookup_interior(from, (uintptr_t)root.bits);
    if (block == NULL) return 0;
    payload = block + 1;
    kind = block->kind & KILN_GC_KIND_MASK;
    if (root.tag == KILN_VALUE_CELL) {
        if (kind != KILN_GC_CELL || payload != (const void *)(uintptr_t)root.bits) return 0;
    } else if (root.tag == KILN_VALUE_STRING) {
        const kiln_string *string = (const kiln_string *)payload;
        if (kind != KILN_GC_STRING || !kiln_c99_graph_inline_string(block) ||
            string->data != (const void *)(uintptr_t)root.bits ||
            string->unit_len != root.len) return 0;
    } else {
        return 0;
    }
    if (!kiln_c99_graph_candidate_closed(from, &chunk, 1, payload) ||
        chunk->alloc_count > from->count || chunk->alloc_bytes > from->bytes ||
        (size_t)(chunk->limit - chunk->base) > from->nursery.full_bytes) return 0;
    memset(out, 0, sizeof *out);
    out->chunk = chunk;
    out->root = root;
    out->candidate_id = chunk->candidate_id;
    out->sender_epoch = from->epoch;
    out->needs.regions = 1;
    out->needs.objects = chunk->alloc_count;
    out->needs.bytes = chunk->alloc_bytes;
    out->needs.filled = (size_t)(chunk->limit - chunk->base);
    out->needs.region_capacity = from->nursery.capacity;
    out->needs.bitmap_words = from->nursery.bitmap_words;
    return 1;
}

/* Receiver preflight is wholly lane-local. The ID must be a host-authenticated reservation,
 * not a caller-supplied graph pointer. It allocates region-set capacity before sender unlink. */
static int kiln_c99_graph_capsule_receiver_preflight(
    kiln_gc_heap *to, const kiln_c99_graph_capsule_requirements *needs, uint64_t reservation_id) {
    if (to == NULL || needs == NULL || needs->regions != 1 || needs->objects == 0 ||
        needs->filled > needs->region_capacity || needs->bytes > needs->filled ||
        needs->region_capacity != to->nursery.capacity ||
        needs->bitmap_words != to->nursery.bitmap_words ||
        needs->objects > SIZE_MAX - to->count || needs->bytes > SIZE_MAX - to->bytes ||
        needs->bytes > SIZE_MAX - to->since_collection ||
        needs->filled > SIZE_MAX - to->nursery.full_bytes) return 0;
    return kiln_c99_graph_receiver_reserve_id(to, needs->regions, reservation_id);
}

/* This is the last sender-only operation. The caller's external certificate must still be
 * valid, and its escrow root must be consumed before the sender can run or collect again. */
static int kiln_c99_graph_capsule_detach(kiln_gc_heap *from, kiln_c99_graph_capsule *capsule) {
    kiln_nursery_chunk *chunk;
    kiln_nursery_chunk **link;
    if (from == NULL || capsule == NULL || capsule->detached || from->collecting ||
        from->parallel_paused || from->epoch != capsule->sender_epoch) return 0;
    chunk = capsule->chunk;
    if (chunk == NULL || chunk->candidate_id != capsule->candidate_id ||
        chunk->candidate_epoch != from->epoch ||
        chunk->alloc_count != capsule->needs.objects ||
        chunk->alloc_bytes != capsule->needs.bytes ||
        (size_t)(chunk->limit - chunk->base) != capsule->needs.filled ||
        !kiln_c99_gc_ptrset_contains(&from->nursery.regions, chunk) ||
        !kiln_c99_chunk_sealed_for_move(&from->nursery, chunk)) return 0;
    for (link = &from->nursery.full; *link != NULL && *link != chunk; link = &(*link)->next) {}
    if (*link == NULL || capsule->needs.objects > from->count ||
        capsule->needs.bytes > from->bytes ||
        capsule->needs.filled > from->nursery.full_bytes) return 0;
    *link = chunk->next;
    chunk->next = NULL;
    kiln_c99_gc_ptrset_remove(&from->nursery.regions, chunk);
    from->nursery.full_bytes -= capsule->needs.filled;
    from->count -= capsule->needs.objects;
    from->bytes -= capsule->needs.bytes;
    capsule->detached = 1;
    return 1;
}

/* Receiver-only, allocation-free commit. The pair claim and preflight make every condition
 * stable; violation here is a transaction invariant failure, not a recoverable refusal after
 * unlink. The native inbox root is installed before either realm resumes or may collect. */
static void kiln_c99_graph_capsule_adopt(kiln_gc_heap *to, kiln_c99_graph_capsule *capsule,
                                         uint64_t reservation_id) {
    kiln_nursery_chunk *chunk = capsule->chunk;
    kiln_c99_realm_state *realm = &to->realm;
    if (!capsule->detached || chunk == NULL || reservation_id == 0 ||
        to->collecting || to->parallel_paused ||
        realm->graph_inbox_state != KILN_C99_GRAPH_INBOX_RESERVED ||
        realm->graph_inbox_reservation_id != reservation_id ||
        realm->graph_inbox_root.tag != KILN_VALUE_UNDEFINED ||
        realm->graph_inbox_reserved_chunks != capsule->needs.regions ||
        realm->graph_inbox_region_count != to->nursery.regions.count ||
        realm->graph_inbox_epoch != to->epoch ||
        capsule->needs.region_capacity != to->nursery.capacity ||
        capsule->needs.bitmap_words != to->nursery.bitmap_words ||
        capsule->needs.objects > SIZE_MAX - to->count ||
        capsule->needs.bytes > SIZE_MAX - to->bytes ||
        capsule->needs.bytes > SIZE_MAX - to->since_collection ||
        capsule->needs.filled > SIZE_MAX - to->nursery.full_bytes ||
        kiln_c99_gc_ptrset_contains(&to->nursery.regions, chunk) ||
        (to->nursery.regions.count + 1) * 2 > to->nursery.regions.capacity ||
        kiln_c99_gc_ptrset_insert(&to->nursery.regions, chunk) != KILN_C99_OK) abort();
    if (to->nursery.lo == NULL || (char *)chunk < to->nursery.lo)
        to->nursery.lo = (char *)chunk;
    if (to->nursery.hi == NULL || (char *)chunk + to->nursery.capacity > to->nursery.hi)
        to->nursery.hi = (char *)chunk + to->nursery.capacity;
    kiln_c99_gc_bound(to, chunk, to->nursery.capacity);
    chunk->mark_epoch = 0;
    chunk->candidate_epoch = 0;
    chunk->candidate_id = 0;
    chunk->marked_count = 0;
    chunk->marked_bytes = 0;
    chunk->next = to->nursery.full;
    to->nursery.full = chunk;
    to->count += capsule->needs.objects;
    to->bytes += capsule->needs.bytes;
    to->since_collection += capsule->needs.bytes;
    to->nursery.full_bytes += capsule->needs.filled;
    realm->graph_inbox_root = capsule->root;
    realm->graph_inbox_state = KILN_C99_GRAPH_INBOX_PUBLISHED;
    capsule->chunk = NULL;
    capsule->detached = 0;
}
