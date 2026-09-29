/* Actor graph transaction primitives. Only the host pair coordinator calls the
 * paused controls below. Public ticket admission still needs a successful source-side poll and
 * a verified JavaScript inbox slot; neither a C capsule nor a registry entry grants that alone. */

typedef struct kiln_c99_graph_capsule_requirements {
    size_t regions;
    size_t objects;
    size_t bytes;
    size_t filled;
    size_t region_capacity;
    size_t bitmap_words;
    int foreign_dictionary_keys;
} kiln_c99_graph_capsule_requirements;

typedef struct kiln_c99_graph_capsule {
    kiln_nursery_chunk *chunk;
    kiln_nursery_chunk **chunks;
    kiln_value root;
    kiln_c99_graph_capsule_requirements needs;
    uint64_t candidate_id;
    uint64_t sender_epoch;
    int detached;
    int adopted;
} kiln_c99_graph_capsule;

/* The private arena implementation follows later in spread.c; only this transaction sees its
 * certified capsule. The handle remains rooted in the sender's pending request until commit. */
static kiln_c99_graph_capsule *kiln_c99_prepared_graph_capsule(kiln_gc_heap *heap,
                                                               kiln_value handle);
static void kiln_c99_prepared_graph_commit(kiln_gc_heap *heap, kiln_value handle,
                                            kiln_c99_graph_capsule *capsule);

static kiln_nursery_chunk *kiln_c99_graph_capsule_chunk(
    const kiln_c99_graph_capsule *capsule, size_t index) {
    return capsule->needs.regions == 1 ? capsule->chunk : capsule->chunks[index];
}

static void kiln_c99_graph_capsule_dispose(kiln_c99_graph_capsule *capsule) {
    free(capsule->chunks);
    capsule->chunks = NULL;
}

/* All fallible closure and root-kind checks run on the sender before its ownership proof. Only
 * exact cell/object/array/date payloads and inline string bytes are supported; the returned
 * requirements contain numbers only and may cross lanes before either heap is changed. */
static int kiln_c99_graph_capsule_prepare(const kiln_gc_heap *from, kiln_nursery_chunk *chunk,
                                          kiln_value root, kiln_c99_graph_capsule *out) {
    kiln_nursery_chunk *it;
    kiln_nursery_chunk **chunks = NULL;
    const kiln_gc_block *block;
    const void *payload;
    uint32_t kind;
    size_t count = 0, index = 0, objects = 0, bytes = 0, filled = 0;
    if (from == NULL || chunk == NULL || out == NULL || from->collecting || from->parallel_paused ||
        chunk == (kiln_nursery_chunk *)from->nursery.region ||
        !kiln_c99_gc_ptrset_contains(&from->nursery.regions, chunk) ||
        chunk->bitmap_words != from->nursery.bitmap_words ||
        chunk->candidate_id == 0 || chunk->candidate_epoch != from->epoch ||
        !kiln_c99_chunk_sealed_for_move(&from->nursery, chunk)) return 0;
    for (it = from->nursery.full; it != NULL && it != chunk; it = it->next) {}
    if (it == NULL || root.bits == 0) return 0;
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
    } else if (root.tag == KILN_VALUE_OBJECT || root.tag == KILN_VALUE_ARRAY) {
        if ((root.tag == KILN_VALUE_OBJECT ? kind != KILN_GC_OBJECT : kind != KILN_GC_ARRAY) ||
            payload != (const void *)(uintptr_t)root.bits ||
            !kiln_c99_graph_inert_finalizer_block(block)) return 0;
    } else if (root.tag == KILN_VALUE_DATE) {
        if (kind != KILN_GC_DATE || block->size != sizeof(double) ||
            payload != (const void *)(uintptr_t)root.bits) return 0;
    } else {
        return 0;
    }
    for (it = from->nursery.full; it != NULL; it = it->next)
        if (it->candidate_id == chunk->candidate_id) ++count;
    if (count == 0 || count > SIZE_MAX / sizeof *chunks) return 0;
    if (count > 1) {
        chunks = (kiln_nursery_chunk **)malloc(count * sizeof *chunks);
        if (chunks == NULL) return 0;
    }
    for (it = from->nursery.full; it != NULL; it = it->next) {
        size_t used;
        if (it->candidate_id != chunk->candidate_id) continue;
        if (it->limit < it->base ||
            it->limit > (char *)it + from->nursery.capacity ||
            objects > SIZE_MAX - it->alloc_count || bytes > SIZE_MAX - it->alloc_bytes) goto fail;
        used = (size_t)(it->limit - it->base);
        if (filled > SIZE_MAX - used) goto fail;
        objects += it->alloc_count;
        bytes += it->alloc_bytes;
        filled += used;
        if (count > 1) chunks[index] = it;
        ++index;
    }
    if (index != count || objects > from->count || bytes > from->bytes ||
        filled > from->nursery.full_bytes ||
        !kiln_c99_graph_candidate_closed(from, count == 1 ? &chunk : chunks, count, payload) ||
        !kiln_c99_graph_value_in_batch(from, count == 1 ? &chunk : chunks, count, &root)) goto fail;
    memset(out, 0, sizeof *out);
    out->chunk = chunk;
    out->chunks = chunks;
    out->root = root;
    out->candidate_id = chunk->candidate_id;
    out->sender_epoch = from->epoch;
    out->needs.regions = count;
    out->needs.objects = objects;
    out->needs.bytes = bytes;
    out->needs.filled = filled;
    out->needs.region_capacity = from->nursery.capacity;
    out->needs.bitmap_words = from->nursery.bitmap_words;
    /* The closure proof above validated each block before this scan. Keep the answer in the
     * capsule so receiver publication stays constant time after sender detachment. */
    for (index = 0; index < count; ++index) {
        kiln_nursery_chunk *part = count == 1 ? chunk : chunks[index];
        const char *cursor;
        for (cursor = part->base; cursor < part->limit;) {
            const kiln_gc_block *member = (const kiln_gc_block *)cursor;
            if ((member->kind & KILN_GC_KIND_MASK) == KILN_GC_OBJECT) {
                const kiln_object *object = (const kiln_object *)(member + 1);
                if (object->shape == NULL && kiln_c99_object_properties_in_block(object))
                    out->needs.foreign_dictionary_keys = 1;
            }
            cursor += (sizeof *member + (member->size == 0 ? 1 : member->size) + 15) & ~(size_t)15;
        }
    }
    return 1;
fail:
    free(chunks);
    return 0;
}

/* ISOLATE is the first operation after the factory boundary. Collection may have occurred
 * while its generated bridge prepared the arguments, so the provisional chunks stay intact
 * until this exact root has passed the outgoing closure proof. Sender aliases are checked
 * separately at the later two-realm handoff. */
static void kiln_c99_actor_candidate_seal(kiln_gc_heap *heap, kiln_value root) {
    kiln_gc_block *block;
    kiln_nursery_chunk *chunk;
    kiln_c99_graph_capsule capsule;
    int closed = 0;
    if (heap == NULL || heap->actor_candidate_pending_id == 0) return;
    block = kiln_c99_gc_lookup_interior(heap, (uintptr_t)root.bits);
    if (block != NULL && (block->kind & KILN_GC_FLAG_NURSERY) != 0) {
        chunk = kiln_c99_nursery_chunk_of(heap, block);
        if (chunk != NULL && chunk->candidate_id == heap->actor_candidate_pending_id &&
            chunk->candidate_state == 1 &&
            kiln_c99_graph_capsule_prepare(heap, chunk, root, &capsule)) {
            closed = 1;
            kiln_c99_graph_capsule_dispose(&capsule);
        }
    }
    kiln_c99_actor_candidate_finish(heap, closed);
}

/* This is the sole positive certificate for the narrow capsule kinds. The machine boundary is
 * captured by `request_scan` before it resolves an opaque request ID into a graph pointer.
 * Every failure restores the sender's registry value. Success retains the exact escrow slot as
 * the graph's only sender root; the pair transaction must either roll it back or consume it
 * before releasing either realm. No actor path calls this until publication is atomic. */
static int kiln_c99_graph_capsule_certify_request(kiln_gc_heap *heap, uint64_t request_id,
                                                   kiln_c99_graph_capsule *out) {
    kiln_c99_realm_state *realm;
    kiln_gc_block *block;
    kiln_nursery_chunk *chunk;
    if (heap == NULL) return 0;
    if (out == NULL || request_id == 0) {
        (void)kiln_c99_actor_isolated_request_rollback(heap, request_id);
        return 0;
    }
    memset(out, 0, sizeof *out);
    if (kiln_c99_graph_incoming_request_scan(heap, request_id, 1) != KILN_C99_GRAPH_CLEAR)
        return 0;
    realm = &heap->realm;
    if (realm->isolated_request_id == request_id && realm->isolated_escrow_entry != NULL &&
        realm->isolated_escrow_value.bits != 0) {
        block = kiln_c99_gc_lookup_interior(heap, (uintptr_t)realm->isolated_escrow_value.bits);
        if (block != NULL && (block->kind & KILN_GC_FLAG_NURSERY) != 0) {
            chunk = kiln_c99_nursery_chunk_of(heap, block);
            if (kiln_c99_graph_capsule_prepare(heap, chunk, realm->isolated_escrow_value, out))
                return 1;
        }
    }
    (void)kiln_c99_actor_isolated_request_rollback(heap, request_id);
    return 0;
}

/* Receiver preflight is wholly lane-local. The ID must be a host-authenticated reservation,
 * not a caller-supplied graph pointer. It allocates region-set capacity before sender unlink. */
static int kiln_c99_graph_capsule_receiver_preflight(
    kiln_gc_heap *to, const kiln_c99_graph_capsule_requirements *needs, uint64_t reservation_id) {
    if (to == NULL || needs == NULL || needs->regions == 0 || needs->objects == 0 ||
        needs->region_capacity == 0 ||
        needs->regions > SIZE_MAX / needs->region_capacity ||
        needs->filled > needs->regions * needs->region_capacity ||
        needs->bytes > needs->filled ||
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
    size_t index, objects = 0, bytes = 0, filled = 0;
    if (from == NULL || capsule == NULL || capsule->detached || from->collecting ||
        from->parallel_paused || from->epoch != capsule->sender_epoch) return 0;
    if (capsule->needs.regions == 0 || capsule->chunk == NULL ||
        (capsule->needs.regions > 1 && capsule->chunks == NULL)) return 0;
    /* Validate the complete batch while the sender still owns it. After the first unlink no
     * fallible check may leave a partly detached graph behind. */
    for (index = 0; index < capsule->needs.regions; ++index) {
        kiln_nursery_chunk *chunk = kiln_c99_graph_capsule_chunk(capsule, index);
        kiln_nursery_chunk *it;
        size_t used;
        if (chunk == NULL || chunk == (kiln_nursery_chunk *)from->nursery.region ||
            chunk->candidate_id != capsule->candidate_id ||
            chunk->candidate_epoch != from->epoch ||
            !kiln_c99_gc_ptrset_contains(&from->nursery.regions, chunk) ||
            !kiln_c99_chunk_sealed_for_move(&from->nursery, chunk)) return 0;
        for (it = from->nursery.full; it != NULL && it != chunk; it = it->next) {}
        if (it == NULL || objects > SIZE_MAX - chunk->alloc_count ||
            bytes > SIZE_MAX - chunk->alloc_bytes) return 0;
        used = (size_t)(chunk->limit - chunk->base);
        if (filled > SIZE_MAX - used) return 0;
        objects += chunk->alloc_count;
        bytes += chunk->alloc_bytes;
        filled += used;
    }
    if (objects != capsule->needs.objects || bytes != capsule->needs.bytes ||
        filled != capsule->needs.filled || objects > from->count ||
        bytes > from->bytes || filled > from->nursery.full_bytes) return 0;
    for (index = 0; index < capsule->needs.regions; ++index) {
        kiln_nursery_chunk *chunk = kiln_c99_graph_capsule_chunk(capsule, index);
        kiln_nursery_chunk **link = &from->nursery.full;
        while (*link != chunk) link = &(*link)->next;
        *link = chunk->next;
        chunk->next = NULL;
        kiln_c99_gc_ptrset_remove(&from->nursery.regions, chunk);
        if (chunk->candidate_state != 0) {
            size_t used = (size_t)(chunk->limit - chunk->base);
            if (used > from->nursery.held_bytes) abort();
            from->nursery.held_bytes -= used;
        }
    }
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
    kiln_c99_realm_state *realm = &to->realm;
    size_t index;
    if (!capsule->detached || capsule->chunk == NULL ||
        (capsule->needs.regions > 1 && capsule->chunks == NULL) || reservation_id == 0 ||
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
        capsule->needs.regions > SIZE_MAX - to->nursery.regions.count ||
        to->nursery.regions.count + capsule->needs.regions > SIZE_MAX / 2 ||
        (to->nursery.regions.count + capsule->needs.regions) * 2 >
            to->nursery.regions.capacity) abort();
    for (index = 0; index < capsule->needs.regions; ++index) {
        kiln_nursery_chunk *chunk = kiln_c99_graph_capsule_chunk(capsule, index);
        if (chunk == NULL || kiln_c99_gc_ptrset_contains(&to->nursery.regions, chunk)) abort();
    }
    for (index = 0; index < capsule->needs.regions; ++index) {
        kiln_nursery_chunk *chunk = kiln_c99_graph_capsule_chunk(capsule, index);
        if (kiln_c99_gc_ptrset_insert(&to->nursery.regions, chunk) != KILN_C99_OK) abort();
        if (to->nursery.lo == NULL || (char *)chunk < to->nursery.lo)
            to->nursery.lo = (char *)chunk;
        if (to->nursery.hi == NULL || (char *)chunk + to->nursery.capacity > to->nursery.hi)
            to->nursery.hi = (char *)chunk + to->nursery.capacity;
        kiln_c99_gc_bound(to, chunk, to->nursery.capacity);
        chunk->mark_epoch = 0;
        chunk->candidate_epoch = 0;
        chunk->candidate_id = 0;
        chunk->candidate_state = 0;
        chunk->marked_count = 0;
        chunk->marked_bytes = 0;
        chunk->next = to->nursery.full;
        to->nursery.full = chunk;
    }
    to->count += capsule->needs.objects;
    to->bytes += capsule->needs.bytes;
    to->since_collection += capsule->needs.bytes;
    to->nursery.full_bytes += capsule->needs.filled;
    if (capsule->needs.foreign_dictionary_keys) to->foreign_dictionary_keys = 1;
    realm->graph_inbox_root = capsule->root;
    realm->graph_inbox_state = KILN_C99_GRAPH_INBOX_PUBLISHED;
    capsule->chunk = NULL;
    capsule->detached = 0;
    capsule->adopted = 1;
}

/* Receiver adoption has already installed the exact root. The pair coordinator must synchronize
 * the receiver callback's completion before calling this on the sender lane, and must keep both
 * realms stopped throughout. The entry deletion and root clearing cannot allocate or fail. */
static void kiln_c99_graph_capsule_commit_sender(kiln_gc_heap *from,
                                                  const kiln_c99_graph_capsule *capsule,
                                                  uint64_t request_id) {
    if (capsule == NULL || !capsule->adopted || capsule->detached || capsule->chunk != NULL)
        abort();
    kiln_c99_actor_isolated_request_commit_published(from, request_id, capsule->root);
}

/* Both turns remain claimed for this whole sequence. The sender owns the capsule allocation
 * even while the detached chunk is briefly lent to the receiver. Every fallible operation is
 * before unlink. An invariant failure after unlink is process-fatal because resuming either
 * realm would expose a dangling sender reference or an unrooted receiver graph. */
static int kiln_c99_graph_capsule_paused_control(uint32_t kind, uint64_t first,
                                                 uint64_t second, uint64_t third) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    kiln_c99_realm_state *realm;
    kiln_c99_graph_capsule *capsule;
    void *opaque = NULL;
    int prepared = 0;
    if (heap == NULL) return 3;
    realm = &heap->realm;
    switch (kind) {
    case 3: /* sender certify, prepare and stage: request, reservation, receiver */
        if (first == 0 || second == 0 || third == 0 ||
            realm->graph_outgoing_capsule != NULL) return 3;
        if (realm->isolated_request_id != first || realm->isolated_request_invalidated) {
            (void)kiln_c99_actor_isolated_request_rollback(heap, first);
            return 2;
        }
        capsule = kiln_c99_prepared_graph_capsule(heap, realm->isolated_request_handle);
        prepared = capsule != NULL;
        if (capsule == NULL) {
            capsule = (kiln_c99_graph_capsule *)malloc(sizeof *capsule);
            if (capsule == NULL) {
                (void)kiln_c99_actor_isolated_request_rollback(heap, first);
                return 3;
            }
            if (!kiln_c99_graph_capsule_certify_request(heap, first, capsule)) {
                kiln_c99_graph_capsule_dispose(capsule);
                free(capsule);
                return 2;
            }
        }
        if (kiln_realm_graph_capsule_stage(second, third, first, capsule) != 0) {
            (void)kiln_c99_actor_isolated_request_rollback(heap, first);
            if (!prepared) {
                kiln_c99_graph_capsule_dispose(capsule);
                free(capsule);
            }
            return 3;
        }
        realm->graph_outgoing_capsule = capsule;
        realm->graph_outgoing_reservation = second;
        realm->graph_outgoing_prepared = prepared;
        return 0;
    case 4: /* receiver preflight: reservation, sender, request */
        if (first == 0 || second == 0 || third == 0 ||
            kiln_realm_graph_capsule_peek(first, second, third, &opaque) != 0 ||
            opaque == NULL) return 3;
        capsule = (kiln_c99_graph_capsule *)opaque;
        return kiln_c99_graph_capsule_receiver_preflight(heap, &capsule->needs, first) ? 0 : 3;
    case 5: /* sender unlink: reservation, sender, request */
        if (first == 0 || second == 0 || third == 0 ||
            realm->graph_outgoing_reservation != first ||
            realm->isolated_request_id != third ||
            kiln_realm_graph_capsule_peek(first, second, third, &opaque) != 0 ||
            opaque == NULL || opaque != realm->graph_outgoing_capsule) return 3;
        capsule = (kiln_c99_graph_capsule *)opaque;
        if (realm->graph_outgoing_prepared) {
            if (capsule->detached || capsule->adopted ||
                capsule != kiln_c99_prepared_graph_capsule(heap, realm->isolated_request_handle))
                return 3;
            capsule->detached = 1;
        } else if (!kiln_c99_graph_capsule_detach(heap, capsule)) return 3;
        if (kiln_realm_graph_capsule_detached(first, third) != 0) abort();
        return 0;
    case 6: /* receiver take and adopt: reservation, sender, request */
        if (first == 0 || second == 0 || third == 0 ||
            kiln_realm_graph_capsule_take(first, second, third, &opaque) != 0 ||
            opaque == NULL) abort();
        capsule = (kiln_c99_graph_capsule *)opaque;
        kiln_c99_graph_capsule_adopt(heap, capsule, first);
        return 0;
    case 7: /* sender consume escrow after receiver adoption: request, reservation */
        capsule = (kiln_c99_graph_capsule *)realm->graph_outgoing_capsule;
        if (first == 0 || second == 0 || third != 0 || capsule == NULL ||
            realm->graph_outgoing_reservation != second ||
            realm->isolated_request_id != first || !capsule->adopted) abort();
        if (realm->graph_outgoing_prepared) {
            if (realm->isolated_request_invalidated ||
                realm->isolated_request_handle.tag != KILN_VALUE_OBJECT) abort();
            kiln_c99_prepared_graph_commit(heap, realm->isolated_request_handle, capsule);
            realm->isolated_request_handle = kiln_c99_undefined();
            realm->isolated_request_id = 0;
            realm->isolated_request_invalidated = 0;
        } else {
            kiln_c99_graph_capsule_commit_sender(heap, capsule, first);
            kiln_c99_graph_capsule_dispose(capsule);
            free(capsule);
        }
        realm->graph_outgoing_capsule = NULL;
        realm->graph_outgoing_reservation = 0;
        realm->graph_outgoing_prepared = 0;
        return 0;
    case 8: /* receiver pre-unlink rollback: reservation */
        return second == 0 && third == 0 &&
            kiln_c99_graph_receiver_rollback(heap, first) ? 0 : 3;
    case 9: /* sender pre-unlink cancel: request, reservation */
        capsule = (kiln_c99_graph_capsule *)realm->graph_outgoing_capsule;
        if (first == 0 || second == 0 || third != 0 || capsule == NULL ||
            realm->graph_outgoing_reservation != second || capsule->detached ||
            kiln_realm_graph_capsule_cancel(second, first) != 0) abort();
        realm->graph_outgoing_capsule = NULL;
        realm->graph_outgoing_reservation = 0;
        if (!realm->graph_outgoing_prepared) {
            kiln_c99_graph_capsule_dispose(capsule);
            free(capsule);
        }
        realm->graph_outgoing_prepared = 0;
        return kiln_c99_actor_isolated_request_rollback(heap, first) ? 0 : 3;
    default:
        return 3;
    }
}
