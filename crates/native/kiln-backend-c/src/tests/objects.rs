use super::*;

#[test]
fn final_builtin_layout_shares_descriptors_but_not_values_or_mutations() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    static const char *names[] = {"aa", "bb", "cc", "dd"};
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    kiln_value first, second, read, descriptor, keys;
    kiln_object *a, *b;
    int deleted, found;
    if (heap == NULL) return 1;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    kiln_test_heap = heap;
    kiln_test_generation += 1;
    for (size_t i = 0; i < 4; ++i)
        if (kiln_c99_intern_static(names[i], 2) == NULL) return 2;
    if (kiln_c99_new_object(&first) != KILN_C99_OK ||
        kiln_c99_new_object(&second) != KILN_C99_OK) return 3;
    a = kiln_c99_object_of(first);
    b = kiln_c99_object_of(second);
    for (size_t i = 0; i < 4; ++i) {
        if (kiln_c99_define_method(first, names[i], 2, kiln_c99_number((double)i)) != KILN_C99_OK ||
            kiln_c99_define_method(second, names[i], 2, kiln_c99_number((double)(i + 10))) !=
                KILN_C99_OK) return 4;
    }
    if (kiln_c99_ensure_dictionary(a) != KILN_C99_OK ||
        kiln_c99_ensure_dictionary(b) != KILN_C99_OK) return 5;
    kiln_c99_final_builtin_store(KILN_VALUE_BUILTIN, KILN_C99_BUILTIN_OBJECT_PROTOTYPE, a);
    kiln_c99_final_builtin_store(KILN_VALUE_BUILTIN, KILN_C99_BUILTIN_OBJECT_PROTOTYPE, b);
    if (a->shape == NULL || a->shape != b->shape ||
        (a->shape->shared & KILN_C99_SHAPE_FINAL) == 0 ||
        a->properties != NULL || b->properties != NULL || a->shape->parent != NULL) return 6;
    if (kiln_c99_get(NULL, first, "aa", 2, &read, 0, NULL) != KILN_C99_OK ||
        kiln_c99_as_number(read) != 0.0 ||
        kiln_c99_get(NULL, second, "aa", 2, &read, 0, NULL) != KILN_C99_OK ||
        kiln_c99_as_number(read) != 10.0) return 7;
    if (kiln_c99_describe_property(NULL, second, "aa", 2, &descriptor, &found, 0, NULL) !=
            KILN_C99_OK || !found ||
        kiln_c99_get(NULL, descriptor, "value", 5, &read, 0, NULL) != KILN_C99_OK ||
        kiln_c99_as_number(read) != 10.0 ||
        kiln_c99_get(NULL, descriptor, "enumerable", 10, &read, 0, NULL) != KILN_C99_OK ||
        kiln_c99_truthy(read) || b->shape != a->shape) return 11;
    if (kiln_c99_all_own_keys(NULL, second, &keys, 0, NULL) != KILN_C99_OK ||
        kiln_c99_array_of(keys)->count != 4 || b->shape != a->shape) return 12;
    if (kiln_c99_put(NULL, first, "aa", 2, kiln_c99_number(99.0), 0, NULL) != KILN_C99_OK ||
        kiln_c99_get(NULL, second, "aa", 2, &read, 0, NULL) != KILN_C99_OK ||
        kiln_c99_as_number(read) != 10.0) return 8;
    if (kiln_c99_define_method(first, "extra", 5, kiln_c99_number(5.0)) != KILN_C99_OK ||
        a->shape != NULL || b->shape == NULL || a->count != 5) return 9;
    if (kiln_c99_delete(NULL, second, "bb", 2, &deleted, 0, NULL) != KILN_C99_OK ||
        !deleted || b->shape != NULL || b->count != 3 || a->count != 5) return 10;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "final-builtin-layout");
    assert!(
        run.status.success(),
        "final built-in layout failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn compact_property_records_trace_the_live_data_or_accessor_arm() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0)).expect("unit emits");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap heap;
    kiln_value object, data, getter, setter;
    kiln_property *slot;
    kiln_gc_block *data_block, *getter_block, *setter_block;
    if (sizeof(kiln_property) != 48) return 1;
    kiln_c99_gc_heap_init(&heap);
    kiln_test_heap = &heap;
    kiln_test_generation += 1;
    heap.enabled = 0;
    if (kiln_c99_new_object(&object) != KILN_C99_OK) return 2;
    data = kiln_c99_copy_text("data", 4);
    getter = kiln_c99_copy_text("getter", 6);
    setter = kiln_c99_copy_text("setter", 6);
    if (data.tag != KILN_VALUE_STRING || getter.tag != KILN_VALUE_STRING ||
        setter.tag != KILN_VALUE_STRING) return 3;
    if (kiln_c99_own_slot(object, "data", 4, &slot) != KILN_C99_OK) return 4;
    slot->value = data;
    if (kiln_c99_define_accessor_key(object, kiln_c99_intern_static("accessor", 8),
                                     getter, setter, 1) != KILN_C99_OK) return 5;
    if (kiln_c99_own_index(kiln_c99_object_of(object), NULL, "data", 4) ==
        kiln_c99_object_of(object)->count) return 6;
    data_block = kiln_c99_gc_lookup(&heap, (void *)(uintptr_t)data.bits);
    getter_block = kiln_c99_gc_lookup(&heap, (void *)(uintptr_t)getter.bits);
    setter_block = kiln_c99_gc_lookup(&heap, (void *)(uintptr_t)setter.bits);
    if (data_block == NULL || getter_block == NULL || setter_block == NULL) return 7;
    kiln_c99_nursery_turn_chunk(&heap);
    kiln_c99_gc_begin(&heap);
    kiln_c99_gc_mark_value(&heap, &object);
    kiln_c99_gc_drain(&heap);
    if (data_block->mark != heap.epoch || getter_block->mark != heap.epoch ||
        setter_block->mark != heap.epoch) return 8;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "compact-property-records");
    assert!(
        run.status.success(),
        "compact property fixture failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn weak_collections_mark_values_to_a_fixed_point_and_clear_dead_keys() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0)).expect("unit emits");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap heap;
    kiln_gc_heap other;
    kiln_value map, first_key, second_key, bridge, final_value;
    kiln_collection *collection;
    kiln_gc_block *final_block;
    kiln_c99_gc_heap_init(&heap);
    kiln_test_heap = &heap;
    kiln_test_generation += 1;
    heap.enabled = 0;
    if (kiln_c99_new_collection(&map, 1, 1) != KILN_C99_OK) return 1;
    if (kiln_c99_new_array(&first_key) != KILN_C99_OK ||
        kiln_c99_new_array(&second_key) != KILN_C99_OK ||
        kiln_c99_new_array(&bridge) != KILN_C99_OK ||
        kiln_c99_new_array(&final_value) != KILN_C99_OK) return 2;
    if (kiln_c99_array_push(bridge, second_key) != KILN_C99_OK) return 3;
    /* Second entry precedes the first in the scan: it takes another pass to find its key. */
    if (kiln_c99_collection_put(map, second_key, final_value) != KILN_C99_OK ||
        kiln_c99_collection_put(map, first_key, bridge) != KILN_C99_OK) return 4;
    collection = kiln_c99_collection_of(map);
    final_block = kiln_c99_gc_lookup(&heap, (void *)(uintptr_t)final_value.bits);
    if (final_block == NULL || heap.weak_collections != collection) return 5;
    kiln_c99_nursery_turn_chunk(&heap);
    kiln_c99_gc_begin(&heap);
    kiln_c99_gc_mark_word(&heap, KILN_VAL_TAG_PTR(KILN_TAG_SHARED_PREFIX,
                                                 (void *)(uintptr_t)first_key.bits));
    if (kiln_c99_gc_lookup(&heap, (void *)(uintptr_t)first_key.bits)->mark != heap.epoch)
        return 12;
    kiln_c99_gc_begin(&heap);
    kiln_c99_gc_mark_value(&heap, &map);
    kiln_c99_gc_mark_value(&heap, &first_key);
    kiln_c99_gc_drain(&heap);
    kiln_c99_gc_weak_ephemerons(&heap);
    if (final_block->mark != heap.epoch || collection->live != 2) return 6;
    kiln_c99_gc_begin(&heap);
    kiln_c99_gc_mark_value(&heap, &map);
    kiln_c99_gc_drain(&heap);
    kiln_c99_gc_weak_ephemerons(&heap);
    if (collection->live != 0 || !collection->entries[0].dead ||
        !collection->entries[1].dead || collection->entries[0].key.tag != KILN_VALUE_UNDEFINED)
        return 7;
    kiln_c99_gc_release_cell(collection);
    if (heap.weak_collections != NULL) return 8;
    kiln_c99_gc_heap_init(&other);
    if (kiln_c99_new_collection(&map, 1, 1) != KILN_C99_OK) return 9;
    collection = kiln_c99_collection_of(map);
    kiln_collection *moved = (kiln_collection *)kiln_c99_gc_splice_sublist(&heap, &other, collection);
    if (moved == NULL || moved == collection || heap.weak_collections != NULL ||
        other.weak_collections != moved || moved->weak_owner != &other) return 10;
    kiln_test_heap = &other;
    kiln_test_generation += 1;
    kiln_c99_gc_release_cell(moved);
    if (other.weak_collections != NULL) return 11;
    kiln_collection *fast = kiln_c99_collection_alloc_fast(1, 1);
    if (fast == NULL || other.weak_collections != fast || fast->weak_owner != &other)
        return 13;
    kiln_c99_gc_release_cell(fast);
    if (other.weak_collections != NULL) return 14;
    kiln_c99_nursery_destroy(&heap.nursery);
    kiln_c99_nursery_destroy(&other.nursery);
    free(heap.large_table.table);
    free(heap.slab_table.table);
    free(other.large_table.table);
    free(other.slab_table.table);
    free(heap.stack);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "weak-ephemerons");
    assert!(
        run.status.success(),
        "weak ephemeron fixture failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// ADR 0071, realms issue 04 step 3: a process-wide site keeps what every realm can read and
/// forgets, when a realm is torn down, everything that realm is about to free -- its realm-local
/// shapes, and a realm-local key a list-mode holder was checked by. Which built-ins are modified
/// is each realm's own record, and a list-mode object's layout token is never reused.
/// Realms issue 04 step 4: shared cache state is published so a reader on another thread never
/// pairs one answer's key with another's words. A replaced answer is rewritten in place only while
/// no other thread runs a realm; otherwise it is retired, left untouched, and reused only once every
/// other thread has passed a turn boundary. The fixture stands for those threads
/// (`kiln_test_other_threads`, `kiln_test_safe_below`).
#[test]
fn a_replaced_answer_is_retired_until_every_reader_has_passed_a_boundary() {
    let generated =
        emit(&program(vec![], vec![Instruction::Return], 0)).expect("a minimal unit emits");
    let wrapper = r#"
static kiln_c99_site kiln_test_site;
static kiln_c99_method_path kiln_test_path = KILN_C99_UNLEARNED_METHOD_PATH;
int main(void) {
    kiln_shape *root = kiln_c99_shape_root();
    kiln_shape *shapes[9];
    char name[3] = { 'k', '0', 0 };
    uint32_t i;
    for (i = 0; i < 9; ++i) {
        name[1] = (char)('0' + i);
        if (!kiln_c99_shape_transition(root, kiln_c99_intern_static(name, 2), 0, &shapes[i]) ||
            !shapes[i]->shared)
            return 1;
    }
    /* Alone, a replacement is the in-place overwrite it always was. */
    kiln_c99_site_remember_own(&kiln_test_site, shapes[0], 0);
    kiln_c99_site_remember_own(&kiln_test_site, shapes[0], 1);
    if (kiln_test_site.pic_count != 1 || kiln_c99_site_slot_for(&kiln_test_site, shapes[0]) != 1)
        return 2;
    /* With other threads running realms, the old answer is retired and left whole, and the new
     * one goes in a fresh entry. */
    kiln_test_other_threads = 1;
    kiln_c99_site_remember_own(&kiln_test_site, shapes[0], 2);
    if (!KILN_C99_SLOT_IS_RETIRED(kiln_test_site.entries[0].shape) ||
        kiln_test_site.entries[0].slot != 1)
        return 3;
    if (kiln_test_site.pic_count != 2 || kiln_c99_site_slot_for(&kiln_test_site, shapes[0]) != 2)
        return 4;
    for (i = 1; i < 7; ++i) kiln_c99_site_remember_own(&kiln_test_site, shapes[i], i);
    if (kiln_test_site.pic_count != KILN_C99_PIC_SIZE) return 5;
    /* Full, and the retired entry not yet safe: evicting the last only retires it. */
    kiln_c99_site_remember_own(&kiln_test_site, shapes[8], 8);
    if (kiln_c99_site_slot_for(&kiln_test_site, shapes[8]) != UINT32_MAX) return 6;
    if (!KILN_C99_SLOT_IS_RETIRED(kiln_test_site.entries[KILN_C99_PIC_SIZE - 1].shape)) return 7;
    /* Every other thread has passed a boundary: a retired entry is written again. */
    kiln_test_safe_below = kiln_test_epoch;
    kiln_c99_site_remember_own(&kiln_test_site, shapes[8], 8);
    if (kiln_c99_site_slot_for(&kiln_test_site, shapes[8]) != 8) return 8;
    for (i = 1; i < 6; ++i)
        if (kiln_c99_site_slot_for(&kiln_test_site, shapes[i]) != i) return 9;
    if (kiln_c99_site_slot_for(&kiln_test_site, shapes[6]) != UINT32_MAX) return 9;

    /* A method path is one slot: relearning it for another receiver retires it first. */
    kiln_test_path.proto = shapes[1];
    kiln_test_path.slot = 3;
    kiln_c99_slot_publish(&kiln_test_path.recv, shapes[0]);
    if (kiln_c99_slot_claim(&kiln_test_path.recv, 1)) return 10;
    if (!KILN_C99_SLOT_IS_RETIRED(kiln_test_path.recv) || kiln_test_path.slot != 3) return 11;
    if (kiln_c99_slot_claim(&kiln_test_path.recv, 0)) return 12;
    kiln_test_safe_below = kiln_test_epoch;
    if (!kiln_c99_slot_claim(&kiln_test_path.recv, 0)) return 13;
    if (kiln_test_path.recv != KILN_C99_SLOT_CLAIMED || kiln_c99_slot_claim(&kiln_test_path.recv, 1))
        return 14;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "slot-protocol");
    assert!(
        run.status.success(),
        "a replaced answer rewritten under another thread's reader ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn a_site_forgets_a_torn_down_realms_local_shapes_and_keys() {
    let generated = emit(&program(
        vec![Constant::String("gamma".into())],
        vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::GetProperty(0),
            Instruction::Return,
        ],
        0,
    ))
    .expect("a unit naming one key emits");
    let wrapper = r#"
static kiln_gc_heap *kiln_test_new_heap(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    if (heap == NULL) return NULL;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    return heap;
}
static void kiln_test_enter(kiln_gc_heap *heap) {
    kiln_test_heap = heap;
    kiln_test_generation += 1;
}
static kiln_c99_site kiln_test_site;
static int kiln_test_site_names(const kiln_shape *shape, const kiln_string *key) {
    uint32_t p;
    for (p = 0; p < kiln_test_site.pic_count; ++p)
        if (kiln_test_site.entries[p].shape == shape && kiln_test_site.entries[p].key == key) return 1;
    return 0;
}
int main(void) {
    kiln_gc_heap *first;
    kiln_gc_heap *second;
    kiln_shape *root = kiln_c99_shape_root();
    const kiln_string *alpha, *xray;
    kiln_shape *a, *ax;
    kiln_c99_pic_entry held;
    kiln_value list;
    kiln_object *level;
    const kiln_shape *named;
    uint32_t token, again;

    first = kiln_test_new_heap();
    if (first == NULL) return 1;
    kiln_test_enter(first);
    alpha = kiln_c99_intern_static("alpha", 5);
    xray = kiln_c99_intern("xray", 4);
    if (!kiln_c99_key_is_shared(alpha) || kiln_c99_key_is_shared(xray)) return 2;
    if (!kiln_c99_shape_transition(root, alpha, 0, &a) ||
        !kiln_c99_shape_transition(a, xray, 0, &ax)) return 3;
    if (!a->shared || ax->shared) return 4;

    /* Only shared shapes: the site joins no heap. */
    kiln_c99_site_remember_own(&kiln_test_site, a, 0);
    if (kiln_test_site.registered != 0) return 5;
    /* A realm-local shape, and a list-mode holder checked by a realm-local key: it joins. */
    kiln_c99_site_remember_own(&kiln_test_site, ax, 1);
    if (kiln_test_site.registered != first->serial) return 6;
    memset(&held, 0, sizeof held);
    held.shape = root;
    held.slot = UINT32_MAX;
    held.depth = 1;
    held.key = xray;
    kiln_c99_site_remember(&kiln_test_site, &held);
    if (kiln_test_site.pic_count != 3 || !kiln_test_site_names(root, xray)) return 7;

    /* Which built-ins are modified is the realm's own record. */
    kiln_c99_realm_note_builtin_store(KILN_C99_BUILTIN_MATH);
    if (!kiln_c99_realm_builtin_modified(KILN_C99_BUILTIN_MATH)) return 8;
    if (kiln_c99_realm_builtin_modified(KILN_C99_BUILTIN_JSON)) return 9;

    /* A list-mode level's token: issued once, cleared by a mutation, never issued twice. */
    if (kiln_c99_new_object(&list) != KILN_C99_OK) return 10;
    level = kiln_c99_object_of(list);
    if (kiln_c99_ensure_dictionary(level) != KILN_C99_OK || level->shape != NULL) return 11;
    if (!kiln_c99_pic_name_level(level, &named, &token) || named != NULL || token == 0) return 12;
    if (!kiln_c99_pic_name_level(level, &named, &again) || again != token) return 13;
    kiln_c99_prototype_touched(level);
    if (level->layout_token != 0) return 14;
    if (!kiln_c99_pic_name_level(level, &named, &again) || again == token) return 15;

    /* A second realm: its own record, and it reads the shared entry the first one left. */
    second = kiln_test_new_heap();
    if (second == NULL) return 16;
    kiln_test_enter(second);
    if (kiln_c99_realm_builtin_modified(KILN_C99_BUILTIN_MATH)) return 17;
    if (kiln_c99_site_slot_for(&kiln_test_site, a) != 0) return 18;

    /* The first realm goes: the site forgets its shape and its key -- the entries are retired in
     * place, never moved under a reader -- and keeps the shared one. */
    kiln_c99_gc_teardown_begin(first);
    kiln_c99_gc_teardown_end(first);
    if (kiln_test_site.pic_count != 3 || kiln_c99_site_slot_for(&kiln_test_site, a) != 0) return 19;
    if (kiln_test_site.registered != 0) return 20;
    if (!KILN_C99_SLOT_IS_RETIRED(kiln_test_site.entries[1].shape) ||
        !KILN_C99_SLOT_IS_RETIRED(kiln_test_site.entries[2].shape) ||
        kiln_test_site_names(root, xray))
        return 21;
    kiln_c99_gc_teardown_begin(second);
    kiln_c99_gc_teardown_end(second);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "site-forgets-local");
    assert!(
        run.status.success(),
        "a site keeping a torn-down realm's shapes or keys ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Realms issue 17: a symbol-keyed site answers per (shape, key); an entry keyed by a realm-local
/// symbol may hand a receiver the realm-local shape a store moves it to even from a shared shape
/// (no other realm holds that key), and is forgotten with the realm; a shared symbol key's entry
/// stays. A private-field site learns a realm-local shape and forgets it the same way.
#[test]
fn symbol_keyed_and_private_sites_learn_realm_local_shapes_and_forget_them() {
    let generated = emit(&program(
        vec![Constant::String("gamma".into())],
        vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::GetProperty(0),
            Instruction::Return,
        ],
        0,
    ))
    .expect("a unit naming one key emits");
    let wrapper = r#"
static kiln_gc_heap *kiln_test_new_heap(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    if (heap == NULL) return NULL;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    return heap;
}
static void kiln_test_enter(kiln_gc_heap *heap) {
    kiln_test_heap = heap;
    kiln_test_generation += 1;
}
static kiln_c99_symbol_cell kiln_test_cell;
static kiln_c99_private_site kiln_test_private;
int main(void) {
    kiln_gc_heap *first;
    kiln_gc_heap *second;
    kiln_shape *root = kiln_c99_shape_root();
    const kiln_string *ka, *kb, *iter, *alpha;
    kiln_shape *sa, *sab, *salpha, *siter;
    kiln_c99_pic_entry entry;
    const kiln_c99_pic_entry *found;
    kiln_c99_site plain;
    kiln_c99_site *site;

    first = kiln_test_new_heap();
    if (first == NULL) return 1;
    kiln_test_enter(first);
    /* Two `Symbol()` keys (realm-local), a well-known one and a constant (shared). */
    ka = kiln_c99_intern_pinned_key(&(kiln_string){11, KILN_STRING_LATIN1, "@@symbol:91"});
    kb = kiln_c99_intern_pinned_key(&(kiln_string){11, KILN_STRING_LATIN1, "@@symbol:92"});
    iter = kiln_c99_intern_static("@@iterator", 10);
    alpha = kiln_c99_intern_static("alpha", 5);
    if (kiln_c99_key_is_shared(ka) || kiln_c99_key_is_shared(kb)) return 2;
    if (!kiln_c99_key_is_shared(iter) || !kiln_c99_key_is_shared(alpha)) return 3;
    if (!kiln_c99_shape_transition(root, ka, 0, &sa) || !kiln_c99_shape_transition(sa, kb, 0, &sab) ||
        !kiln_c99_shape_transition(root, alpha, 0, &salpha) ||
        !kiln_c99_shape_transition(salpha, iter, 0, &siter))
        return 4;
    if (sa->shared || sab->shared || !salpha->shared || !siter->shared) return 5;

    /* A store from the shared root to the realm-local child: refused unkeyed, taken keyed. */
    memset(&plain, 0, sizeof plain);
    memset(&entry, 0, sizeof entry);
    entry.shape = root;
    entry.transition_to = sa;
    entry.slot = 0;
    entry.key = ka;
    kiln_c99_site_remember(&plain, &entry);
    if (plain.pic_count != 0) return 6;
    kiln_c99_symbol_site_remember(&kiln_test_cell, &entry, ka);
    site = kiln_test_cell;
    if (site == NULL || kiln_c99_site_entry_keyed(site, root, ka) == NULL) return 7;
    if (site->registered != first->serial) return 8;

    /* One shape read under two keys keeps two entries; each answers only for its own key. */
    memset(&entry, 0, sizeof entry);
    entry.shape = sab;
    entry.slot = 0;
    kiln_c99_symbol_site_remember(&kiln_test_cell, &entry, ka);
    memset(&entry, 0, sizeof entry);
    entry.shape = sab;
    entry.slot = 1;
    kiln_c99_symbol_site_remember(&kiln_test_cell, &entry, kb);
    found = kiln_c99_site_entry_keyed(site, sab, ka);
    if (found == NULL || found->slot != 0) return 9;
    found = kiln_c99_site_entry_keyed(site, sab, kb);
    if (found == NULL || found->slot != 1) return 10;
    if (kiln_c99_site_entry_keyed(site, sab, iter) != NULL) return 11;
    /* Relearning one key's answer replaces that entry, not the other key's. */
    memset(&entry, 0, sizeof entry);
    entry.shape = sab;
    entry.slot = 0;
    kiln_c99_symbol_site_remember(&kiln_test_cell, &entry, ka);
    if (site->pic_count != 3) return 12;

    /* A shared shape read under a shared symbol key: kept across realms. */
    memset(&entry, 0, sizeof entry);
    entry.shape = siter;
    entry.slot = 1;
    kiln_c99_symbol_site_remember(&kiln_test_cell, &entry, iter);
    if (site->pic_count != 4) return 13;

    /* A private site learns a realm-local shape, registering with the realm first. */
    kiln_c99_private_site_learn(&kiln_test_private, sab, UINT32_MAX, 1);
    if (kiln_c99_slot_key(&kiln_test_private.shape) != sab ||
        kiln_test_private.registered != first->serial)
        return 14;

    second = kiln_test_new_heap();
    if (second == NULL) return 15;
    kiln_test_enter(second);
    /* Another realm's `Symbol()` of the same description is another key: no answer. */
    if (kiln_c99_site_entry_keyed(site, root,
                                  kiln_c99_intern_pinned_key(
                                      &(kiln_string){11, KILN_STRING_LATIN1, "@@symbol:93"})) != NULL)
        return 16;
    if (kiln_c99_site_entry_keyed(site, siter, iter) == NULL) return 17;

    /* The first realm goes: every entry naming its keys or shapes is retired, the shared one
     * stays, and so does nothing in the private site. */
    kiln_c99_gc_teardown_begin(first);
    kiln_c99_gc_teardown_end(first);
    if (kiln_c99_site_entry_keyed(site, root, ka) != NULL ||
        kiln_c99_site_entry_keyed(site, sab, ka) != NULL ||
        kiln_c99_site_entry_keyed(site, sab, kb) != NULL)
        return 18;
    found = kiln_c99_site_entry_keyed(site, siter, iter);
    if (found == NULL || found->slot != 1) return 19;
    if (!KILN_C99_SLOT_IS_RETIRED(kiln_test_private.shape) || kiln_test_private.registered != 0)
        return 20;
    kiln_c99_gc_teardown_begin(second);
    kiln_c99_gc_teardown_end(second);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "symbol-sites-forget-local");
    assert!(
        run.status.success(),
        "a symbol-keyed or private site and a torn-down realm's keys and shapes ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Issue 04 step 3b: definitions keep an object shaped, with the attributes the list-mode record
/// would carry recorded in its shape; a transition is keyed by (key, attributes); dictionary mode
/// materializes the same records; a method cache names a shaped prototype one step up.
#[test]
fn definitions_keep_objects_shaped_with_attributes() {
    let generated = emit(&program(
        vec![Constant::String("m".into())],
        vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::GetProperty(0),
            Instruction::Return,
        ],
        0,
    ))
    .expect("a unit naming one key emits");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    kiln_value proto, plain, receiver, getter, descriptor, read, keys, builtin_store;
    kiln_object *p, *q, *r;
    const kiln_string *m;
    kiln_c99_site site;
    kiln_c99_pic_entry entry;
    uint32_t slot;
    int found;
    if (heap == NULL) return 1;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    kiln_test_heap = heap;
    kiln_test_generation += 1;
    m = kiln_c99_intern_static("m", 1);

    /* A method definition is a transition with attributes, not a trip to dictionary mode. */
    if (kiln_c99_new_object(&proto) != KILN_C99_OK) return 2;
    p = kiln_c99_object_of(proto);
    if (kiln_c99_define_method(proto, "m", 1, kiln_c99_number(1.0)) != KILN_C99_OK) return 3;
    if (p->shape == NULL || p->properties != NULL) return 4;
    if (p->shape->attrs[0] != KILN_C99_ATTR_ENUM_FLIP || p->shape->special_mask != 1u ||
        p->shape->store_mask != 0u || !p->shape->shared) return 5;
    /* An assignment of the same key from the same shape takes a different edge. */
    if (kiln_c99_new_object(&plain) != KILN_C99_OK) return 6;
    q = kiln_c99_object_of(plain);
    if (kiln_c99_put(NULL, plain, "m", 1, kiln_c99_number(1.0), 0, NULL) != KILN_C99_OK) return 7;
    if (q->shape == NULL || q->shape == p->shape || q->shape->parent != p->shape->parent ||
        q->shape->special_mask != 0u) return 8;
    /* A second object defined the same way shares the first one's shape. */
    if (kiln_c99_new_object(&plain) != KILN_C99_OK) return 9;
    q = kiln_c99_object_of(plain);
    if (kiln_c99_define_method(plain, "m", 1, kiln_c99_number(2.0)) != KILN_C99_OK) return 10;
    if (q->shape != p->shape) return 11;

    /* An accessor keeps its pair in a slot; a private method is read-only. */
    getter = kiln_c99_number(7.0);
    if (kiln_c99_define_accessor(proto, "g", 1, getter, kiln_c99_undefined(), 0) != KILN_C99_OK)
        return 12;
    if (p->shape == NULL || (p->shape->accessor_mask & 2u) == 0 || (p->shape->store_mask & 2u) == 0 ||
        p->accessor_count != 1) return 13;
    if (kiln_c99_define_private(proto, "@@private:9:x", 13, kiln_c99_number(3.0), 0) != KILN_C99_OK)
        return 14;
    if (p->shape == NULL || p->shape->attrs[2] != KILN_C99_ATTR_READONLY || !p->read_only_seen)
        return 15;

    /* A method cache names the shaped prototype one step up and reads the slot live. */
    if (kiln_c99_new_object(&receiver) != KILN_C99_OK) return 16;
    r = kiln_c99_object_of(receiver);
    r->prototype = proto;
    memset(&site, 0, sizeof site);
    memset(&entry, 0, sizeof entry);
    entry.shape = r->shape;
    entry.slot = UINT32_MAX;
    entry.key = m;
    {
        const kiln_object *holder = NULL;
        if (kiln_c99_pic_learn_path(&entry, r, m, &holder) != KILN_C99_PATH_FOUND ||
            holder != p) return 17;
    }
    kiln_c99_site_remember(&site, &entry);
    if (!site.entries[0].one_step) return 18;
    if (kiln_c99_site_cached_callee(&site, r) != kiln_val_from_boundary(kiln_c99_number(1.0)))
        return 19;
    p->slots[0] = kiln_val_from_boundary(kiln_c99_number(5.0));
    if (kiln_c99_site_cached_callee(&site, r) != kiln_val_from_boundary(kiln_c99_number(5.0)))
        return 20;
    /* The accessor's slot is never a path's answer. */
    memset(&entry, 0, sizeof entry);
    entry.shape = r->shape;
    entry.slot = UINT32_MAX;
    {
        const kiln_object *holder = NULL;
        if (kiln_c99_pic_learn_path(&entry, r, kiln_c99_intern_static("g", 1), &holder) !=
            KILN_C99_PATH_NONE) return 21;
    }

    /* Dictionary mode carries exactly the records the list path would have made. */
    if (!kiln_c99_shape_find_slot(p->shape, m, &slot) || slot != 0) return 22;
    if (kiln_c99_describe_property(NULL, proto, "m", 1, &descriptor, &found, 0, NULL) !=
            KILN_C99_OK || !found || p->shape == NULL || p->properties != NULL) return 28;
    if (kiln_c99_get(NULL, descriptor, "value", 5, &read, 0, NULL) != KILN_C99_OK ||
        read.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(read) != 5.0) return 29;
    if (kiln_c99_get(NULL, descriptor, "enumerable", 10, &read, 0, NULL) != KILN_C99_OK ||
        kiln_c99_truthy(read)) return 30;
    if (kiln_c99_describe_property(NULL, proto, "g", 1, &descriptor, &found, 0, NULL) !=
            KILN_C99_OK || !found || p->shape == NULL || p->properties != NULL) return 31;
    if (kiln_c99_get(NULL, descriptor, "get", 3, &read, 0, NULL) != KILN_C99_OK ||
        read.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(read) != 7.0) return 32;
    if (kiln_c99_get(NULL, descriptor, "writable", 8, &read, 0, NULL) != KILN_C99_OK ||
        read.tag != KILN_VALUE_UNDEFINED) return 33;
    if (kiln_c99_describe_property(NULL, proto, "missing", 7, &descriptor, &found, 0, NULL) !=
            KILN_C99_OK || found || p->shape == NULL || p->properties != NULL) return 34;
    if (kiln_c99_all_own_keys(NULL, proto, &keys, 0, NULL) != KILN_C99_OK ||
        p->shape == NULL || p->properties != NULL || kiln_c99_array_of(keys)->count != 2)
        return 35;
    if (kiln_c99_define_method(kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE),
                               "trial", 5, kiln_c99_number(1.0)) != KILN_C99_OK ||
        kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE),
                                &builtin_store, 0) != KILN_C99_OK) return 36;
    if (kiln_c99_new_array(&keys) != KILN_C99_OK ||
        kiln_c99_builtin_store_keys(KILN_C99_BUILTIN_OBJECT_PROTOTYPE, keys) != KILN_C99_OK ||
        kiln_c99_array_of(keys)->count != 1 ||
        kiln_c99_object_of(builtin_store)->shape == NULL ||
        kiln_c99_object_of(builtin_store)->properties != NULL) return 37;
    if (kiln_c99_ensure_dictionary(p) != KILN_C99_OK || p->shape != NULL || p->count != 3) return 23;
    if (p->properties[0].enumerable || !p->properties[0].writable ||
        !p->properties[0].configurable || p->properties[0].accessor) return 24;
    if (!p->properties[1].accessor || p->properties[1].writable ||
        p->properties[1].getter.tag != KILN_VALUE_NUMBER ||
        p->properties[1].setter.tag != KILN_VALUE_UNDEFINED) return 25;
    if (p->properties[2].writable || p->properties[2].enumerable) return 26;
    /* The prototype left shape mode: the entry no longer answers. */
    if (kiln_c99_site_cached_callee(&site, r) != 0) return 27;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "shape-attributes");
    assert!(
        run.status.success(),
        "definitions and attribute-bearing shapes disagree ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Issue 04 step 3c: a method definition records the method's code in the shape it takes, the
/// code is part of the edge, and a store that would make the record false moves the object to
/// the shape without it -- so a shape's record holds for every object that has it, and a method
/// path is only learned from a prototype shape that records the candidate's code.
#[test]
fn method_definitions_record_their_code_and_stores_keep_it_true() {
    let generated = emit(&program(
        vec![Constant::String("m".into())],
        vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::GetProperty(0),
            Instruction::Return,
        ],
        0,
    ))
    .expect("a unit naming one key emits");
    let wrapper = r#"
static int body_a(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                  const kiln_val *receiver, const kiln_val *args, size_t argc) {
    (void)ctx; (void)upvalues; (void)receiver; (void)args; (void)argc;
    *result = KILN_VAL_UNDEFINED; return KILN_C99_OK;
}
static int body_b(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                  const kiln_val *receiver, const kiln_val *args, size_t argc) {
    (void)ctx; (void)upvalues; (void)receiver; (void)args; (void)argc;
    *result = KILN_VAL_NULL; return KILN_C99_OK;
}
static const kiln_string fn_name = { 1, KILN_STRING_LATIN1, "m" };
static int closure(kiln_value *out, kiln_body_fn body) {
    return kiln_c99_make_closure(out, body, &fn_name, NULL, 0, "<test>", 0, 0, 0, 0, 0, 1, 0, 1,
                                 KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0, NULL, 0);
}
int main(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    kiln_value a1, a2, b1, protoA, protoA2, protoB, receiver, ctor;
    kiln_object *pa, *pa2, *pb, *r;
    const kiln_shape *recorded, *twin;
    const kiln_string *m;
    kiln_c99_site site;
    kiln_c99_pic_entry entry;
    kiln_c99_method_path path;
    if (heap == NULL) return 1;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    kiln_test_heap = heap;
    kiln_test_generation += 1;
    m = kiln_c99_intern_static("m", 1);
    if (closure(&a1, body_a) || closure(&a2, body_a) || closure(&b1, body_b)) return 2;

    /* Two closures of one code define one shape; another code is another shape. */
    if (kiln_c99_new_object(&protoA) || kiln_c99_new_object(&protoA2) ||
        kiln_c99_new_object(&protoB)) return 3;
    pa = kiln_c99_object_of(protoA);
    pa2 = kiln_c99_object_of(protoA2);
    pb = kiln_c99_object_of(protoB);
    if (kiln_c99_define_method(protoA, "m", 1, a1) || kiln_c99_define_method(protoA2, "m", 1, a2) ||
        kiln_c99_define_method(protoB, "m", 1, b1)) return 4;
    if (pa->shape == NULL || pa->shape != pa2->shape || pb->shape == NULL ||
        pb->shape == pa->shape || pb->shape->parent != pa->shape->parent) return 5;
    if (pa->shape->code_mask != 1u || (pa->shape->store_mask & 1u) == 0 ||
        kiln_c99_shape_slot_code(pa->shape, 0) != body_a ||
        kiln_c99_shape_slot_code(pb->shape, 0) != body_b) return 6;
    /* A number is not a closure: no record, the step 3b shape. */
    if (kiln_c99_new_object(&receiver)) return 7;
    if (kiln_c99_define_method(receiver, "m", 1, kiln_c99_number(1.0))) return 8;
    if (kiln_c99_object_of(receiver)->shape->code_mask != 0u ||
        kiln_c99_object_of(receiver)->shape->parent != pa->shape->parent) return 9;
    /* No record for a prototype's `constructor`. */
    if (kiln_c99_new_object(&ctor) || kiln_c99_define_method(ctor, "constructor", 11, a1)) return 10;
    if (kiln_c99_object_of(ctor)->shape == NULL || kiln_c99_object_of(ctor)->shape->code_mask != 0u)
        return 11;

    /* A method path is learned only from a prototype shape recording the candidate's code. */
    if (kiln_c99_new_object(&receiver)) return 12;
    r = kiln_c99_object_of(receiver);
    r->prototype = protoA;
    memset(&site, 0, sizeof site);
    memset(&entry, 0, sizeof entry);
    entry.shape = r->shape;
    entry.slot = UINT32_MAX;
    entry.key = m;
    {
        const kiln_object *holder = NULL;
        if (kiln_c99_pic_learn_path(&entry, r, m, &holder) != KILN_C99_PATH_FOUND) return 13;
    }
    kiln_c99_site_remember(&site, &entry);
    {
        static const kiln_c99_method_path unlearned = KILN_C99_UNLEARNED_METHOD_PATH;
        path = unlearned;
    }
    /* An unlearned path matches no receiver, a dictionary-mode one included. */
    if (path.recv == NULL || path.recv == r->shape) return 14;
    kiln_c99_method_path_learn(&path, &site, r, body_b, NULL, 0);
    if (path.recv == r->shape) return 14;
    kiln_c99_method_path_learn(&path, &site, r, body_a, NULL, 1);
    if (path.recv == r->shape) return 15;
    kiln_c99_method_path_learn(&path, &site, r, body_a, NULL, 0);
    if (path.recv != r->shape || path.proto != pa->shape || path.slot != 0) return 16;
    {
        const kiln_object *level = NULL;
        if (!kiln_c99_method_path_hit(&path, r, &level) || level != pa) return 17;
        /* The other prototype of the same shape hits too: the code is the same. */
        r->prototype = protoA2;
        if (!kiln_c99_method_path_hit(&path, r, &level) || level != pa2) return 18;
        r->prototype = protoB;
        if (kiln_c99_method_path_hit(&path, r, &level)) return 19;
        r->prototype = protoA;
    }

    /* Storing a closure of the same code keeps the record and the shape. */
    recorded = pa->shape;
    if (kiln_c99_put(NULL, protoA, "m", 1, a2, 0, NULL) != KILN_C99_OK) return 20;
    if (pa->shape != recorded || pa->slots[0] != kiln_val_from_boundary(a2)) return 21;
    /* Another code: the object moves to the shape without the record, same layout. */
    if (kiln_c99_put(NULL, protoA, "m", 1, b1, 0, NULL) != KILN_C99_OK) return 22;
    if (pa->shape == NULL || pa->shape == recorded || pa->shape->code_mask != 0u ||
        pa->shape->attrs[0] != KILN_C99_ATTR_ENUM_FLIP || pa->shape->store_mask != 0u ||
        pa->slots[0] != kiln_val_from_boundary(b1)) return 23;
    /* ...a sibling of the recorded one, and the path no longer hits. */
    if (pa->shape->parent != recorded->parent) return 24;
    {
        const kiln_object *level = NULL;
        if (kiln_c99_method_path_hit(&path, r, &level)) return 25;
    }
    /* The twin prototype keeps its record. */
    if (pa2->shape != recorded) return 26;
    /* A non-function store drops it too; a later slot keeps its own record. */
    if (kiln_c99_define_method(protoA2, "n", 1, b1)) return 27;
    twin = pa2->shape;
    if (twin->code_mask != 3u) return 28;
    if (kiln_c99_put(NULL, protoA2, "m", 1, kiln_c99_number(4.0), 0, NULL) != KILN_C99_OK) return 29;
    if (pa2->shape == twin || pa2->shape->code_mask != 2u || pa2->shape->property_count != 2 ||
        kiln_c99_shape_slot_code(pa2->shape, 1) != body_b ||
        pa2->slots[0] != kiln_val_from_boundary(kiln_c99_number(4.0)) ||
        pa2->slots[1] != kiln_val_from_boundary(b1)) return 30;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "shape-method-code");
    assert!(
        run.status.success(),
        "method code records disagree ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// ADR 0071, keys and shapes: grow the shape tree from two heaps in turn on one thread, the way
/// a thread hosting several realms does, and check that constant keys and the shapes built from
/// them are the same pointers in both, that each realm's run-time keys stay its own, that no edge
/// either realm can follow names the other's key, and that tearing one heap down frees its shapes
/// without disturbing the other's.
///
/// Step 1b: the unit names `gamma` as a property key, so its load-time constructor registers it
/// before `main` and fills the unit's key static from the shared table. `gamma` first interned
/// while both heaps are live is therefore the shared key, where before registration it was
/// realm-local; `epsilon`, which no unit names, keeps the run-time rule.
#[test]
fn two_heaps_share_constant_keys_and_shapes_but_not_run_time_ones() {
    let generated = emit(&program(
        vec![Constant::String("gamma".into())],
        vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::GetProperty(0),
            Instruction::Return,
        ],
        0,
    ))
    .expect("a unit naming one key emits");
    let symbol = &generated.symbol;
    let unit_key = if symbol == "ku" {
        "ku_keys[0]".to_owned()
    } else {
        format!("{symbol}_key_0")
    };
    let wrapper = r#"
static kiln_gc_heap *kiln_test_new_heap(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    if (heap == NULL) return NULL;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    return heap;
}
static void kiln_test_enter(kiln_gc_heap *heap) {
    kiln_test_heap = heap;
    kiln_test_generation += 1;
}
/* Whether any edge a shared shape owns -- which every realm follows -- names `key`. */
static int kiln_test_shared_edge_names(const kiln_shape *shape, const kiln_string *key) {
    const kiln_shape_transition *edge;
    uint32_t index;
    for (index = 0; index < 2 && index < shape->num_transitions; ++index)
        if (shape->inline_trans_ikey[index] == key) return 1;
    for (edge = shape->transitions; edge != NULL; edge = edge->next_transition)
        if (edge->ikey == key) return 1;
    return 0;
}
static int kiln_test_owns(const kiln_gc_heap *heap, const kiln_shape *shape) {
    const kiln_shape *each;
    for (each = heap->local_shapes; each != NULL; each = each->owned_next)
        if (each == shape) return 1;
    return 0;
}
int main(void) {
    kiln_gc_heap *first;
    kiln_gc_heap *second;
    kiln_shape *root = kiln_c99_shape_root();
    const kiln_string *alpha1, *beta1, *xray1, *alpha2, *beta2, *xray2, *gamma2, *delta2;
    const kiln_string *epsilon2, *gamma1, *late;
    kiln_shape *a1, *ab1, *ax1, *a2, *ab2, *ax2, *ag2, *ae2, *again;
    kiln_value object;
    kiln_string late_view = {4, KILN_STRING_LATIN1, "zeta"};
    const kiln_string *late_source = &late_view;
    /* Before any heap: the unit's constructor registered its key and filled its static from the
     * shared table, and the prelude's registered the built-ins'. */
    if (!UNIT_KEYS_SHARED || UNIT_KEY == NULL) return 40;
    if (!kiln_c99_key_is_shared(UNIT_KEY)) return 41;
    if (kiln_c99_process()->live_heaps != 0) return 42;
    first = kiln_test_new_heap();
    if (first == NULL) return 1;
    kiln_test_enter(first);
    if (!root->shared) return 2;
    if (kiln_c99_intern("", 0) != kiln_c99_key_empty) return 3;
    if (!kiln_c99_key_is_shared(kiln_c99_key_empty)) return 43;
    /* The built-ins' keys, registered before the first heap: a run-time intern answers shared. */
    if (!kiln_c99_key_is_shared(kiln_c99_intern("statusCode", 10)) ||
        !kiln_c99_key_is_shared(kiln_c99_intern("value", 5)) ||
        !kiln_c99_key_is_shared(kiln_c99_intern("done", 4))) return 44;
    /* Registration after a heap exists cannot vouch for the spelling, so it refuses. */
    if (kiln_c99_register_static_keys(&late_source, NULL, 1) != 0) return 45;
    if (kiln_c99_interned("zeta", 4) != NULL) return 46;

    /* The first realm, alone: constants join the shared table, a run-time key stays its own. */
    alpha1 = kiln_c99_intern_static("alpha", 5);
    beta1 = kiln_c99_intern_static("beta", 4);
    xray1 = kiln_c99_intern("xray", 4);
    if (alpha1 == NULL || beta1 == NULL || xray1 == NULL) return 4;
    if (!kiln_c99_key_is_shared(alpha1) || !kiln_c99_key_is_shared(beta1)) return 5;
    if (kiln_c99_key_is_shared(xray1)) return 6;
    if (kiln_c99_interned("alpha", 5) != alpha1 || kiln_c99_intern("alpha", 5) != alpha1) return 7;
    if (!kiln_c99_shape_transition(root, alpha1, 0, &a1) ||
        !kiln_c99_shape_transition(a1, beta1, 0, &ab1) ||
        !kiln_c99_shape_transition(a1, xray1, 0, &ax1)) return 8;
    if (!a1->shared || !ab1->shared || ax1->shared) return 9;
    if (!kiln_test_owns(first, ax1) || kiln_test_owns(first, a1)) return 10;
    if (kiln_test_shared_edge_names(a1, xray1)) return 11;
    if (kiln_c99_new_object(&object) != KILN_C99_OK ||
        kiln_c99_object_of(object)->shape != root) return 12;

    /* The second realm, while the first is alive. */
    second = kiln_test_new_heap();
    if (second == NULL) return 13;
    kiln_test_enter(second);
    if (kiln_c99_shape_root() != root) return 14;
    alpha2 = kiln_c99_intern_static("alpha", 5);
    beta2 = kiln_c99_intern("beta", 4);
    xray2 = kiln_c99_intern("xray", 4);
    /* Two realms live, and `gamma` is met here for the first time -- but a unit registered it
     * before the first realm, so it is the shared key, the very pointer the unit holds. */
    gamma2 = kiln_c99_intern_static("gamma", 5);
    /* `epsilon` no unit names: a new constant cannot be shared without risking a clash while
     * two realms live, so it is ours. */
    epsilon2 = kiln_c99_intern_static("epsilon", 7);
    if (alpha2 != alpha1 || beta2 != beta1) return 15;
    if (xray2 == NULL || xray2 == xray1 || kiln_c99_key_is_shared(xray2)) return 16;
    if (gamma2 == NULL || !kiln_c99_key_is_shared(gamma2) || gamma2 != UNIT_KEY) return 17;
    if (epsilon2 == NULL || kiln_c99_key_is_shared(epsilon2)) return 47;
    if (!kiln_c99_shape_transition(root, alpha2, 0, &a2) ||
        !kiln_c99_shape_transition(a2, beta2, 0, &ab2)) return 18;
    if (a2 != a1 || ab2 != ab1) return 19;
    if (!kiln_c99_shape_transition(a2, xray2, 0, &ax2) ||
        !kiln_c99_shape_transition(a2, gamma2, 0, &ag2) ||
        !kiln_c99_shape_transition(a2, epsilon2, 0, &ae2)) return 20;
    if (ax2 == ax1 || ax2->shared || !ag2->shared || ae2->shared) return 21;
    if (ax2->keys[1] != xray2 || ax2->ikey != xray2) return 22;
    if (!kiln_test_owns(second, ax2) || kiln_test_owns(second, ax1)) return 23;
    if (kiln_test_owns(second, ag2) || !kiln_test_owns(second, ae2)) return 48;
    if (kiln_test_shared_edge_names(a1, xray2) || kiln_test_shared_edge_names(a1, epsilon2)) return 24;
    /* The first realm's edge is invisible here: its key is not ours, and asking by our own key
     * made our own shape. */
    if (kiln_c99_local_edge_row(second, a1, xray1, 1u << 1, 0, 0, NULL) != NULL) return 25;

    /* Back to the first: its own edges, and the shared ones, unchanged. */
    kiln_test_enter(first);
    if (!kiln_c99_shape_transition(a1, xray1, 0, &again) || again != ax1) return 26;
    if (!kiln_c99_shape_transition(root, alpha1, 0, &again) || again != a1) return 27;
    if (kiln_c99_interned("epsilon", 7) != NULL) return 28;
    /* The shared `gamma` edge the second realm grew is the first realm's too. */
    gamma1 = kiln_c99_intern("gamma", 5);
    if (gamma1 != gamma2 || !kiln_c99_shape_transition(a1, gamma1, 0, &again) || again != ag2)
        return 49;

    /* Tear the first down: its shapes go with it, and the second is untouched. */
    kiln_c99_gc_teardown_begin(first);
    kiln_c99_gc_teardown_end(first);
    kiln_test_enter(second);
    if (!kiln_c99_shape_transition(a1, xray2, 0, &again) || again != ax2) return 29;
    if (!kiln_c99_shape_transition(a1, beta2, 0, &again) || again != ab1) return 30;
    /* Alone again: a new constant is shared, and one this realm already made stays its own. */
    delta2 = kiln_c99_intern_static("delta", 5);
    if (delta2 == NULL || !kiln_c99_key_is_shared(delta2)) return 31;
    if (kiln_c99_intern_static("epsilon", 7) != epsilon2 || kiln_c99_key_is_shared(epsilon2))
        return 32;
    late = kiln_c99_intern_static("gamma", 5);
    if (late != UNIT_KEY) return 50;
    kiln_c99_gc_teardown_begin(second);
    kiln_c99_gc_teardown_end(second);
    return 0;
}
"#;
    let wrapper = wrapper
        .replace("UNIT_KEYS_SHARED", &format!("{symbol}_keys_shared"))
        .replace("UNIT_KEY", &unit_key);
    let run = run_with_main(&generated, &wrapper, "two-heap-shapes");
    assert!(
        run.status.success(),
        "two heaps sharing keys and shapes failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Realms issue 08, the shared immutable region: the first realm to reach a region site builds
/// the region and makes the closure there; once its bootstrap is done the region is sealed
/// read-only and every other realm gets that same closure. What a program can change about the
/// closure -- its `prototype`, a property written onto it -- is each realm's own, kept in the
/// realm's context rather than the closure. The collector never marks into the region, and the
/// no-write read path consults caches without teaching them, computes a string's ASCII answer
/// without storing it and looks keys up without interning them.
#[test]
fn the_shared_region_holds_one_closure_and_each_realm_keeps_its_own_parts() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static kiln_gc_heap *kiln_test_new_heap(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    if (heap == NULL) return NULL;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    return heap;
}
static void kiln_test_enter(kiln_gc_heap *heap) {
    kiln_test_heap = heap;
    kiln_test_generation += 1;
}
static int kiln_test_body(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                          const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {
    (void)ctx; (void)upvalues; (void)receiver_ptr; (void)args; (void)argc;
    *result = KILN_VAL_UNDEFINED;
    return KILN_C99_OK;
}
static const kiln_string kiln_test_fn_name = { 4, KILN_STRING_LATIN1, "func" };
static kiln_closure *kiln_test_site;
static int kiln_test_make(kiln_value *out) {
    return kiln_c99_region_closure(out, &kiln_test_site, (kiln_body_fn)kiln_test_body,
                                   &kiln_test_fn_name, NULL, 0, "<test>", 0, 2, 0, 0, 0, 0, 0, 1,
                                   KILN_VAL_UNDEFINED, 1, 1, 1, 0, 1);
}
#if defined(__linux__)
/* The protection of the page holding `pointer`, from the process's own map. */
static int kiln_test_page_writable(const void *pointer) {
    FILE *maps = fopen("/proc/self/maps", "r");
    char line[512];
    int writable = -1;
    if (maps == NULL) return -1;
    while (fgets(line, sizeof line, maps) != NULL) {
        unsigned long lo, hi;
        char perms[8];
        if (sscanf(line, "%lx-%lx %7s", &lo, &hi, perms) == 3 && (uintptr_t)pointer >= lo &&
            (uintptr_t)pointer < hi) {
            writable = perms[1] == 'w';
            break;
        }
    }
    fclose(maps);
    return writable;
}
#endif
int main(void) {
    kiln_gc_heap *first;
    kiln_gc_heap *second;
    kiln_value fa, fb, proto_a, proto_b, again, holder, renamed_key, other_key;
    kiln_closure *closure;
    size_t blocks;
    first = kiln_test_new_heap();
    if (first == NULL) return 1;
    kiln_test_enter(first);
    if (kiln_c99_process()->region_state != KILN_C99_REGION_EMPTY) return 2;
    /* The first realm at a region site builds the region. */
    if (kiln_test_make(&fa) != KILN_C99_OK) return 3;
    closure = kiln_c99_closure_of(fa);
    if (!closure->region || !kiln_c99_in_region(closure) || kiln_test_site != closure) return 4;
    if (first->region_mode != KILN_C99_REGION_MODE_BUILDER) return 5;
    if (kiln_c99_process()->region_state != KILN_C99_REGION_BUILDING) return 6;
    if (kiln_c99_gc_payload_kind(closure) != KILN_GC_CLOSURE) return 7;
    /* Named while it builds: the name is copied into the region. */
    renamed_key = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)kiln_c99_intern("renamed", 7), 7);
    if (kiln_c99_set_function_name(fa, renamed_key, NULL) != KILN_C99_OK) return 8;
    if (closure->name == NULL || !kiln_c99_in_region(closure->name) || closure->name->unit_len != 7)
        return 9;
    /* Its prototype and a property are the first realm's, not the closure's. */
    if (kiln_c99_prototype_object(fa, &proto_a) != KILN_C99_OK || proto_a.tag != KILN_VALUE_OBJECT)
        return 10;
    if (closure->own_prototype.tag != KILN_VALUE_UNDEFINED) return 11;
    if (kiln_c99_put(NULL, fa, "marker", 6, kiln_c99_number(1.0), 0, NULL) != KILN_C99_OK) return 12;
    if (closure->own_store.tag != KILN_VALUE_UNDEFINED ||
        kiln_c99_closure_store(closure).tag != KILN_VALUE_OBJECT) return 13;
    /* A collection with the closure held by a realm object: nothing in the region is marked or
     * freed, and the realm's own parts survive through the realm context. */
    if (kiln_c99_new_object(&holder) != KILN_C99_OK ||
        kiln_c99_put(NULL, holder, "f", 1, fa, 0, NULL) != KILN_C99_OK) return 14;
    blocks = kiln_c99_process()->region_blocks;
    first->enabled = 1;
    kiln_c99_gc_collect(first, 1);
    first->enabled = 0;
    if (kiln_c99_process()->region_blocks != blocks || closure->body != (kiln_body_fn)kiln_test_body)
        return 15;
    if (kiln_c99_prototype_object(fa, &again) != KILN_C99_OK || again.bits != proto_a.bits) return 16;
    /* The bootstrap is done: the region is sealed, read-only, and every realm reads it. */
    kiln_c99_builtin_bootstrap_done();
    if (kiln_c99_process()->region_state != KILN_C99_REGION_SEALED) return 17;
    if (first->region_mode != KILN_C99_REGION_MODE_USER) return 18;
#if defined(__linux__)
    if (kiln_test_page_writable(closure) != 0) return 19;
#endif
    second = kiln_test_new_heap();
    if (second == NULL) return 20;
    kiln_test_enter(second);
    if (kiln_test_make(&fb) != KILN_C99_OK || kiln_c99_closure_of(fb) != closure) return 21;
    if (second->region_mode != KILN_C99_REGION_MODE_USER) return 22;
    /* None of the first realm's changes is visible here. */
    if (kiln_c99_closure_store(closure).tag != KILN_VALUE_UNDEFINED) return 23;
    if (kiln_c99_prototype_object(fb, &proto_b) != KILN_C99_OK || proto_b.bits == proto_a.bits)
        return 24;
    /* A sealed closure is not renamed: the builder named it, as every realm's definition would. */
    other_key = kiln_c99_value(KILN_VALUE_STRING, 0, (const char *)kiln_c99_intern("other", 5), 5);
    if (kiln_c99_set_function_name(fb, other_key, NULL) != KILN_C99_OK ||
        closure->name->unit_len != 7) return 25;
    /* And the first realm still sees its own. */
    kiln_test_enter(first);
    if (kiln_c99_closure_store(closure).tag != KILN_VALUE_OBJECT) return 26;
    if (kiln_c99_prototype_object(fa, &again) != KILN_C99_OK || again.bits != proto_a.bits) return 27;

    /* The no-write read path, reading the first realm's heap. */
    {
        kiln_value text = kiln_c99_copy_text("plain", 5);
        kiln_gc_block *block;
        kiln_c99_no_write_frame frame;
        kiln_c99_site site;
        uint32_t before;
        if (text.tag != KILN_VALUE_STRING) return 28;
        block = kiln_c99_gc_lookup(first, text.ptr);
        if (block == NULL) return 29;
        before = block->kind;
        memset(&site, 0, sizeof site);
        kiln_test_enter(second);
        kiln_c99_no_write_enter(first, &frame);
        if (!kiln_c99_no_write() || kiln_c99_gc_heap() != first) return 30;
        if (!kiln_c99_string_is_ascii(text) || block->kind != before) return 31;
        if (kiln_c99_intern("never interned", 14) != NULL) return 32;
        if (kiln_c99_intern("marker", 6) == NULL) return 33;
        kiln_c99_site_remember_own(&site, kiln_c99_shape_root(), 0);
        if (kiln_c99_site_count(&site) != 0) return 34;
        if (kiln_c99_closure_store(closure).tag != KILN_VALUE_OBJECT) return 35;
        kiln_c99_no_write_leave(&frame);
        if (kiln_c99_no_write() || kiln_c99_gc_heap() != second) return 36;
        kiln_test_enter(first);
        if (!kiln_c99_string_is_ascii(text) || (block->kind & KILN_GC_FLAG_ASCII_KNOWN) == 0) return 37;
        kiln_c99_site_remember_own(&site, kiln_c99_shape_root(), 0);
        if (kiln_c99_site_count(&site) == 0) return 38;
    }
    kiln_c99_gc_teardown_begin(second);
    kiln_c99_gc_teardown_end(second);
    kiln_c99_gc_teardown_begin(first);
    kiln_c99_gc_teardown_end(first);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "shared-region");
    assert!(
        run.status.success(),
        "the shared region failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// A realm that began building the region and was torn down before sealing it abandons it: no
/// later realm waits for it or reads it, and each makes its own closures as before.
#[test]
fn a_region_whose_builder_is_torn_down_unsealed_is_abandoned() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static kiln_gc_heap *kiln_test_new_heap(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    if (heap == NULL) return NULL;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    return heap;
}
static void kiln_test_enter(kiln_gc_heap *heap) {
    kiln_test_heap = heap;
    kiln_test_generation += 1;
}
static int kiln_test_body(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                          const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {
    (void)ctx; (void)upvalues; (void)receiver_ptr; (void)args; (void)argc;
    *result = KILN_VAL_UNDEFINED;
    return KILN_C99_OK;
}
static kiln_closure *kiln_test_site;
static int kiln_test_make(kiln_value *out) {
    return kiln_c99_region_closure(out, &kiln_test_site, (kiln_body_fn)kiln_test_body, NULL, NULL,
                                   0, "<test>", 0, 0, 0, 0, 0, 0, 0, 1, KILN_VAL_UNDEFINED, 1, 1,
                                   1, 0, 1);
}
int main(void) {
    kiln_gc_heap *builder = kiln_test_new_heap();
    kiln_gc_heap *later;
    kiln_value made;
    if (builder == NULL) return 1;
    kiln_test_enter(builder);
    if (kiln_test_make(&made) != KILN_C99_OK || !kiln_c99_closure_of(made)->region) return 2;
    kiln_c99_gc_teardown_begin(builder);
    kiln_c99_gc_teardown_end(builder);
    if (kiln_c99_process()->region_state != KILN_C99_REGION_ABANDONED) return 3;
    later = kiln_test_new_heap();
    if (later == NULL) return 4;
    kiln_test_enter(later);
    if (kiln_test_make(&made) != KILN_C99_OK) return 5;
    if (kiln_c99_closure_of(made)->region || kiln_c99_in_region(kiln_c99_closure_of(made))) return 6;
    if (later->region_mode != KILN_C99_REGION_MODE_NONE) return 7;
    kiln_c99_gc_teardown_begin(later);
    kiln_c99_gc_teardown_end(later);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "abandoned-region");
    assert!(
        run.status.success(),
        "an abandoned region was used ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_runtime_readable_has_a_checked_native_prototype_graph() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int readable_child(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                          const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {
    (void)ctx; (void)upvalues; (void)args; (void)argc;
    *result = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;
    return KILN_C99_OK;
}
static const kiln_string readable_child_name = { 5, KILN_STRING_LATIN1, "Child" };
int main(void) {
    uint64_t realm;
    kiln_value readable = kiln_c99_method(KILN_C99_METHOD_NODE_READABLE);
    kiln_value child;
    kiln_value result;
    kiln_value instance;
    kiln_value method;
    kiln_value readable_prototype;
    kiln_value emitter_prototype;
    kiln_value child_prototype;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_callable_prototype(readable, &readable_prototype) != KILN_C99_OK) return 1;
    if (kiln_c99_callable_prototype(kiln_c99_method(KILN_C99_METHOD_NODE_EVENT_EMITTER),
                                    &emitter_prototype) != KILN_C99_OK) return 2;
    if (kiln_c99_object_of(readable_prototype)->prototype.bits != emitter_prototype.bits) return 3;
    if (kiln_c99_get(NULL, readable_prototype, "push", 4, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_READABLE_PUSH) return 4;
    if (kiln_c99_get(NULL, readable_prototype, "on", 2, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 5;
    if (kiln_c99_make_closure(&child, readable_child, &readable_child_name, "<test>", 6, "<test>", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 6;
    if (kiln_c99_callable_prototype(child, &child_prototype) != KILN_C99_OK ||
        kiln_c99_set_prototype(child_prototype, readable_prototype) != KILN_C99_OK) return 7;
    if (kiln_c99_construct(NULL, &instance, child, NULL, 0, 0, &thrown) != KILN_C99_OK) return 8;
    if (kiln_c99_get(NULL, instance, "push", 4, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_READABLE_PUSH) return 9;
    if (kiln_c99_get(NULL, instance, "on", 2, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 10;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 11;
}
"#;
    let run = run_with_main(&generated, wrapper, "native-readable-prototype-graph");
    assert!(
        run.status.success(),
        "native Readable prototype graph failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_intrinsic_prototype_tables_can_be_relinked() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value intrinsic = kiln_c99_builtin(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
    kiln_value parent;
    kiln_value observed;
    kiln_value result;
    kiln_value arguments[2];
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_new_object(&parent) != KILN_C99_OK) return 1;
    arguments[0] = intrinsic;
    arguments[1] = parent;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_OBJECT_SET_PROTOTYPE_OF,
                               kiln_c99_undefined(), arguments, 2, 0, &thrown) != KILN_C99_OK)
        return 2;
    if (result.tag != KILN_VALUE_BUILTIN || result.bits != intrinsic.bits) return 3;
    if (kiln_c99_invoke_method(NULL, &observed, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                               kiln_c99_undefined(), &intrinsic, 1, 0, &thrown) != KILN_C99_OK)
        return 4;
    if (observed.tag != KILN_VALUE_OBJECT || observed.bits != parent.bits) return 5;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "intrinsic-prototype-relink");
    assert!(
        run.status.success(),
        "intrinsic prototype relink failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_promise_constructor_exposes_a_distinct_prototype_table() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value constructor = kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE);
    kiln_value prototype;
    kiln_value method;
    kiln_value owner;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_get(NULL, constructor, "prototype", 9, &prototype, 0, &thrown) != KILN_C99_OK)
        return 1;
    if (prototype.tag != KILN_VALUE_BUILTIN ||
        prototype.bits != KILN_C99_BUILTIN_PROMISE_PROTOTYPE) return 2;
    if (prototype.bits == constructor.bits) return 3;
    if (kiln_c99_get(NULL, prototype, "then", 4, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_THEN) return 4;
    if (kiln_c99_get(NULL, prototype, "constructor", 11, &owner, 0, &thrown) != KILN_C99_OK ||
        owner.tag != KILN_VALUE_BUILTIN || owner.bits != KILN_C99_BUILTIN_PROMISE) return 5;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "promise-prototype-table");
    assert!(
        run.status.success(),
        "Promise prototype table failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_reflective_object_statics_dispatch_without_a_receiver() {
    let generated = emit(&program(
        vec![Constant::Undefined],
        vec![Instruction::Const(0), Instruction::Return],
        0,
    ))
    .expect("reflective static probe emits");
    let wrapper = r#"
int main(void) {
    uint64_t realm;
    kiln_value target;
    kiln_value descriptor;
    kiln_value key = kiln_c99_copy_text("answer", 6);
    kiln_value value = kiln_c99_number(42.0);
    kiln_value args[3];
    kiln_value result;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_new_object(&target) != KILN_C99_OK ||
        kiln_c99_new_object(&descriptor) != KILN_C99_OK) return 2;
    if (kiln_c99_put(NULL, descriptor, "value", 5, value, 0, NULL) != KILN_C99_OK) return 3;
    args[0] = target;
    args[1] = key;
    args[2] = descriptor;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY,
                               kiln_c99_undefined(), args, 3, 0, &thrown) != KILN_C99_OK) return 4;
    if (!kiln_c99_strict_equal(result, target)) return 5;
    if (kiln_c99_get(NULL, target, "answer", 6, &result, 0, &thrown) != KILN_C99_OK) return 6;
    if (result.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(result) != 42.0) return 7;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 8;
}
"#;
    let run = run_with_main(&generated, wrapper, "object-define-property-static");
    assert!(
        run.status.success(),
        "Object.defineProperty static dispatch failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Every tag `typeof` calls an object also converts, is truthy, and names itself.
///
/// The sibling of `callable_text_covers_every_callable_tag`, and it exists because the same
/// mistake had been made four more times in four more places. `KILN_VALUE_TEMPORAL` was
/// absent from `kiln_c99_text`, from `kiln_c99_truthy`, from the `Object.prototype.toString`
/// tag switch and from `kiln_c99_is_reference`, so a `Temporal` value converted to
/// `undefined`, was *falsy*, named itself `[object Object]`, and was never reduced by a
/// coercing operator at all -- `"x" + aPlainDate` appended nothing and the program carried
/// on. Promises, generators and collections were missing from the text conversion too.
///
/// Each of those is a classifier written as a list of the kinds it knows about, and a list
/// goes stale silently. `kiln_c99_typeof` is the one that cannot: a value the program can
/// hold has to answer *something* there, so its "object" arm is the closed enumeration of
/// the exotic object kinds and the right thing to check the others against.
///
/// This does not check the answers -- `conformance/native/exotic-conversions.ts` drives every
/// kind through every implicit conversion against the interpreter for that. It checks that
/// the arm exists, which is what stops the next tag from being wrong in four places at once.
#[test]
fn every_object_tag_converts_and_is_truthy_and_names_itself() {
    let runtime = HEADER;

    fn body<'a>(runtime: &'a str, signature: &str) -> &'a str {
        let start = runtime
            .split(signature)
            .nth(1)
            .unwrap_or_else(|| panic!("{signature} must exist"));
        let end = start.find("\n}").expect("function must be closed");
        &start[..end]
    }

    let typeof_body = body(
        runtime,
        "static kiln_value kiln_c99_typeof(kiln_value value) {",
    );
    // The run of `case` labels that falls through to the "object" answer.
    let object_arm = typeof_body
        .split("case KILN_VALUE_OBJECT:")
        .nth(1)
        .expect("`kiln_c99_typeof` must answer \"object\" for something");
    let object_arm = &object_arm[..object_arm
        .find("object_text")
        .expect("the \"object\" arm must end at its answer")];
    let mut tags: Vec<&str> = vec!["KILN_VALUE_OBJECT"];
    tags.extend(
        object_arm
            .split("case ")
            .skip(1)
            .filter_map(|rest| rest.split(':').next())
            .filter(|tag| tag.starts_with("KILN_VALUE_")),
    );
    // A guard that has quietly stopped reading the thing it checks is worse than none.
    assert!(
        tags.len() >= 8,
        "read only {tags:?} out of `kiln_c99_typeof`; the parse has stopped working"
    );
    assert!(
        tags.contains(&"KILN_VALUE_TEMPORAL"),
        "the parse of `kiln_c99_typeof` has stopped working"
    );

    // An array and a plain object are deliberately outside `kiln_c99_has_intrinsic_text`:
    // their text depends on what they hold, so only the `valueOf`/`toString` walk can ask.
    let intrinsic_text = body(
        runtime,
        "static int kiln_c99_has_intrinsic_text(kiln_value value) {",
    );
    for (what, region, exempt) in [
        (
            "kiln_c99_text",
            body(
                runtime,
                "static kiln_value kiln_c99_text(kiln_value value) {",
            ),
            &[][..],
        ),
        (
            "kiln_c99_truthy",
            body(runtime, "static int kiln_c99_truthy(kiln_value value) {"),
            &[][..],
        ),
        (
            "kiln_c99_has_intrinsic_text",
            intrinsic_text,
            &["KILN_VALUE_OBJECT", "KILN_VALUE_ARRAY", "KILN_VALUE_SHARED"][..],
        ),
    ] {
        let missing: Vec<&&str> = tags
            .iter()
            .filter(|tag| !exempt.contains(tag))
            .filter(|tag| !region.contains(&format!("case {tag}:")))
            .collect();
        assert!(
            missing.is_empty(),
            "`kiln_c99_typeof` answers \"object\" for {missing:?}, which `{what}` has no \
                 case for. Each would fall to its `default:`, and every one of those defaults is \
                 a wrong answer rather than a refusal: `undefined` from the text conversion, \
                 *falsy* from the truthiness test. A `Temporal` value was in exactly that state \
                 and no probe converted one."
        );
    }
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
fn executable_native_callables_have_isolated_static_property_stores() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value stream = kiln_c99_method(KILN_C99_METHOD_NODE_STREAM);
    kiln_value readable = kiln_c99_method(KILN_C99_METHOD_NODE_READABLE);
    kiln_value buffer = kiln_c99_builtin(KILN_C99_BUILTIN_BUFFER);
    kiln_value constants;
    kiln_value read;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_put(NULL, stream, "Readable", 8, readable, 0, &thrown) != KILN_C99_OK) return 1;
    if (kiln_c99_new_object(&constants) != KILN_C99_OK) return 2;
    if (kiln_c99_put(NULL, buffer, "constants", 9, constants, 0, &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_get(NULL, stream, "Readable", 8, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_METHOD || read.bits != KILN_C99_METHOD_NODE_READABLE) return 4;
    if (kiln_c99_get(NULL, buffer, "constants", 9, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_OBJECT || read.bits != constants.bits) return 5;
    if (kiln_c99_get(NULL, stream, "constants", 9, &read, 0, &thrown) == KILN_C99_OK) return 6;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "native-callable-statics");
    assert!(
        run.status.success(),
        "native callable statics failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_bound_callables_have_isolated_own_property_stores() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value target = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
    kiln_value first;
    kiln_value second;
    kiln_value read;
    kiln_value thrown = kiln_c99_undefined();
    kiln_value marker = kiln_c99_copy_text("first", 5);
    if (kiln_c99_bind_with(&first, target, kiln_c99_undefined(), NULL, 0) != KILN_C99_OK ||
        kiln_c99_bind_with(&second, target, kiln_c99_undefined(), NULL, 0) != KILN_C99_OK)
        return 1;
    if (kiln_c99_put(NULL, first, "unwrappedName", 13, marker, 1, &thrown) != KILN_C99_OK)
        return 2;
    if (kiln_c99_get(NULL, first, "unwrappedName", 13, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_STRING || !kiln_test_units_equal(read, "first", 5))
        return 3;
    if (kiln_c99_get(NULL, second, "unwrappedName", 13, &read, 0, &thrown) == KILN_C99_OK)
        return 4;
    if (kiln_c99_get(NULL, target, "unwrappedName", 13, &read, 0, &thrown) == KILN_C99_OK)
        return 5;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "bound-callable-own-properties");
    assert!(
        run.status.success(),
        "bound callable own properties failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// A closure over a module-level binding, which is the shape every counter has: the module
/// body and the function must write through one box, and the module must still read the
/// current value out of the slot afterwards.
#[test]
fn executable_closure_shares_a_cell_with_the_module_body() {
    let bump = FunctionProto {
        inferred_name: None,
        prologue_end: 0,
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
        source: "".into(),
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
        template_sites: Vec::new(),
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

#[test]
fn executable_for_in_follows_prototypes_and_keeps_constructor_hidden() {
    let constants = vec![
        Constant::Number(1.0f64.to_bits()),
        Constant::String("prototype".into()),
        Constant::String("inherited".into()),
        Constant::Number(0.0f64.to_bits()),
        Constant::String("length".into()),
        Constant::Boolean(false),
    ];
    let generated = emit(&Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(0),
            // Touching `prototype` creates its ordinary object and its non-enumerable
            // constructor link.
            Instruction::LoadLocal(0),
            Instruction::GetProperty(1),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::Const(2),
            Instruction::Const(0),
            Instruction::SetComputed,
            Instruction::Pop,
            // `new C()` links the fresh instance to that prototype.
            Instruction::LoadLocal(0),
            Instruction::New { argc: 0 },
            Instruction::EnumerateKeys,
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(2),
            Instruction::GetProperty(4),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::JumpIfFalse(25),
            Instruction::LoadLocal(2),
            Instruction::Const(3),
            Instruction::GetComputed,
            Instruction::Const(2),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::Return,
            Instruction::Const(5),
            Instruction::Return,
        ],
        local_count: 3,
        functions: vec![function(vec![Instruction::Return])],
        template_sites: Vec::new(),
    })
    .expect("prototype enumeration probe emits");
    let wrapper = format!(
        r#"
int main(void) {{
    kiln_value result;
    if ({symbol}(NULL, &result) != KILN_C99_OK) return 2;
    return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3;
}}
"#,
        symbol = generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "prototype-enumeration");
    assert!(
        run.status.success(),
        "for-in must include enumerable prototype properties and omit constructor (exit {:?})",
        run.status.code()
    );
}

#[test]
fn executable_scalar_replacement_of_non_escaping_objects_in_loop() {
    let constants = vec![
        Constant::Number(0.0f64.to_bits()),     // c0: 0
        Constant::Number(100.0f64.to_bits()),   // c1: 100
        Constant::Number(1.0f64.to_bits()),     // c2: 1
        Constant::String("x".into()),           // c3: "x"
        Constant::String("y".into()),           // c4: "y"
        Constant::Number(10000.0f64.to_bits()), // c5: 10000
    ];

    let code = vec![
        // sum = 0
        Instruction::Const(0),
        Instruction::StoreLocal(0),
        // i = 0
        Instruction::Const(0),
        Instruction::StoreLocal(1),
        // Loop header: pc 4
        // i < 100
        Instruction::LoadLocal(1),
        Instruction::Const(1),
        Instruction::Binary(BinaryOp::LessThan),
        Instruction::JumpIfFalse(29), // Jump to exit (pc 29)
        // Body: pc 8
        // p = { x: i, y: i + 1 }
        Instruction::Const(3),              // "x"
        Instruction::LoadLocal(1),          // i
        Instruction::Const(4),              // "y"
        Instruction::LoadLocal(1),          // i
        Instruction::Const(2),              // 1
        Instruction::Binary(BinaryOp::Add), // i + 1
        Instruction::NewObject { count: 2 },
        Instruction::StoreLocal(2),
        // sum = sum + p.x + p.y: pc 16
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(2),
        Instruction::GetProperty(3), // p.x
        Instruction::Binary(BinaryOp::Add),
        Instruction::LoadLocal(2),
        Instruction::GetProperty(4), // p.y
        Instruction::Binary(BinaryOp::Add),
        Instruction::StoreLocal(0),
        // i = i + 1: pc 24
        Instruction::LoadLocal(1),
        Instruction::Const(2),
        Instruction::Binary(BinaryOp::Add),
        Instruction::StoreLocal(1),
        // Jump to header (pc 4): pc 28
        Instruction::Jump(4),
        // Exit: pc 29
        // return sum === 10000
        Instruction::LoadLocal(0),
        Instruction::Const(5),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let prog = program(constants, code, 3);
    let generated = emit(&prog).expect("scalar replacement loop emits C");
    let body_symbol = format!("{}_body", generated.symbol);
    let body_start = generated
        .source
        .find(&body_symbol)
        .expect("body function in generated source");
    let body_source = &generated.source[body_start..];

    // Zero heap allocations for the object in the function body:
    assert!(
        !body_source.contains("kiln_c99_new_object"),
        "body should not allocate any heap objects in the loop"
    );
    assert!(
        !body_source.contains("kiln_c99_val_read_property"),
        "body should not perform runtime property reads on non-escaping objects"
    );
    assert!(
        body_source.contains("_f_0"),
        "body should use scalar replaced field variables"
    );

    // Compile and execute to verify the computation is exactly correct
    let executable = std::env::temp_dir().join(format!(
        "kiln-backend-c-scalar-repl-{}-{}",
        std::process::id(),
        generated.program_hash
    ));
    let wrapper = format!(
        "int main(void) {{ kiln_value result; if ({}(NULL, &result) != 0) return 2; return !(result.tag == KILN_VALUE_BOOLEAN && result.bits == 1); }}\n",
        generated.symbol
    );
    let mut child = Command::new("cc")
        .args([
            "-std=c99",
            "-pedantic",
            "-Werror=implicit-function-declaration",
            "-x",
            "c",
            "-",
            "-lm",
            "-o",
        ])
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
        .expect("scalar replacement source is accepted by cc stdin");
    let compile = child.wait_with_output().expect("cc exits");
    assert!(
        compile.status.success(),
        "generated scalar replacement C did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("generated scalar replacement executable runs");
    let _ = std::fs::remove_file(&executable);
    assert!(
        run.status.success(),
        "scalar replacement execution failed with status: {:?}, stderr: {}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_gc_block_sublist_splice_and_destructive_transfer() {
    let program = program(vec![], vec![Instruction::Return], 0);
    let generated = emit(&program).expect("program emits");
    let wrapper = r#"
int main(void) {
    /* 1. Initialize two disjoint heaps A and B */
    kiln_gc_heap heap_a;
    kiln_gc_heap heap_b;
    kiln_c99_gc_heap_init(&heap_a);
    kiln_c99_gc_heap_init(&heap_b);
    heap_a.enabled = 1;
    heap_b.enabled = 1;

    /* 2. Allocate a large tenured block in heap_a */
    size_t payload_size = 1024 * 1024; /* 1 MB payload */
    kiln_gc_block *block = (kiln_gc_block *)malloc(sizeof(kiln_gc_block) + payload_size);
    if (block == NULL) return 1;
    block->kind = KILN_GC_BYTES | KILN_GC_FLAG_LARGE;
    block->mark = 0;
    block->size = payload_size;
    block->prev = NULL;
    block->next = heap_a.head;
    if (heap_a.head != NULL) heap_a.head->prev = block;
    heap_a.head = block;
    kiln_c99_gc_bound(&heap_a, block, sizeof(kiln_gc_block) + payload_size);
    if (kiln_c99_gc_ptrset_insert(&heap_a.large_table, block) != KILN_C99_OK) return 2;
    heap_a.count += 1;
    heap_a.bytes += payload_size;

    void *payload_ptr = block + 1;
    memset(payload_ptr, 0x42, payload_size);

    /* Verify initial state in heap_a */
    if (heap_a.count != 1 || heap_a.bytes != payload_size) return 3;
    if (heap_b.count != 0 || heap_b.bytes != 0) return 4;
    if (!kiln_c99_gc_ptrset_contains(&heap_a.large_table, block)) return 5;
    if (kiln_c99_gc_ptrset_contains(&heap_b.large_table, block)) return 6;

    /* 3. Splice from heap_a to heap_b */
    void *spliced_ptr = kiln_c99_gc_splice_sublist(&heap_a, &heap_b, payload_ptr);
    if (spliced_ptr != payload_ptr) return 7; /* Zero-copy: exact same payload pointer! */

    /* Verify heap_a is now empty */
    if (heap_a.count != 0 || heap_a.bytes != 0 || heap_a.head != NULL) return 8;
    if (kiln_c99_gc_ptrset_contains(&heap_a.large_table, block)) return 9;

    /* Verify heap_b now owns the spliced block */
    if (heap_b.count != 1 || heap_b.bytes != payload_size || heap_b.head != block) return 10;
    if (!kiln_c99_gc_ptrset_contains(&heap_b.large_table, block)) return 11;
    if (block->prev != NULL || block->next != NULL) return 12;

    /* Payload bytes intact with zero copy */
    unsigned char *bytes = (unsigned char *)spliced_ptr;
    if (bytes[0] != 0x42 || bytes[payload_size - 1] != 0x42) return 13;

    /* 4. Test destructive read on sender local slot */
    kiln_val local_slot = KILN_TAG_OBJECT_PREFIX | ((uint64_t)(uintptr_t)spliced_ptr & KILN_PAYLOAD_MASK);
    kiln_c99_destructive_read_val(&local_slot);
    if (local_slot != KILN_VAL_UNDEFINED) return 14;

    kiln_value boundary_slot;
    boundary_slot.tag = KILN_VALUE_OBJECT;
    boundary_slot.len = 1;
    boundary_slot.bits = 0x1234;
    kiln_c99_destructive_read_value(&boundary_slot);
    if (boundary_slot.tag != KILN_VALUE_UNDEFINED || boundary_slot.len != 0 || boundary_slot.bits != 0) return 15;

    /* 5. Mark and sweep in heap_b */
    heap_b.epoch += 1;
    block->mark = heap_b.epoch; /* Mark root block as live */
    kiln_c99_gc_sweep(&heap_b, 0);

    /* Verified survived in heap_b */
    if (heap_b.count != 1 || heap_b.bytes != payload_size || heap_b.head != block) return 16;

    /* Sweep in heap_a (empty heap) */
    heap_a.epoch += 1;
    kiln_c99_gc_sweep(&heap_a, 0);
    if (heap_a.count != 0 || heap_a.head != NULL) return 17;

    /* Clean teardown: unmark and sweep to free block */
    heap_b.epoch += 1;
    kiln_c99_gc_sweep(&heap_b, 0);
    if (heap_b.count != 0 || heap_b.head != NULL) return 18;

    /* A buffer wrapper's moved backing block follows the heap move too. Its finalizer must
     * subtract from B, not underflow A after the block has been spliced. */
    block = (kiln_gc_block *)calloc(1, sizeof(kiln_gc_block) + sizeof(kiln_typed_buffer));
    if (block == NULL) return 19;
    block->kind = KILN_GC_TYPED_BUFFER | KILN_GC_FLAG_LARGE;
    block->size = sizeof(kiln_typed_buffer);
    heap_a.head = block;
    heap_a.count = 1;
    heap_a.bytes = block->size;
    kiln_c99_gc_bound(&heap_a, block, sizeof(kiln_gc_block) + block->size);
    if (kiln_c99_gc_ptrset_insert(&heap_a.large_table, block) != KILN_C99_OK) return 20;
    kiln_typed_buffer *typed = (kiln_typed_buffer *)(block + 1);
    typed->bytes = (uint8_t *)malloc(4096);
    if (typed->bytes == NULL) return 21;
    typed->byte_length = 4096;
    typed->owns_bytes = 1;
    typed->moved_external = 1;
    heap_a.moved_external_current = 4096;
    if (kiln_c99_gc_splice_sublist(&heap_a, &heap_b, typed) != typed) return 22;
    if (heap_a.moved_external_current != 0 || heap_b.moved_external_current != 4096) return 23;
    heap_b.epoch += 1;
    kiln_c99_gc_sweep(&heap_b, 0);
    if (heap_b.moved_external_current != 0 || heap_b.count != 0) return 24;

    /* A nursery wrapper is promoted by copying its header. The source header must give up
     * byte and shared-handle ownership before its own finalizer runs. */
    kiln_test_heap = &heap_a;
    kiln_test_generation += 1;
    typed = (kiln_typed_buffer *)kiln_c99_gc_nursery_bump(&heap_a, KILN_GC_TYPED_BUFFER,
                                                         sizeof(kiln_typed_buffer));
    if (typed == NULL) return 25;
    memset(typed, 0, sizeof *typed);
    typed->bytes = (uint8_t *)malloc(2048);
    if (typed->bytes == NULL) return 26;
    typed->byte_length = 2048;
    typed->owns_bytes = 1;
    typed->moved_external = 1;
    heap_a.moved_external_current = 2048;
    kiln_typed_buffer *promoted = (kiln_typed_buffer *)kiln_c99_gc_splice_sublist(&heap_a, &heap_b, typed);
    if (promoted == NULL || promoted == typed || promoted->bytes == NULL) return 27;
    if (heap_a.moved_external_current != 0 || heap_b.moved_external_current != 2048) return 28;
    if (typed->owns_bytes || typed->moved_external || typed->shared != NULL) return 29;
    heap_b.epoch += 1;
    kiln_c99_gc_sweep(&heap_b, 0);
    if (heap_b.moved_external_current != 0 || heap_b.count != 0) return 30;

    free(heap_a.large_table.table);
    free(heap_b.large_table.table);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "gc-splice-test");
    assert!(
        run.status.success(),
        "executable gc block sublist splice test failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_gc_chunk_handoff_validates_membership_and_preserves_payload() {
    let program = program(vec![], vec![Instruction::Return], 0);
    let generated = emit(&program).expect("program emits");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap sender, receiver, ordinary;
    kiln_c99_par_arena arena, worker;
    kiln_nursery_chunk *first, *second;
    kiln_nursery_chunk *batch[2];
    void *first_value, *second_value;
    kiln_c99_gc_heap_init(&sender);
    kiln_c99_gc_heap_init(&receiver);
    kiln_c99_gc_heap_init(&ordinary);
    memset(&worker, 0, sizeof worker);
    worker.heap = &ordinary;
    worker.budget = ordinary.nursery.capacity;
    if (kiln_c99_par_alloc(&worker, KILN_GC_BYTES, 8) == NULL) return 31;
    kiln_c99_par_arena_seal(&worker);
    if (!kiln_c99_heap_adopt_chunks(&ordinary, worker.filled)) return 32;
    batch[0] = worker.filled;
    if (kiln_c99_heap_move_chunks(&ordinary, &receiver, batch, 1)) return 33;
    if (ordinary.count != 1 || receiver.count != 0) return 34;
    kiln_c99_nursery_destroy(&ordinary.nursery);

    if (!kiln_c99_graph_candidate_open(&arena, &sender, 2 * sender.nursery.capacity)) return 35;
    first_value = kiln_c99_par_alloc(&arena, KILN_GC_BYTES, 8);
    if (first_value == NULL) return 1;
    memset(first_value, 0x51, 8);
    first = arena.current;
    kiln_c99_par_arena_seal(&arena);
    second_value = kiln_c99_par_alloc(&arena, KILN_GC_BYTES, 8);
    if (second_value == NULL) return 2;
    memset(second_value, 0x52, 8);
    second = arena.current;
    kiln_c99_par_arena_seal(&arena);
    second->next = first;
    if (!kiln_c99_heap_adopt_chunks(&sender, second)) return 3;
    batch[0] = first;
    batch[1] = first;
    if (kiln_c99_heap_move_chunks(&sender, &receiver, batch, 2)) return 4;
    if (sender.count != 2 || receiver.count != 0) return 5;
    batch[1] = (kiln_nursery_chunk *)sender.nursery.region;
    if (kiln_c99_heap_move_chunks(&sender, &receiver, batch, 2)) return 6;
    if (sender.count != 2 || receiver.count != 0) return 7;
    batch[1] = second;
    /* The relink must reject a malformed or finalizable source chunk before moving either
     * candidate. The second chunk makes this an all-or-nothing batch test. */
    first->finals[0] |= UINT64_C(1);
    if (kiln_c99_heap_move_chunks(&sender, &receiver, batch, 2)) return 15;
    first->finals[0] &= ~UINT64_C(1);
    if (sender.count != 2 || receiver.count != 0) return 16;
    first->bitmap[0] |= UINT64_C(2);
    if (kiln_c99_heap_move_chunks(&sender, &receiver, batch, 2)) return 17;
    first->bitmap[0] &= ~UINT64_C(2);
    if (sender.count != 2 || receiver.count != 0) return 18;
    first->alloc_count += 1;
    if (kiln_c99_heap_move_chunks(&sender, &receiver, batch, 2)) return 19;
    first->alloc_count -= 1;
    if (sender.count != 2 || receiver.count != 0) return 20;
    if (!kiln_c99_heap_move_chunks(&sender, &receiver, batch, 2)) return 8;
    if (sender.count != 0 || receiver.count != 2 || sender.nursery.full != NULL) return 9;
    if (receiver.nursery.full_bytes != 2 * (size_t)(second->limit - second->base)) return 10;
    if (kiln_c99_gc_lookup(&sender, first_value) != NULL ||
        kiln_c99_gc_lookup(&sender, second_value) != NULL) return 11;
    if (kiln_c99_gc_lookup(&receiver, first_value) == NULL ||
        kiln_c99_gc_lookup(&receiver, second_value) == NULL) return 12;
    if (((unsigned char *)first_value)[0] != 0x51 ||
        ((unsigned char *)second_value)[0] != 0x52) return 13;
    if (kiln_c99_heap_move_chunks(&sender, &receiver, batch, 2)) return 14;
    kiln_c99_nursery_destroy(&sender.nursery);
    kiln_c99_nursery_destroy(&receiver.nursery);

    /* Separate factory candidates cannot be smuggled into one move. A collection epoch change
     * also invalidates the candidate even if its chunk still looks sealed. */
    kiln_gc_heap another_sender, another_receiver;
    kiln_c99_par_arena graph_a, graph_b;
    kiln_nursery_chunk *a, *b;
    kiln_c99_gc_heap_init(&another_sender);
    kiln_c99_gc_heap_init(&another_receiver);
    if (!kiln_c99_graph_candidate_open(&graph_a, &another_sender,
                                        another_sender.nursery.capacity) ||
        !kiln_c99_graph_candidate_open(&graph_b, &another_sender,
                                        another_sender.nursery.capacity)) return 36;
    if (kiln_c99_par_alloc(&graph_a, KILN_GC_BYTES, 8) == NULL ||
        kiln_c99_par_alloc(&graph_b, KILN_GC_BYTES, 8) == NULL) return 37;
    kiln_c99_par_arena_seal(&graph_a);
    kiln_c99_par_arena_seal(&graph_b);
    a = graph_a.filled;
    b = graph_b.filled;
    a->next = b;
    if (!kiln_c99_heap_adopt_chunks(&another_sender, a)) return 38;
    batch[0] = a;
    batch[1] = b;
    if (kiln_c99_heap_move_chunks(&another_sender, &another_receiver, batch, 2)) return 39;
    if (another_sender.count != 2 || another_receiver.count != 0) return 40;
    another_sender.epoch += 1;
    if (kiln_c99_heap_move_chunks(&another_sender, &another_receiver, batch, 1)) return 41;
    another_sender.epoch -= 1;
    if (!kiln_c99_heap_move_chunks(&another_sender, &another_receiver, batch, 1)) return 42;
    batch[0] = b;
    if (!kiln_c99_heap_move_chunks(&another_sender, &another_receiver, batch, 1)) return 43;
    if (another_sender.count != 0 || another_receiver.count != 2) return 44;
    kiln_c99_nursery_destroy(&another_sender.nursery);
    kiln_c99_nursery_destroy(&another_receiver.nursery);
    kiln_c99_graph_candidate_next_id = UINT64_MAX;
    if (kiln_c99_graph_candidate_open(&graph_a, &sender, sender.nursery.capacity)) return 45;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "gc-chunk-handoff");
    assert!(
        run.status.success(),
        "exclusive chunk handoff failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_receiver_reserves_then_publishes_a_root() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap sender, receiver;
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *batch[1], *chunk;
    kiln_val *cell;
    kiln_value root, wrong;
    uint64_t old_id = 0, id = 0;
    kiln_c99_gc_heap_init(&sender);
    kiln_c99_gc_heap_init(&receiver);
    if (!kiln_c99_graph_candidate_open(&candidate, &sender,
                                        sender.nursery.capacity)) return 1;
    cell = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *cell);
    if (cell == NULL) return 2;
    *cell = KILN_VAL_UNDEFINED;
    kiln_c99_par_arena_seal(&candidate);
    chunk = candidate.filled;
    if (chunk == NULL || !kiln_c99_heap_adopt_chunks(&sender, chunk)) return 3;
    batch[0] = chunk;
    root.tag = KILN_VALUE_CELL;
    root.bits = (uint64_t)(uintptr_t)cell;
    root.len = 0;
    root.flags = 0;
    if (!kiln_c99_graph_receiver_reserve(&receiver, 1, &old_id) || old_id == 0 ||
        receiver.realm.graph_inbox_root.tag != KILN_VALUE_UNDEFINED ||
        receiver.nursery.regions.capacity < 2 * (receiver.nursery.regions.count + 1))
        return 4;
    if (kiln_c99_graph_receiver_reserve(&receiver, 1, &id) ||
        kiln_c99_graph_receiver_rollback(&receiver, old_id + 1) ||
        kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, root,
                                                   old_id + 1)) return 5;
    if (!kiln_c99_graph_receiver_rollback(&receiver, old_id) ||
        receiver.realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_EMPTY) return 6;
    if (!kiln_c99_graph_receiver_reserve(&receiver, 1, &id) || id <= old_id ||
        kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, root,
                                                   old_id)) return 7;
    wrong = root;
    wrong.tag = KILN_VALUE_STRING;
    if (kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, wrong, id) ||
        sender.count != 1 || receiver.count != 0) return 8;
    receiver.epoch++;
    if (kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, root, id))
        return 9;
    receiver.epoch--;
    chunk->finals[0] |= UINT64_C(1);
    if (kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, root, id) ||
        sender.count != 1 || receiver.count != 0 ||
        receiver.realm.graph_inbox_root.tag != KILN_VALUE_UNDEFINED) return 10;
    chunk->finals[0] &= ~UINT64_C(1);
    if (!kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, root, id))
        return 11;
    if (sender.count != 0 || receiver.count != 1 ||
        kiln_c99_gc_lookup(&sender, cell) != NULL ||
        kiln_c99_gc_lookup(&receiver, cell) == NULL ||
        receiver.realm.graph_inbox_root.tag != root.tag ||
        receiver.realm.graph_inbox_root.bits != root.bits ||
        receiver.realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_PUBLISHED) return 12;
    kiln_c99_gc_mark_realm(&receiver);
    if (((kiln_gc_block *)cell - 1)->mark != receiver.epoch) return 14;
    if (kiln_c99_graph_receiver_rollback(&receiver, id) ||
        kiln_c99_graph_receiver_reserve(&receiver, 1, &old_id)) return 13;
    kiln_c99_nursery_destroy(&sender.nursery);
    kiln_c99_nursery_destroy(&receiver.nursery);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-receiver-reservation");
    assert!(
        run.status.success(),
        "graph receiver reservation check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_paused_receiver_probe_clears_its_slot() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int paused_receiver_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap();
    (void)context;
    if (heap == NULL || kiln_c99_graph_paused_control(2, 0, 0, 0) != 3) return 1;
    if (kiln_c99_graph_paused_control(2, 73, 0, 0) != 0) return 2;
    if (heap->realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_EMPTY ||
        heap->realm.graph_inbox_reservation_id != 0 ||
        heap->realm.graph_inbox_root.tag != KILN_VALUE_UNDEFINED) return 3;
    if (kiln_c99_graph_paused_control(2, 73, 0, 0) != 0) return 4;
    if (heap->realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_EMPTY) return 5;
    return 0;
}

int main(void) {
    uint64_t realm = 0;
    int status;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 6;
    status = kiln_realm_enter(realm, paused_receiver_entry, NULL);
    if (status != KILN_C99_OK) return status;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 7;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-paused-receiver-probe");
    assert!(
        run.status.success(),
        "actor paused receiver probe failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_paused_capsule_controls_publish_and_consume() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
typedef struct graph_control_probe {
    uint64_t sender;
    uint64_t receiver;
    uint64_t request;
    uint64_t reservation;
    kiln_value root;
    kiln_gc_heap *sender_heap;
    int stage_status;
} graph_control_probe;

static void graph_control_enter(kiln_gc_heap *heap, uint64_t realm) {
    kiln_test_heap = heap;
    kiln_test_generation++;
    kiln_test_realm_active = 1;
    graph_active_realm = realm;
}

__attribute__((noinline)) static uint64_t graph_control_request(kiln_gc_heap *heap) {
    kiln_c99_par_arena arena;
    kiln_nursery_chunk *chunk;
    kiln_object *handle;
    kiln_collection *registry;
    kiln_string *string;
    kiln_value handle_value;
    uint64_t request = 0;
    handle = (kiln_object *)kiln_c99_gc_alloc(KILN_GC_OBJECT, sizeof *handle);
    registry = (kiln_collection *)kiln_c99_gc_alloc(KILN_GC_COLLECTION, sizeof *registry);
    if (handle == NULL || registry == NULL) return 0;
    memset(handle, 0, sizeof *handle);
    handle->prototype = kiln_c99_undefined();
    memset(registry, 0, sizeof *registry);
    registry->is_weak = 1;
    registry->properties = kiln_c99_undefined();
    registry->entries = (kiln_entry *)calloc(1, sizeof(kiln_entry));
    if (registry->entries == NULL) return 0;
    registry->capacity = registry->count = registry->live = 1;
    if (!kiln_c99_graph_candidate_open(&arena, heap, heap->nursery.capacity)) return 0;
    string = (kiln_string *)kiln_c99_par_alloc(&arena, KILN_GC_STRING, sizeof *string + 4);
    if (string == NULL) return 0;
    string->unit_len = 4;
    string->flags = KILN_STRING_LATIN1;
    string->data = string + 1;
    memcpy((void *)string->data, "root", 4);
    kiln_c99_par_arena_seal(&arena);
    chunk = arena.filled;
    if (chunk == NULL || !kiln_c99_heap_adopt_chunks(heap, chunk)) return 0;
    handle_value = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)handle, NULL, 0);
    registry->entries[0].key = handle_value;
    registry->entries[0].value = kiln_c99_value(KILN_VALUE_STRING,
        (uint64_t)(uintptr_t)string->data, string->data, string->unit_len);
    heap->realm.isolated_graphs = kiln_c99_value(KILN_VALUE_COLLECTION,
        (uint64_t)(uintptr_t)registry, NULL, 0);
    if (!kiln_c99_actor_isolated_request_begin(heap, handle_value, &request)) return 0;
    return request;
}

static int graph_control_make(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    if (heap == NULL) return 1;
    heap->fallback = 0;
    heap->enabled = 1;
    probe->request = graph_control_request(heap);
    return probe->request != 0 ? 0 : 2;
}
static int graph_control_sender(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    if (heap == NULL) return 3;
    if (kiln_c99_graph_paused_control(3, probe->request, probe->reservation,
                                      probe->receiver) != 0) return 4;
    probe->root = heap->realm.isolated_escrow_value;
    if (heap->realm.graph_outgoing_capsule == NULL) return 5;
    return 0;
}
static void *graph_control_sender_lane(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    graph_control_enter(probe->sender_heap, probe->sender);
    probe->stage_status = graph_control_sender(context);
    return NULL;
}
static int graph_control_preflight(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    if (heap == NULL || kiln_c99_graph_paused_control(4, probe->reservation,
                   probe->sender, probe->request) != 0) return 6;
    return heap->realm.graph_inbox_state == KILN_C99_GRAPH_INBOX_RESERVED ? 0 : 7;
}
static int graph_control_receiver_rollback(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    if (kiln_c99_graph_paused_control(8, probe->reservation, 0, 0) != 0 ||
        heap->realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_EMPTY) return 17;
    return 0;
}
static int graph_control_sender_cancel(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_collection *registry = kiln_c99_collection_of(heap->realm.isolated_graphs);
    uint64_t next = 0;
    if (kiln_c99_graph_paused_control(9, probe->request, probe->reservation, 0) != 0 ||
        heap->realm.graph_outgoing_capsule != NULL ||
        heap->realm.isolated_request_id != 0 ||
        registry->entries[0].value.bits != probe->root.bits ||
        !kiln_c99_actor_isolated_request_begin(heap, registry->entries[0].key, &next) ||
        next == probe->request) return 18;
    probe->request = next;
    probe->root = kiln_c99_undefined();
    return 0;
}
static int graph_control_detach(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    return kiln_c99_graph_paused_control(5, probe->reservation, probe->sender,
                                          probe->request) == 0 ? 0 : 8;
}
static int graph_control_adopt(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    if (kiln_c99_graph_paused_control(6, probe->reservation, probe->sender,
                                      probe->request) != 0) return 9;
    if (heap->realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_PUBLISHED ||
        heap->realm.graph_inbox_root.bits != probe->root.bits ||
        kiln_c99_gc_lookup_interior(heap, (uintptr_t)probe->root.bits) == NULL) return 10;
    return 0;
}
static int graph_control_commit(void *context) {
    graph_control_probe *probe = (graph_control_probe *)context;
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_collection *registry = kiln_c99_collection_of(heap->realm.isolated_graphs);
    if (kiln_c99_graph_paused_control(7, probe->request, probe->reservation, 0) != 0) return 11;
    if (heap->realm.graph_outgoing_capsule != NULL ||
        heap->realm.isolated_request_id != 0 || registry->live != 0 ||
        !registry->entries[0].dead) return 12;
    return 0;
}
int main(void) {
    graph_control_probe probe = {0};
    kiln_gc_heap sender, receiver;
    pthread_t sender_lane;
    int status;
    kiln_c99_gc_heap_init(&sender);
    kiln_c99_gc_heap_init(&receiver);
    probe.sender = 1;
    probe.receiver = 2;
    probe.reservation = 83;
    probe.sender_heap = &sender;
    graph_control_enter(&sender, probe.sender);
    status = graph_control_make(&probe);
    /* A turn-boundary control can migrate to a clean owner lane. The factory's C stack on
     * this thread retains dead candidate pointer bytes, which the conservative scan must
     * reject if it runs here. */
    if (status == 0 && pthread_create(&sender_lane, NULL,
                                      graph_control_sender_lane, &probe) != 0) status = 15;
    if (status == 0 && pthread_join(sender_lane, NULL) != 0) status = 16;
    if (status == 0) status = probe.stage_status;
    if (status == 0) {
        graph_control_enter(&receiver, probe.receiver);
        status = graph_control_preflight(&probe);
    }
    if (status == 0) status = graph_control_receiver_rollback(&probe);
    if (status == 0) {
        graph_control_enter(&sender, probe.sender);
        status = graph_control_sender_cancel(&probe);
    }
    if (status == 0 && pthread_create(&sender_lane, NULL,
                                      graph_control_sender_lane, &probe) != 0) status = 19;
    if (status == 0 && pthread_join(sender_lane, NULL) != 0) status = 20;
    if (status == 0) status = probe.stage_status;
    if (status == 0) {
        graph_control_enter(&receiver, probe.receiver);
        status = graph_control_preflight(&probe);
    }
    if (status == 0) {
        graph_control_enter(&sender, probe.sender);
        status = graph_control_detach(&probe);
    }
    if (status == 0) {
        graph_control_enter(&receiver, probe.receiver);
        status = graph_control_adopt(&probe);
    }
    if (status == 0) {
        graph_control_enter(&sender, probe.sender);
        status = graph_control_commit(&probe);
    }
    kiln_test_heap = NULL;
    kiln_test_realm_active = 0;
    kiln_c99_nursery_destroy(&sender.nursery);
    kiln_c99_nursery_destroy(&receiver.nursery);
    return status;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-paused-capsule-controls");
    assert!(
        run.status.success(),
        "actor paused capsule control failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_candidate_scope_contains_ordinary_inline_graph() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int candidate_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_value root, child;
    kiln_object *object;
    kiln_shape *shape;
    kiln_gc_block *block;
    kiln_nursery_chunk *chunk;
    kiln_c99_graph_capsule capsule;
    (void)context;
    if (heap == NULL) return 1;
    /* This fixture provider refuses heap attachment. Use its fallback as a normal collector
     * heap, as the source-level candidate fixture above does. */
    heap->fallback = 0;
    heap->enabled = 1;
    if (!kiln_c99_actor_candidate_begin(heap)) return 9;
    if (kiln_c99_new_object(&root) != KILN_C99_OK ||
        kiln_c99_new_array(&child) != KILN_C99_OK) return 2;
    object = (kiln_object *)(uintptr_t)root.bits;
    if (!kiln_c99_shape_transition(object->shape, kiln_c99_key_empty, 0, &shape)) return 3;
    object->shape = shape;
    object->slots[0] = KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX,
                                         (const void *)(uintptr_t)child.bits);
    if (!kiln_c99_actor_candidate_end(heap, 1)) return 4;
    block = kiln_c99_gc_lookup(heap, object);
    if (block == NULL || (block->kind & KILN_GC_FLAG_NURSERY) == 0) return 5;
    chunk = kiln_c99_nursery_chunk_of(heap, block);
    if (chunk->candidate_id == 0 || chunk->alloc_count != 2 ||
        !kiln_c99_graph_capsule_prepare(heap, chunk, root, &capsule)) return 6;
    return 0;
}
int main(void) {
    uint64_t realm = 0;
    int status;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 7;
    status = kiln_realm_enter(realm, candidate_entry, NULL);
    if (kiln_realm_destroy(realm) != KILN_C99_OK) return 8;
    return status;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-inline-candidate-scope");
    assert!(
        run.status.success(),
        "actor inline candidate scope failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_capsule_moves_two_closed_chunks() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap sender, receiver;
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *first, *second;
    kiln_c99_graph_capsule capsule;
    kiln_val *root_cell, *child_cell;
    kiln_value root;
    uint64_t reservation = 94;
    size_t filler = 0;
    kiln_c99_gc_heap_init(&sender);
    kiln_c99_gc_heap_init(&receiver);
    if (!kiln_c99_graph_candidate_open(&candidate, &sender,
                                        sender.nursery.capacity * 3)) return 1;
    root_cell = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *root_cell);
    if (root_cell == NULL) return 2;
    first = candidate.current;
    while (candidate.current == first && filler++ < 10000) {
        kiln_array *orphan = (kiln_array *)kiln_c99_par_alloc(
            &candidate, KILN_GC_ARRAY, sizeof(kiln_array));
        if (orphan == NULL) return 3;
        memset(orphan, 0, sizeof *orphan);
        orphan->properties = kiln_c99_undefined();
        orphan->match_input = kiln_c99_undefined();
        orphan->match_groups = kiln_c99_undefined();
        orphan->match_index = -1;
    }
    second = candidate.current;
    if (second == NULL || second == first) return 4;
    child_cell = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *child_cell);
    if (child_cell == NULL || candidate.current != second) return 5;
    *root_cell = KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, child_cell);
    *child_cell = KILN_VAL_UNDEFINED;
    kiln_c99_par_arena_seal(&candidate);
    if (!kiln_c99_heap_adopt_chunks(&sender, candidate.filled)) return 6;
    root = kiln_c99_value(KILN_VALUE_CELL, (uint64_t)(uintptr_t)root_cell, NULL, 0);
    if (!kiln_c99_graph_capsule_prepare(&sender, first, root, &capsule) ||
        capsule.needs.regions != 2 || capsule.chunks == NULL) return 7;
    if (!kiln_c99_graph_capsule_receiver_preflight(&receiver, &capsule.needs,
                                                    reservation)) return 8;
    if (!kiln_c99_graph_capsule_detach(&sender, &capsule) ||
        sender.count != 0 || sender.nursery.full != NULL ||
        kiln_c99_gc_lookup(&sender, root_cell) != NULL ||
        kiln_c99_gc_lookup(&receiver, root_cell) != NULL) return 9;
    kiln_c99_graph_capsule_adopt(&receiver, &capsule, reservation);
    if (receiver.count != capsule.needs.objects || capsule.chunk != NULL ||
        kiln_c99_gc_lookup(&receiver, root_cell) == NULL ||
        kiln_c99_gc_lookup(&receiver, child_cell) == NULL ||
        receiver.realm.graph_inbox_root.bits != root.bits) return 10;
    kiln_c99_gc_mark_realm(&receiver);
    if (((kiln_gc_block *)root_cell - 1)->mark != receiver.epoch ||
        ((kiln_gc_block *)child_cell - 1)->mark != receiver.epoch) return 11;
    kiln_c99_graph_capsule_dispose(&capsule);
    kiln_c99_nursery_destroy(&sender.nursery);
    kiln_c99_nursery_destroy(&receiver.nursery);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-two-chunks");
    assert!(
        run.status.success(),
        "graph two-chunk capsule failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_capsule_moves_inline_object_and_array() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap sender, receiver;
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *chunk;
    kiln_c99_graph_capsule capsule;
    kiln_shape shape;
    kiln_object *object;
    kiln_array *array;
    kiln_string *text;
    kiln_val external[12];
    kiln_value root;
    size_t bit;
    uint64_t reservation = 91;
    kiln_c99_gc_heap_init(&sender);
    kiln_c99_gc_heap_init(&receiver);
    if (!kiln_c99_graph_candidate_open(&candidate, &sender,
                                        sender.nursery.capacity)) return 1;
    object = (kiln_object *)kiln_c99_par_alloc(&candidate, KILN_GC_OBJECT, sizeof *object);
    array = (kiln_array *)kiln_c99_par_alloc(&candidate, KILN_GC_ARRAY,
                                             sizeof *array + 12 * sizeof(kiln_val));
    text = (kiln_string *)kiln_c99_par_alloc(&candidate, KILN_GC_STRING,
                                              sizeof *text + 3);
    if (object == NULL || array == NULL || text == NULL) return 2;
    memset(&shape, 0, sizeof shape);
    shape.shared = 1;
    shape.property_count = 1;
    shape.keys[0] = kiln_c99_key_empty;
    memset(object, 0, sizeof *object);
    object->prototype = kiln_c99_undefined();
    object->shape = &shape;
    object->slots = object->inline_slots;
    object->slot_capacity = KILN_C99_INLINE_SLOTS;
    for (size_t i = 0; i < KILN_C99_INLINE_SLOTS; ++i)
        object->slots[i] = KILN_VAL_UNDEFINED;
    object->slots[0] = KILN_VAL_TAG_PTR(KILN_TAG_BUFFER_PREFIX, array);
    memset(array, 0, sizeof *array);
    array->elements = array->inline_elements;
    array->count = 1;
    array->capacity = 8;
    array->properties = kiln_c99_undefined();
    array->match_index = -1;
    array->match_input = kiln_c99_undefined();
    array->match_groups = kiln_c99_undefined();
    array->inline_elements[0] = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, text);
    text->unit_len = 3;
    text->flags = KILN_STRING_LATIN1;
    text->data = text + 1;
    memcpy((void *)text->data, "yes", 3);
    kiln_c99_par_arena_seal(&candidate);
    chunk = candidate.filled;
    if (chunk == NULL || !kiln_c99_heap_adopt_chunks(&sender, chunk)) return 3;
    root = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)object, NULL, 0);
    if (!kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 4;
    external[0] = object->slots[0];
    object->slots = external;
    if (kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 5;
    object->slots = object->inline_slots;
    array->properties = root;
    if (kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 6;
    array->properties = kiln_c99_undefined();
    shape.shared = 0;
    if (kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 7;
    shape.shared = 1;
    bit = (size_t)((char *)((kiln_gc_block *)object - 1) - chunk->base) >> 4;
    chunk->finals[bit >> 6] &= ~(UINT64_C(1) << (bit & 63));
    if (kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 8;
    chunk->finals[bit >> 6] |= UINT64_C(1) << (bit & 63);
    array->elements = (kiln_val *)(array + 1);
    array->count = 12;
    array->capacity = 12;
    array->elements[0] = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, text);
    for (size_t i = 1; i < array->count; ++i)
        array->elements[i] = KILN_VAL_UNDEFINED;
    if (!kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 9;
    array->elements = external;
    if (kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 14;
    array->elements = (kiln_val *)(array + 1);
    array->capacity = 13;
    if (kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 15;
    array->capacity = 12;
    if (!kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 16;
    if (!kiln_c99_graph_capsule_receiver_preflight(&receiver, &capsule.needs,
                                                    reservation)) return 10;
    if (!kiln_c99_graph_capsule_detach(&sender, &capsule)) return 11;
    kiln_c99_graph_capsule_adopt(&receiver, &capsule, reservation);
    if (sender.count != 0 || receiver.count != 3 ||
        receiver.realm.graph_inbox_root.bits != root.bits ||
        kiln_c99_gc_lookup(&receiver, object) == NULL ||
        array->elements != (kiln_val *)(array + 1) ||
        array->count != 12 || array->elements[11] != KILN_VAL_UNDEFINED ||
        kiln_c99_gc_lookup(&sender, object) != NULL) return 12;
    kiln_c99_gc_mark_realm(&receiver);
    if (((kiln_gc_block *)object - 1)->mark != receiver.epoch ||
        ((kiln_gc_block *)array - 1)->mark != receiver.epoch ||
        ((kiln_gc_block *)text - 1)->mark != receiver.epoch) return 13;
    kiln_c99_nursery_destroy(&sender.nursery);
    kiln_c99_nursery_destroy(&receiver.nursery);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-inline-object-array");
    assert!(
        run.status.success(),
        "graph inline object/array capsule failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_capsule_detaches_before_receiver_adoption() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap sender, receiver;
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *chunk;
    kiln_c99_graph_capsule capsule;
    kiln_c99_graph_capsule_requirements wrong;
    kiln_val *cell;
    kiln_collection registry;
    kiln_entry entry;
    kiln_object handle;
    kiln_value root, handle_value;
    uint64_t request = 0;
    uint64_t reservation = 83;
    kiln_c99_gc_heap_init(&sender);
    kiln_c99_gc_heap_init(&receiver);
    if (!kiln_c99_graph_candidate_open(&candidate, &sender,
                                        sender.nursery.capacity)) return 1;
    cell = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *cell);
    if (cell == NULL) return 2;
    *cell = KILN_VAL_UNDEFINED;
    kiln_c99_par_arena_seal(&candidate);
    chunk = candidate.filled;
    if (chunk == NULL || !kiln_c99_heap_adopt_chunks(&sender, chunk)) return 3;
    root = kiln_c99_value(KILN_VALUE_CELL, (uint64_t)(uintptr_t)cell, NULL, 0);
    if (!kiln_c99_graph_capsule_prepare(&sender, chunk, root, &capsule)) return 4;
    memset(&registry, 0, sizeof registry);
    memset(&entry, 0, sizeof entry);
    memset(&handle, 0, sizeof handle);
    handle_value = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)&handle, NULL, 0);
    entry.key = handle_value;
    entry.value = root;
    registry.entries = &entry;
    registry.capacity = registry.count = registry.live = 1;
    registry.is_weak = 1;
    sender.realm.isolated_graphs = kiln_c99_value(KILN_VALUE_COLLECTION,
        (uint64_t)(uintptr_t)&registry, NULL, 0);
    if (!kiln_c99_actor_isolated_request_begin(&sender, handle_value, &request) ||
        !kiln_c99_actor_isolated_request_escrow(&sender, request) ||
        entry.value.tag != KILN_VALUE_UNDEFINED) return 12;
    wrong = capsule.needs;
    wrong.region_capacity++;
    if (kiln_c99_graph_capsule_receiver_preflight(&receiver, &wrong, reservation) ||
        receiver.realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_EMPTY) return 5;
    if (!kiln_c99_graph_capsule_receiver_preflight(&receiver, &capsule.needs,
                                                    reservation)) return 6;
    if (kiln_c99_graph_capsule_receiver_preflight(&receiver, &capsule.needs,
                                                   reservation + 1)) return 7;
    if (!kiln_c99_graph_capsule_detach(&sender, &capsule)) return 8;
    if (sender.count != 0 || sender.bytes != 0 || sender.nursery.full != NULL ||
        kiln_c99_gc_lookup(&sender, cell) != NULL ||
        kiln_c99_gc_lookup(&receiver, cell) != NULL) return 9;
    kiln_c99_graph_capsule_adopt(&receiver, &capsule, reservation);
    if (capsule.chunk != NULL || capsule.detached || !capsule.adopted || receiver.count != 1 ||
        kiln_c99_gc_lookup(&receiver, cell) == NULL ||
        receiver.realm.graph_inbox_state != KILN_C99_GRAPH_INBOX_PUBLISHED ||
        receiver.realm.graph_inbox_root.bits != root.bits) return 10;
    kiln_c99_graph_capsule_commit_sender(&sender, &capsule, request);
    if (!entry.dead || registry.live != 0 || entry.key.tag != KILN_VALUE_UNDEFINED ||
        entry.value.tag != KILN_VALUE_UNDEFINED ||
        sender.realm.isolated_escrow_entry != NULL ||
        sender.realm.isolated_escrow_value.tag != KILN_VALUE_UNDEFINED ||
        sender.realm.isolated_request_id != 0) return 13;
    kiln_c99_gc_mark_realm(&receiver);
    if (((kiln_gc_block *)cell - 1)->mark != receiver.epoch) return 11;
    kiln_c99_nursery_destroy(&sender.nursery);
    kiln_c99_nursery_destroy(&receiver.nursery);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-detached-capsule");
    assert!(
        run.status.success(),
        "graph detached capsule failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_receiver_accepts_an_inline_string_data_pointer() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap sender, receiver;
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *batch[1], *chunk;
    kiln_string *string;
    kiln_value root, wrong;
    uint64_t id = 0;
    kiln_c99_gc_heap_init(&sender);
    kiln_c99_gc_heap_init(&receiver);
    if (!kiln_c99_graph_candidate_open(&candidate, &sender,
                                        sender.nursery.capacity)) return 1;
    string = (kiln_string *)kiln_c99_par_alloc(&candidate, KILN_GC_STRING,
                                               sizeof *string + 4);
    if (string == NULL) return 2;
    string->unit_len = 4;
    string->flags = KILN_STRING_LATIN1;
    string->data = string + 1;
    memcpy((void *)string->data, "root", 4);
    kiln_c99_par_arena_seal(&candidate);
    chunk = candidate.filled;
    if (chunk == NULL || !kiln_c99_heap_adopt_chunks(&sender, chunk)) return 3;
    batch[0] = chunk;
    root = kiln_c99_value(KILN_VALUE_STRING, 0, string->data, string->unit_len);
    if (!kiln_c99_graph_receiver_reserve(&receiver, 1, &id)) return 4;
    wrong = root;
    wrong.len--;
    if (kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, wrong, id))
        return 5;
    wrong = root;
    wrong.bits = (uint64_t)(uintptr_t)string;
    if (kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, wrong, id))
        return 6;
    if (!kiln_c99_graph_receiver_move_and_publish(&sender, &receiver, batch, 1, root, id))
        return 7;
    if (sender.count != 0 || receiver.count != 1 ||
        receiver.realm.graph_inbox_root.bits != root.bits ||
        memcmp(receiver.realm.graph_inbox_root.ptr, "root", 4) != 0) return 8;
    kiln_c99_gc_mark_realm(&receiver);
    if (((kiln_gc_block *)string - 1)->mark != receiver.epoch) return 9;
    kiln_c99_nursery_destroy(&sender.nursery);
    kiln_c99_nursery_destroy(&receiver.nursery);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-receiver-inline-string");
    assert!(
        run.status.success(),
        "graph receiver string root check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_candidate_closure_rejects_external_and_unreachable_edges() {
    let program = program(vec![], vec![Instruction::Return], 0);
    let generated = emit(&program).expect("program emits");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap heap;
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *first, *second, *batch[2];
    kiln_val *root, *next;
    kiln_string *text;
    kiln_sliced_string *slice;
    kiln_object *outside;
    kiln_shape shared_shape;
    const void *saved_data;
    kiln_val saved_edge;
    uintptr_t stack_words[2];
    uint32_t root_mark;
    uint32_t saved_kind;
    kiln_c99_gc_heap_init(&heap);
    if (!kiln_c99_graph_candidate_open(&candidate, &heap, 2 * heap.nursery.capacity)) return 1;
    root = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *root);
    if (root == NULL) return 2;
    first = candidate.current;
    kiln_c99_par_arena_seal(&candidate);
    next = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *next);
    text = (kiln_string *)kiln_c99_par_alloc(&candidate, KILN_GC_STRING,
                                              sizeof *text + 4);
    slice = (kiln_sliced_string *)kiln_c99_par_alloc(&candidate, KILN_GC_SLICED_STRING,
                                                      sizeof *slice);
    if (next == NULL || text == NULL || slice == NULL) return 3;
    second = candidate.current;
    text->unit_len = 4;
    text->flags = KILN_STRING_LATIN1;
    text->data = text + 1;
    memcpy((void *)text->data, "abcd", 4);
    slice->base.unit_len = 2;
    slice->base.flags = KILN_STRING_SLICED;
    slice->base.data = (const char *)text->data + 1;
    slice->parent = text;
    *next = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &slice->base);
    *root = KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, next);
    kiln_c99_par_arena_seal(&candidate);
    second->next = first;
    if (!kiln_c99_heap_adopt_chunks(&heap, second)) return 4;
    batch[0] = first;
    batch[1] = second;
    if (!kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 5;
    if (kiln_c99_graph_candidate_closed(&heap, batch, 1, root)) return 6;
    saved_edge = *next;
    *next = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
    if (kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 7;
    *next = saved_edge;
    saved_data = text->data;
    text->data = &kiln_c99_empty_string;
    if (kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 8;
    text->data = saved_data;
    slice->base.data = (const char *)text->data + 3;
    if (kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 9;
    slice->base.data = (const char *)text->data + 1;
    saved_kind = ((kiln_gc_block *)next - 1)->kind;
    ((kiln_gc_block *)next - 1)->kind = KILN_GC_PROXY | KILN_GC_FLAG_NURSERY;
    if (kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 10;
    ((kiln_gc_block *)next - 1)->kind = saved_kind;
    if (kiln_c99_graph_candidate_closed(&heap, batch, 2, next)) return 11;
    if (!kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 12;
    saved_edge = *root;
    *root = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, next);
    if (kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 13;
    *root = saved_edge;
    if (kiln_c99_graph_candidate_closed(&heap, batch, 2, (char *)root + 1)) return 14;
    if (!kiln_c99_graph_candidate_closed(&heap, batch, 2, root)) return 15;
    root_mark = ((kiln_gc_block *)root - 1)->mark;
    stack_words[0] = 0;
    stack_words[1] = (uintptr_t)&kiln_c99_empty_string;
    if (kiln_c99_graph_machine_words_alias(&heap, batch, 2, stack_words, 2)) return 39;
    stack_words[1] = (uintptr_t)root;
    if (!kiln_c99_graph_machine_words_alias(&heap, batch, 2, stack_words, 2)) return 40;
    stack_words[1] = (uintptr_t)KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &slice->base);
    if (!kiln_c99_graph_machine_words_alias(&heap, batch, 2, stack_words, 2)) return 41;
    stack_words[1] = (uintptr_t)((const char *)text->data + 1);
    if (!kiln_c99_graph_machine_words_alias(&heap, batch, 2, stack_words, 2)) return 42;
    if (((kiln_gc_block *)root - 1)->mark != root_mark) return 43;
    /* This standalone heap has no active provider realm. The provider visitor must report
     * incomplete coverage, rather than allowing a positive graph certificate. */
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED) return 16;
    heap.realm.array_unscopables.tag = KILN_VALUE_STRING;
    heap.realm.array_unscopables.bits = (uint64_t)(uintptr_t)text;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_C_ROOT_ALIAS) return 17;
    heap.realm.array_unscopables = kiln_c99_undefined();
    heap.realm.native_prototypes[0].tag = KILN_VALUE_STRING;
    heap.realm.native_prototypes[0].bits = (uint64_t)(uintptr_t)((const char *)text->data + 1);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_C_ROOT_ALIAS) return 18;
    heap.realm.native_prototypes[0] = kiln_c99_undefined();
    heap.hot_keys[0] = (const kiln_string *)((const char *)text->data + 1);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_C_ROOT_ALIAS) return 19;
    heap.hot_keys[0] = NULL;
    heap.jobs = (kiln_microtask *)calloc(1, sizeof *heap.jobs);
    if (heap.jobs == NULL) return 20;
    heap.job_head = 0;
    heap.job_tail = 1;
    heap.job_mask = 0;
    heap.jobs[0].argument.tag = KILN_VALUE_STRING;
    heap.jobs[0].argument.bits = (uint64_t)(uintptr_t)text;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_C_ROOT_ALIAS) return 21;
    heap.job_tail = 0;
    free(heap.jobs);
    heap.jobs = NULL;
    heap.keys.entries = (kiln_key_entry *)calloc(1, sizeof *heap.keys.entries);
    if (heap.keys.entries == NULL) return 24;
    heap.keys.capacity = 1;
    heap.keys.entries[0].string = text;
    heap.keys.entries[0].pinned = 0;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_C_ROOT_ALIAS) return 25;
    free(heap.keys.entries);
    heap.keys.entries = NULL;
    heap.keys.capacity = 0;
    *next = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &kiln_c99_empty_string);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTGOING_UNPROVEN) return 22;
    *next = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, &slice->base);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED) return 23;
    outside = (kiln_object *)kiln_c99_gc_nursery_bump(&heap, KILN_GC_OBJECT, sizeof *outside);
    if (outside == NULL) return 26;
    memset(outside, 0, sizeof *outside);
    outside->properties = (kiln_property *)calloc(1, sizeof *outside->properties);
    if (outside->properties == NULL) return 27;
    outside->capacity = outside->count = 1;
    outside->prototype = kiln_c99_undefined();
    outside->properties[0].key = &kiln_c99_empty_string;
    outside->properties[0].value.tag = KILN_VALUE_STRING;
    outside->properties[0].value.bits = (uint64_t)(uintptr_t)text;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 28;
    outside->properties[0].value = kiln_c99_undefined();
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED) return 29;
    kiln_collection *outside_collection = (kiln_collection *)kiln_c99_gc_nursery_bump(
        &heap, KILN_GC_COLLECTION, sizeof *outside_collection);
    if (outside_collection == NULL) return 44;
    memset(outside_collection, 0, sizeof *outside_collection);
    outside_collection->entries = (kiln_entry *)calloc(1, sizeof(kiln_entry));
    if (outside_collection->entries == NULL) return 45;
    outside_collection->capacity = outside_collection->count = 1;
    outside_collection->is_weak = 1;
    outside_collection->entries[0].dead = 1;
    outside_collection->entries[0].key = kiln_c99_value(KILN_VALUE_STRING,
        (uint64_t)(uintptr_t)text, text->data, text->unit_len);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 46;
    outside_collection->entries[0].key = kiln_c99_undefined();
    outside_collection->entries[0].value = kiln_c99_value(KILN_VALUE_STRING,
        (uint64_t)(uintptr_t)text, text->data, text->unit_len);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 47;
    outside_collection->entries[0].value = kiln_c99_undefined();
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED) return 48;
    outside_collection->count = 2;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED) return 49;
    outside_collection->count = 1;
    memset(&shared_shape, 0, sizeof shared_shape);
    shared_shape.shared = 1 | KILN_C99_SHAPE_FINAL;
    shared_shape.property_count = 1;
    shared_shape.keys[0] = &kiln_c99_empty_string;
    outside->shape = &shared_shape;
    outside->slots = outside->inline_slots;
    outside->slot_capacity = KILN_C99_INLINE_SLOTS;
    outside->slots[0] = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, text);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 50;
    outside->slots[0] = KILN_VAL_UNDEFINED;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED) return 51;
    shared_shape.keys[0] = text;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 52;
    shared_shape.keys[0] = &kiln_c99_empty_string;
    shared_shape.property_count = KILN_C99_INLINE_SLOTS + 1;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED) return 53;
    shared_shape.property_count = 1;
    shared_shape.shared = 0;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_KIND_UNENUMERATED) return 30;
    outside->shape = NULL;
    outside->slots = NULL;
    outside->slot_capacity = 0;
    outside->properties[0].value.tag = KILN_VALUE_STRING;
    outside->properties[0].value.bits = (uint64_t)(uintptr_t)text;
    kiln_nursery_chunk *outside_chunk = kiln_c99_nursery_seal(&heap.nursery);
    outside_chunk->next = heap.nursery.full;
    heap.nursery.full = outside_chunk;
    heap.nursery.region = NULL;
    heap.nursery.base = heap.nursery.bump = heap.nursery.limit = NULL;
    heap.nursery.active = 0;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 31;
    heap.nursery.full = outside_chunk->next;
    outside_chunk->next = heap.nursery.promoted;
    heap.nursery.promoted = outside_chunk;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 32;
    outside->properties[0].value = kiln_c99_undefined();
    kiln_gc_slab *slab = kiln_c99_gc_slab_new(&heap, 0);
    if (slab == NULL) return 33;
    kiln_gc_block *slot = kiln_c99_gc_slab_take(&heap, slab);
    slot->kind = KILN_GC_CELL;
    slot->size = sizeof(kiln_val);
    *(kiln_val *)(slot + 1) = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, text);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 34;
    *(kiln_val *)(slot + 1) = KILN_VAL_UNDEFINED;
    kiln_gc_block *large = (kiln_gc_block *)calloc(1, sizeof *large + sizeof(kiln_val));
    if (large == NULL) return 35;
    large->kind = KILN_GC_CELL | KILN_GC_FLAG_LARGE;
    large->size = sizeof(kiln_val);
    heap.head = large;
    kiln_c99_gc_bound(&heap, large, sizeof *large + sizeof(kiln_val));
    if (kiln_c99_gc_ptrset_insert(&heap.large_table, large) != KILN_C99_OK) return 36;
    *(kiln_val *)(large + 1) = KILN_VAL_TAG_PTR(KILN_TAG_STRING_PREFIX, text);
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 37;
    *(kiln_val *)(large + 1) = KILN_VAL_UNDEFINED;
    if (kiln_c99_graph_incoming_probe(&heap, batch, 2, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED) return 38;
    kiln_c99_gc_ptrset_remove(&heap.large_table, large);
    free(heap.large_table.table);
    free(large);
    kiln_c99_gc_ptrset_remove(&heap.slab_table, slab);
    free(heap.slab_table.table);
    kiln_c99_gc_aligned_free(slab);
    free(outside->properties);
    free(outside_collection->entries);
    kiln_c99_nursery_destroy(&heap.nursery);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-candidate-closure");
    assert!(
        run.status.success(),
        "graph candidate closure check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_factory_allocations_have_dedicated_candidate_chunks() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int actor_candidate_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_value args[2], result, thrown;
    kiln_val *old, *root, *aborted, *side_elements;
    kiln_array *orphan;
    kiln_value orphan_value;
    kiln_nursery_chunk *candidate, *batch[1], *discarded;
    uint32_t epoch;
    (void)context;
    if (heap == NULL) return 1;
    /* The fixture provider refuses heap attachment and supplies a fallback heap. Exercise
     * the normal collector path explicitly within this one-process fixture. */
    heap->fallback = 0;
    heap->enabled = 1;
    old = (kiln_val *)kiln_c99_gc_alloc(KILN_GC_CELL, sizeof *old);
    if (old == NULL) return 2;
    *old = KILN_VAL_UNDEFINED;
    args[0] = kiln_c99_number(28);
    if (kiln_c99_worker_op_raw(&result, args, 1, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result)) return 3;
    if (kiln_c99_actor_candidate_begin(heap)) return 4; /* nested scopes do not claim chunks */
    root = (kiln_val *)kiln_c99_gc_alloc(KILN_GC_CELL, sizeof *root);
    if (root == NULL) return 5;
    *root = KILN_VAL_UNDEFINED;
    if (kiln_c99_new_array(&orphan_value) != KILN_C99_OK) return 16;
    orphan = (kiln_array *)(uintptr_t)orphan_value.bits;
    orphan->elements = orphan->inline_elements;
    orphan->capacity = 8;
    orphan->count = 2;
    orphan->elements[0] = kiln_val_from_boundary(kiln_c99_number(29));
    orphan->elements[1] = KILN_VAL_TRUE;
    candidate = kiln_c99_nursery_chunk_of(heap, (kiln_gc_block *)root - 1);
    if (candidate == kiln_c99_nursery_chunk_of(heap, (kiln_gc_block *)old - 1) ||
        candidate->candidate_id == 0 || candidate->candidate_id != heap->actor_candidate_id)
        return 6;
    epoch = heap->epoch;
    kiln_c99_gc_collect(heap, 1); /* collection is deferred until the candidate is sealed */
    if (heap->epoch != epoch) return 7;
    args[0] = kiln_c99_number(29);
    args[1] = kiln_c99_boolean(1);
    if (kiln_c99_worker_op_raw(&result, args, 2, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result)) return 8;
    if (candidate->candidate_epoch != heap->epoch || heap->actor_candidate_id != 0 ||
        ((kiln_nursery_chunk *)heap->nursery.region)->candidate_id != 0) return 9;
    batch[0] = candidate;
    if (!kiln_c99_graph_candidate_closed(heap, batch, 1, root)) return 10;
    /* The orphan may move as garbage only while its layout has no pointer or side allocation. */
    orphan->elements[1] = KILN_VAL_TAG_PTR(KILN_TAG_CELL_PREFIX, old);
    if (kiln_c99_graph_candidate_closed(heap, batch, 1, root)) return 17;
    orphan->elements[1] = KILN_VAL_TRUE;
    side_elements = (kiln_val *)malloc(2 * sizeof *side_elements);
    if (side_elements == NULL) return 18;
    side_elements[0] = orphan->elements[0];
    side_elements[1] = orphan->elements[1];
    orphan->elements = side_elements;
    if (kiln_c99_graph_candidate_closed(heap, batch, 1, root)) return 19;
    orphan->elements = orphan->inline_elements;
    free(side_elements);
    if (!kiln_c99_graph_candidate_closed(heap, batch, 1, root)) return 20;
    heap->realm.graph_inbox_root = kiln_c99_value(
        KILN_VALUE_CELL, (uint64_t)(uintptr_t)root, NULL, 0);
    kiln_c99_gc_collect(heap, 1); /* a collection in the registration window preserves it */
    if (candidate->candidate_state != 1 || candidate->candidate_epoch != heap->epoch ||
        !kiln_c99_graph_candidate_closed(heap, batch, 1, root)) return 21;
    kiln_c99_actor_candidate_finish(heap, 1);
    if (candidate->candidate_state != 2 || heap->actor_candidate_pending_id != 0) return 22;
    heap->realm.graph_inbox_root = kiln_c99_undefined();
    args[0] = kiln_c99_number(28);
    if (kiln_c99_worker_op_raw(&result, args, 1, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result)) return 11;
    aborted = (kiln_val *)kiln_c99_gc_alloc(KILN_GC_CELL, sizeof *aborted);
    if (aborted == NULL) return 12;
    *aborted = KILN_VAL_UNDEFINED;
    discarded = kiln_c99_nursery_chunk_of(heap, (kiln_gc_block *)aborted - 1);
    args[0] = kiln_c99_number(29);
    args[1] = kiln_c99_boolean(0);
    if (kiln_c99_worker_op_raw(&result, args, 2, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result) || discarded->candidate_id != 0) return 13;
    return 0;
}
int main(void) {
    uint64_t realm;
    int result;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 14;
    result = kiln_realm_enter(realm, actor_candidate_entry, NULL);
    if (result != KILN_C99_OK) return result;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 15;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-factory-candidates");
    assert!(
        run.status.success(),
        "actor factory candidate check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_candidate_stress_collects_and_discards_provenance() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int actor_candidate_stress_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_nursery_chunk *chunk;
    uint32_t before;
    (void)context;
    if (heap == NULL) return 1;
    heap->fallback = 0;
    heap->enabled = 1;
    /* Arm stress after ordinary realm bootstrap so this fixture isolates the candidate's
     * allocation cadence rather than testing bootstrap's separate root lifetime. */
    heap->stress = 1;
    heap->stress_every = 1;
    heap->stress_countdown = 1;
    if (!kiln_c99_actor_candidate_begin(heap)) return 2;
    before = heap->epoch;
    if (kiln_c99_gc_alloc(KILN_GC_CELL, sizeof(kiln_val)) == NULL) return 3;
    if (heap->epoch <= before || !heap->actor_candidate_collected) return 4;
    if (!kiln_c99_actor_candidate_end(heap, 1)) return 5;
    if (heap->actor_candidate_id != 0) return 6;
    for (chunk = heap->nursery.full; chunk != NULL; chunk = chunk->next)
        if (chunk->candidate_id != 0) return 7;
    for (chunk = heap->nursery.promoted; chunk != NULL; chunk = chunk->next)
        if (chunk->candidate_id != 0) return 8;
    if (heap->nursery.region != NULL &&
        ((kiln_nursery_chunk *)heap->nursery.region)->candidate_id != 0) return 9;
    return 0;
}
int main(void) {
    uint64_t realm;
    int result;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 11;
    result = kiln_realm_enter(realm, actor_candidate_stress_entry, NULL);
    if (result != KILN_C99_OK) return result;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 12;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-candidate-stress");
    assert!(
        run.status.success(),
        "actor candidate stress check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_graph_capsule_certificate_retains_only_escrow() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
__attribute__((noinline)) static uint64_t make_closed_request(kiln_gc_heap *heap) {
    kiln_c99_par_arena arena;
    kiln_nursery_chunk *chunk;
    kiln_object *handle;
    kiln_collection *registry;
    kiln_string *string;
    kiln_value handle_value;
    uint64_t request = 0;
    handle = (kiln_object *)kiln_c99_gc_alloc(KILN_GC_OBJECT, sizeof *handle);
    registry = (kiln_collection *)kiln_c99_gc_alloc(KILN_GC_COLLECTION, sizeof *registry);
    if (handle == NULL || registry == NULL) return 0;
    memset(handle, 0, sizeof *handle);
    handle->prototype = kiln_c99_undefined();
    memset(registry, 0, sizeof *registry);
    registry->is_weak = 1;
    registry->properties = kiln_c99_undefined();
    registry->entries = (kiln_entry *)calloc(1, sizeof(kiln_entry));
    if (registry->entries == NULL) return 0;
    registry->capacity = registry->count = registry->live = 1;
    if (!kiln_c99_graph_candidate_open(&arena, heap, heap->nursery.capacity)) return 0;
    string = (kiln_string *)kiln_c99_par_alloc(&arena, KILN_GC_STRING, sizeof *string + 4);
    if (string == NULL) return 0;
    string->unit_len = 4;
    string->flags = KILN_STRING_LATIN1;
    string->data = string + 1;
    memcpy((void *)string->data, "root", 4);
    kiln_c99_par_arena_seal(&arena);
    chunk = arena.filled;
    if (chunk == NULL || !kiln_c99_heap_adopt_chunks(heap, chunk)) return 0;
    handle_value = kiln_c99_value(KILN_VALUE_OBJECT, (uint64_t)(uintptr_t)handle, NULL, 0);
    registry->entries[0].key = handle_value;
    registry->entries[0].value = kiln_c99_value(KILN_VALUE_STRING,
        (uint64_t)(uintptr_t)string->data, string->data, string->unit_len);
    heap->realm.isolated_graphs = kiln_c99_value(KILN_VALUE_COLLECTION,
        (uint64_t)(uintptr_t)registry, NULL, 0);
    if (!kiln_c99_actor_isolated_request_begin(heap, handle_value, &request)) return 0;
    return request;
}

static int actor_certificate_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_c99_graph_capsule capsule;
    kiln_collection *registry;
    uint64_t request;
    (void)context;
    if (heap == NULL) return 1;
    heap->fallback = 0;
    heap->enabled = 1;
    request = make_closed_request(heap);
    if (request == 0) return 2;
    if (!kiln_c99_graph_capsule_certify_request(heap, request, &capsule)) return 3;
    if (capsule.chunk == NULL || capsule.detached ||
        heap->realm.isolated_escrow_entry == NULL ||
        heap->realm.isolated_escrow_value.bits != capsule.root.bits) return 4;
    registry = kiln_c99_collection_of(heap->realm.isolated_graphs);
    if (registry->entries[0].value.tag != KILN_VALUE_UNDEFINED) return 5;
    if (!kiln_c99_actor_isolated_request_rollback(heap, request)) return 6;
    if (registry->entries[0].value.bits != capsule.root.bits ||
        heap->realm.isolated_escrow_entry != NULL ||
        heap->realm.isolated_request_id != 0) return 7;
    return 0;
}

int main(void) {
    uint64_t realm;
    int result;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 8;
    result = kiln_realm_enter(realm, actor_certificate_entry, NULL);
    if (result != KILN_C99_OK) return result;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 9;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-graph-capsule-certificate");
    assert!(
        run.status.success(),
        "actor graph certificate failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_request_machine_boundary_sees_older_frame_alias() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int actor_boundary_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_value handle, graph, args[3], result, thrown;
    kiln_string *text;
    kiln_collection *registry;
    volatile uintptr_t older_frame_alias;
    uint64_t id = 0;
    (void)context;
    if (heap == NULL || kiln_c99_new_object(&handle) != KILN_C99_OK) return 1;
    heap->fallback = 0;
    heap->enabled = 1;
    if (!kiln_c99_actor_candidate_begin(heap)) return 2;
    text = (kiln_string *)kiln_c99_gc_alloc(KILN_GC_STRING, sizeof *text + 4);
    if (text == NULL) return 3;
    text->unit_len = 4;
    text->flags = KILN_STRING_LATIN1;
    text->data = text + 1;
    memcpy((void *)text->data, "root", 4);
    if (!kiln_c99_actor_candidate_end(heap, 1)) return 4;
    {
        kiln_nursery_chunk *batch[1] = {
            kiln_c99_nursery_chunk_of(heap, (kiln_gc_block *)text - 1)
        };
        if (!kiln_c99_graph_candidate_closed(heap, batch, 1, text)) return 11;
    }
    graph = kiln_c99_value(KILN_VALUE_STRING, (uint64_t)(uintptr_t)text,
                           text->data, text->unit_len);
    args[0] = kiln_c99_number(26);
    args[1] = handle;
    args[2] = graph;
    if (kiln_c99_worker_op_raw(&result, args, 3, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result) ||
        !kiln_c99_actor_isolated_request_begin(heap, handle, &id)) return 5;
    {
        kiln_nursery_chunk *batch[1] = {
            kiln_c99_nursery_chunk_of(heap, (kiln_gc_block *)text - 1)
        };
        if (!kiln_c99_graph_candidate_closed(heap, batch, 1, text)) return 12;
    }
    registry = kiln_c99_collection_of(heap->realm.isolated_graphs);
    {
        double at = kiln_c99_collection_find(registry, handle);
        if (at < 0.0 || registry->entries[(size_t)at].value.bits != graph.bits) return 13;
    }
    older_frame_alias = (uintptr_t)text;
    {
        int probe = kiln_c99_graph_incoming_request_probe(heap, id);
        if (probe != KILN_C99_GRAPH_MACHINE_ALIAS) {
            fprintf(stderr, "graph probe: %d\n", probe);
            return 6;
        }
    }
    if (older_frame_alias != (uintptr_t)text) return 7;
    registry = kiln_c99_collection_of(heap->realm.isolated_graphs);
    {
        double at = kiln_c99_collection_find(registry, handle);
        if (at < 0.0 || registry->entries[(size_t)at].value.bits != graph.bits ||
            heap->realm.isolated_request_id != 0 ||
            heap->realm.isolated_escrow_entry != NULL) return 8;
    }
    {
        kiln_c99_graph_capsule rejected;
        if (!kiln_c99_actor_isolated_request_begin(heap, handle, &id)) return 14;
        if (kiln_c99_graph_capsule_certify_request(heap, id, &rejected)) return 15;
        if (heap->realm.isolated_request_id != 0 ||
            heap->realm.isolated_escrow_entry != NULL ||
            registry->entries[0].value.bits != graph.bits) return 16;
    }
    return 0;
}
int main(void) {
    uint64_t realm;
    int result;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 9;
    result = kiln_realm_enter(realm, actor_boundary_entry, NULL);
    if (result != KILN_C99_OK) return result;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 10;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-request-machine-boundary");
    assert!(
        run.status.success(),
        "actor machine boundary check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_actor_isolated_registry_escrow_rolls_back_without_allocation() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int actor_escrow_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_value args[3], handle, graph, result, thrown;
    kiln_collection *registry;
    kiln_entry *entry;
    double at;
    uint64_t request_id = 0, next_id = 0;
    (void)context;
    if (heap == NULL || kiln_c99_new_object(&handle) != KILN_C99_OK ||
        kiln_c99_new_object(&graph) != KILN_C99_OK) return 1;
    args[0] = kiln_c99_number(26);
    args[1] = handle;
    args[2] = graph;
    if (kiln_c99_worker_op_raw(&result, args, 3, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result)) return 2;
    registry = kiln_c99_collection_of(heap->realm.isolated_graphs);
    at = kiln_c99_collection_find(registry, handle);
    if (at < 0.0) return 3;
    entry = &registry->entries[(size_t)at];
    if (entry->value.tag != graph.tag || entry->value.bits != graph.bits) return 4;
    if (!kiln_c99_actor_isolated_request_begin(heap, handle, &request_id) || request_id == 0)
        return 5;
    if (kiln_c99_actor_isolated_request_begin(heap, handle, &next_id) ||
        kiln_c99_actor_isolated_request_escrow(heap, request_id + 1) ||
        kiln_c99_actor_isolated_request_rollback(heap, request_id + 1)) return 14;
    if (!kiln_c99_actor_isolated_request_escrow(heap, request_id)) return 15;
    if (kiln_c99_actor_isolated_escrow_begin(heap, handle)) return 6;
    if (entry->value.tag != KILN_VALUE_UNDEFINED ||
        heap->realm.isolated_escrow_value.bits != graph.bits) return 7;
    if (!kiln_c99_actor_isolated_request_rollback(heap, request_id)) return 8;
    if (kiln_c99_actor_isolated_request_rollback(heap, request_id) ||
        kiln_c99_actor_isolated_escrow_rollback(heap) ||
        heap->realm.isolated_request_handle.tag != KILN_VALUE_UNDEFINED) return 9;
    if (entry->value.tag != graph.tag || entry->value.bits != graph.bits ||
        heap->realm.isolated_escrow_entry != NULL) return 10;
    if (!kiln_c99_actor_isolated_request_begin(heap, handle, &next_id) ||
        next_id <= request_id) return 16;
    if (kiln_c99_worker_op_raw(&result, args, 3, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result) ||
        kiln_c99_actor_isolated_request_escrow(heap, next_id) ||
        !kiln_c99_actor_isolated_request_rollback(heap, next_id)) return 17;
    args[0] = kiln_c99_number(27);
    if (kiln_c99_worker_op_raw(&result, args, 2, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result)) return 11;
    return 0;
}
int main(void) {
    uint64_t realm;
    int result;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 12;
    result = kiln_realm_enter(realm, actor_escrow_entry, NULL);
    if (result != KILN_C99_OK) return result;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 13;
}
"#;
    let run = run_with_main(&generated, wrapper, "actor-isolated-escrow");
    assert!(
        run.status.success(),
        "actor isolated escrow check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_request_excludes_only_its_escrow_slot() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int graph_request_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *batch[1];
    kiln_val *root;
    kiln_value args[3], result, thrown, handle, graph;
    uint64_t id = 0;
    (void)context;
    if (heap == NULL || !kiln_c99_graph_candidate_open(&candidate, heap,
                                                        heap->nursery.capacity)) return 1;
    root = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *root);
    if (root == NULL) return 2;
    *root = KILN_VAL_UNDEFINED;
    kiln_c99_par_arena_seal(&candidate);
    if (candidate.filled == NULL || !kiln_c99_heap_adopt_chunks(heap, candidate.filled)) return 3;
    batch[0] = candidate.filled;
    if (kiln_c99_new_object(&handle) != KILN_C99_OK) return 4;
    graph.tag = KILN_VALUE_CELL;
    graph.bits = (uint64_t)(uintptr_t)root;
    args[0] = kiln_c99_number(26);
    args[1] = handle;
    args[2] = graph;
    if (kiln_c99_worker_op_raw(&result, args, 3, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(result)) return 5;
    if (!kiln_c99_actor_isolated_request_begin(heap, handle, &id) ||
        !kiln_c99_actor_isolated_request_escrow(heap, id)) return 6;
    if (!kiln_c99_graph_has_c_root_alias(heap, batch, 1)) return 7;
    if (kiln_c99_graph_has_c_root_alias_for_request(heap, batch, 1, id)) return 8;
    if (!kiln_c99_graph_has_c_root_alias_for_request(heap, batch, 1, id + 1)) return 9;
    heap->realm.array_unscopables = graph;
    if (!kiln_c99_graph_has_c_root_alias_for_request(heap, batch, 1, id)) return 10;
    heap->realm.array_unscopables = kiln_c99_undefined();
    if (!kiln_c99_actor_isolated_request_rollback(heap, id)) return 11;
    if (!kiln_c99_graph_has_c_root_alias_for_request(heap, batch, 1, id)) return 12;
    return 0;
}
int main(void) {
    uint64_t realm;
    int result;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 13;
    result = kiln_realm_enter(realm, graph_request_entry, NULL);
    if (result != KILN_C99_OK) return result;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 14;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-request-escrow-alias");
    assert!(
        run.status.success(),
        "graph request escrow alias check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_graph_incoming_probe_sees_provider_root_aliases() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static void graph_provider_noop_drop(void *pointer) { (void)pointer; }
static void graph_provider_mark(void *heap, void *context, int phase) {
    (void)heap; (void)context; (void)phase;
}
static int graph_provider_visit(void *context, kiln_gc_visit_root_fn visit, void *visit_context) {
    return visit(visit_context, context, KILN_GC_VISIT_BLOCK);
}
static int graph_provider_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *chunk, *batch[1];
    kiln_val *root;
    kiln_value *slot;
    void *store = NULL;
    (void)context;
    if (heap == NULL || !kiln_c99_graph_candidate_open(&candidate, heap,
                                                        heap->nursery.capacity)) return 1;
    root = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *root);
    if (root == NULL) return 2;
    *root = KILN_VAL_UNDEFINED;
    kiln_c99_par_arena_seal(&candidate);
    chunk = candidate.filled;
    if (chunk == NULL || !kiln_c99_heap_adopt_chunks(heap, chunk)) return 3;
    batch[0] = chunk;
    if (kiln_c99_graph_incoming_probe(heap, batch, 1, root) !=
        KILN_C99_GRAPH_ROOTS_UNENUMERATED) return 4;
    if (kiln_realm_value_slot(5, (void **)&slot) != KILN_C99_OK) return 5;
    slot->tag = KILN_VALUE_CELL;
    slot->bits = (uint64_t)(uintptr_t)root;
    if (kiln_c99_graph_incoming_probe(heap, batch, 1, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOT_ALIAS) return 6;
    *slot = kiln_c99_undefined();
    if (kiln_c99_graph_incoming_probe(heap, batch, 1, root) !=
        KILN_C99_GRAPH_ROOTS_UNENUMERATED) return 7;
    if (kiln_realm_gc_register_roots_with_visitor(graph_provider_mark, graph_provider_visit,
                                                   root, 0) != KILN_C99_OK) return 8;
    if (kiln_c99_graph_incoming_probe(heap, batch, 1, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOT_ALIAS) return 9;
    if (kiln_realm_callable_store(1, 991, root, graph_provider_noop_drop, &store) !=
        KILN_C99_OK || store != root) return 12;
    if (kiln_c99_graph_incoming_probe(heap, batch, 1, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOT_ALIAS) return 13;
    return 0;
}
static int graph_provider_incomplete(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_c99_par_arena candidate;
    kiln_nursery_chunk *batch[1];
    kiln_val *root;
    (void)context;
    if (heap == NULL || !kiln_c99_graph_candidate_open(&candidate, heap,
                                                        heap->nursery.capacity)) return 14;
    root = (kiln_val *)kiln_c99_par_alloc(&candidate, KILN_GC_CELL, sizeof *root);
    if (root == NULL) return 15;
    *root = KILN_VAL_UNDEFINED;
    kiln_c99_par_arena_seal(&candidate);
    if (candidate.filled == NULL || !kiln_c99_heap_adopt_chunks(heap, candidate.filled)) return 16;
    batch[0] = candidate.filled;
    /* A mark-only embedding callback cannot be interpreted as an empty root set, even when
     * the callback happens to do nothing in this fixture. */
    if (kiln_realm_gc_register_roots(graph_provider_mark, NULL, 0) != KILN_C99_OK) return 17;
    if (kiln_c99_graph_incoming_probe(heap, batch, 1, root) !=
        KILN_C99_GRAPH_PROVIDER_ROOTS_UNENUMERATED) return 18;
    return 0;
}
int main(void) {
    uint64_t realm;
    int result;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 10;
    result = kiln_realm_enter(realm, graph_provider_entry, NULL);
    if (result != KILN_C99_OK) return result;
    if (kiln_realm_destroy(realm) != KILN_C99_OK) return 11;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 19;
    result = kiln_realm_enter(realm, graph_provider_incomplete, NULL);
    if (result != KILN_C99_OK) return result;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 20;
}
"#;
    let run = run_with_main(&generated, wrapper, "graph-provider-root-alias");
    assert!(
        run.status.success(),
        "provider root alias check failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}
#[test]
fn executable_gc_boundary_defers_partial_nursery_but_forced_collects() {
    let program = program(vec![], vec![Instruction::Return], 0);
    let generated = emit(&program).expect("program emits");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap heap;
    kiln_c99_gc_heap_init(&heap);
    heap.enabled = 1;

    /* A normal drive boundary must retain a partial chunk until the allocator's pressure
     * signal says it is due; otherwise every await turn pays a complete root walk. */
    if (kiln_c99_gc_nursery_bump(&heap, KILN_GC_BYTES, 32) == NULL) return 1;
    uint64_t before = heap.collections;
    uint32_t before_epoch = heap.epoch;
    kiln_c99_gc_collect(&heap, 0);
    if (heap.collections != before || heap.epoch != before_epoch) return 2;
    kiln_c99_gc_collect(&heap, 0);
    if (heap.collections != before || heap.epoch != before_epoch) return 3;

    /* An explicit force still settles a partial chunk, even without stress mode. */
    kiln_c99_gc_collect(&heap, 1);
    if (heap.collections != before + 1 || heap.epoch == before_epoch) return 4;

    /* Normal size pressure collects a filled chunk and resets dead nursery allocations. */
    for (size_t chunk = 0; chunk < 4; ++chunk) {
        size_t allocations = 0;
        while (kiln_c99_gc_nursery_bump(&heap, KILN_GC_BYTES, 32) != NULL) {
            if (++allocations > 10000) return 5;
        }
        kiln_c99_nursery_turn_chunk(&heap);
    }
    before = heap.collections;
    before_epoch = heap.epoch;
    kiln_c99_gc_collect(&heap, 0);
    if (heap.collections <= before || heap.epoch == before_epoch || heap.nursery.full != NULL) return 6;

    /* Stress remains an independent path for a partial current chunk. */
    if (kiln_c99_gc_nursery_bump(&heap, KILN_GC_BYTES, 32) == NULL) return 7;
    before = heap.collections;
    heap.stress = 1;
    kiln_c99_gc_collect(&heap, 0);
    if (heap.collections != before + 1) return 8;
    kiln_c99_nursery_destroy(&heap.nursery);
    free(heap.large_table.table);
    free(heap.slab_table.table);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "gc-boundary-pressure-test");
    assert!(
        run.status.success(),
        "executable GC boundary pressure test failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_gc_inactive_nursery_still_finds_filled_chunk_roots() {
    let program = program(vec![], vec![Instruction::Return], 0);
    let generated = emit(&program).expect("program emits");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap heap;
    void *volatile root;
    void *unused_current;
    kiln_c99_gc_heap_init(&heap);
    heap.enabled = 1;
    root = kiln_c99_gc_nursery_bump(&heap, KILN_GC_BYTES, 32);
    if (root == NULL) return 1;
    memset((void *)root, 0x5a, 32);
    kiln_c99_nursery_turn_chunk(&heap);
    if (heap.nursery.full == NULL) return 2;
    unused_current = heap.nursery.region;

    /* Simulate an allocation failure while retaining the already-filled region. */
    kiln_c99_nursery_free_region(&heap.nursery, unused_current);
    heap.nursery.active = 0;
    heap.nursery.region = NULL;
    heap.nursery.base = NULL;
    heap.nursery.bump = NULL;
    heap.nursery.limit = NULL;
    heap.nursery.bitmap = NULL;
    if (kiln_c99_gc_lookup(&heap, (void *)root) == NULL) return 3;
    kiln_c99_gc_collect_mid_turn(&heap);
    if (heap.nursery.full != NULL || heap.nursery.promoted == NULL) return 4;
    if (kiln_c99_gc_lookup(&heap, (void *)root) == NULL) return 5;
    for (size_t index = 0; index < 32; ++index) {
        if (((unsigned char *)root)[index] != 0x5a) return 6;
    }
    kiln_c99_gc_collect_mid_turn(&heap);
    if (kiln_c99_gc_lookup(&heap, (void *)root) == NULL) return 7;
    for (size_t index = 0; index < 32; ++index) {
        if (((unsigned char *)root)[index] != 0x5a) return 8;
    }

    kiln_c99_nursery_destroy(&heap.nursery);
    free(heap.large_table.table);
    free(heap.slab_table.table);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "gc-inactive-nursery-test");
    assert!(
        run.status.success(),
        "inactive nursery lookup test failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_gc_stack_scan_uses_the_current_lane_after_a_heap_moves() {
    let program = program(vec![], vec![Instruction::Return], 0);
    let generated = emit(&program).expect("program emits");
    let wrapper = r#"
typedef struct {
    kiln_gc_heap *heap;
    void *root;
    void *first_stack_base;
    int failed;
} kiln_stack_move_test;

static void *kiln_stack_move_second_lane(void *opaque) {
    kiln_stack_move_test *test = (kiln_stack_move_test *)opaque;
    void *volatile root = test->root;
    void *second_stack_base = kiln_c99_gc_stack_base();
    if (second_stack_base == NULL || second_stack_base == test->first_stack_base) {
        test->failed = 1;
        return NULL;
    }
    kiln_c99_gc_collect_mid_turn(test->heap);
    if (kiln_c99_gc_lookup(test->heap, (void *)root) == NULL) test->failed = 2;
    for (size_t index = 0; index < 32; ++index) {
        if (((unsigned char *)root)[index] != 0x5a) test->failed = 3;
    }
    return NULL;
}

int main(void) {
    kiln_gc_heap heap;
    void *volatile root;
    kiln_stack_move_test test;
    pthread_t second;
    kiln_c99_gc_heap_init(&heap);
    heap.enabled = 1;
    root = kiln_c99_gc_nursery_bump(&heap, KILN_GC_BYTES, 32);
    if (root == NULL) return 1;
    memset((void *)root, 0x5a, 32);
    kiln_c99_nursery_turn_chunk(&heap);
    kiln_c99_gc_collect_mid_turn(&heap);
    if (kiln_c99_gc_lookup(&heap, (void *)root) == NULL) return 2;
    test.heap = &heap;
    test.root = (void *)root;
    test.first_stack_base = kiln_c99_gc_stack_base();
    test.failed = 0;
    if (pthread_create(&second, NULL, kiln_stack_move_second_lane, &test) != 0) return 3;
    if (pthread_join(second, NULL) != 0) return 4;
    if (test.failed != 0) return 4 + test.failed;
    kiln_c99_nursery_destroy(&heap.nursery);
    free(heap.large_table.table);
    free(heap.slab_table.table);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "gc-stack-moved-lane-test");
    assert!(
        run.status.success(),
        "moved-heap stack scan failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_cached_shape_object_literal_executes_correctly() {
    // Function 0 (maker): takes 2 arguments, returns { x: arg0, y: arg1 }
    let maker = FunctionProto {
        inferred_name: None,
        prologue_end: 0,
        code: vec![
            Instruction::Const(0),     // "x"
            Instruction::LoadLocal(0), // arg0
            Instruction::Const(1),     // "y"
            Instruction::LoadLocal(1), // arg1
            Instruction::NewObject { count: 2 },
            Instruction::Return,
        ],
        parameter_count: 2,
        local_count: 2,
        ..function(Vec::new())
    };

    // Main body:
    // make = MakeFunction(0)
    // obj1 = make(10, 20)
    // obj2 = make(30, 40)
    // return (obj1.x + obj1.y + obj2.x + obj2.y) === 100
    let constants = vec![
        Constant::String("x".into()),         // 0
        Constant::String("y".into()),         // 1
        Constant::Number(10.0f64.to_bits()),  // 2
        Constant::Number(20.0f64.to_bits()),  // 3
        Constant::Number(30.0f64.to_bits()),  // 4
        Constant::Number(40.0f64.to_bits()),  // 5
        Constant::Number(100.0f64.to_bits()), // 6
    ];

    let code = vec![
        Instruction::MakeFunction(0),
        Instruction::StoreLocal(0), // make fn
        // obj1 = make(10, 20)
        Instruction::LoadLocal(0),
        Instruction::Const(2),
        Instruction::Const(3),
        Instruction::Call { argc: 2 },
        Instruction::StoreLocal(1), // obj1
        // obj2 = make(30, 40)
        Instruction::LoadLocal(0),
        Instruction::Const(4),
        Instruction::Const(5),
        Instruction::Call { argc: 2 },
        Instruction::StoreLocal(2), // obj2
        // sum = obj1.x + obj1.y + obj2.x + obj2.y
        Instruction::LoadLocal(1),
        Instruction::GetProperty(0),
        Instruction::LoadLocal(1),
        Instruction::GetProperty(1),
        Instruction::Binary(BinaryOp::Add),
        Instruction::LoadLocal(2),
        Instruction::GetProperty(0),
        Instruction::Binary(BinaryOp::Add),
        Instruction::LoadLocal(2),
        Instruction::GetProperty(1),
        Instruction::Binary(BinaryOp::Add),
        Instruction::Const(6), // 100
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 3,
        functions: vec![maker],
        template_sites: Vec::new(),
    };

    let generated = emit(&program).expect("program emits");
    assert!(
        generated
            .source
            .contains("static const kiln_shape *lit_shape = NULL;"),
        "expected lit_shape optimization in generated source:\n{}",
        generated.source
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "cached-shape-object-literal");
    assert!(
        run.status.success(),
        "cached shape object literal execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Realms issue 04 step 5: a unit's module globals are the realm's. Two heaps on one thread, the
/// way a thread hosting several realms has them, each run the same unit's body: each gets its own
/// block, allocated on its first entry (a realm that never ran the body sees nothing), and a
/// closure the first realm made still reads the first realm's global after the second realm ran
/// the body and stored its own. Before step 5 both read one static array, so the closure saw the
/// second realm's object -- which that realm's teardown would then free under it.
#[test]
fn two_realms_running_one_unit_keep_their_own_module_globals() {
    let mut top = program(
        Vec::new(),
        vec![
            Instruction::NewObject { count: 0 },
            Instruction::StoreGlobal(0),
            Instruction::MakeFunction(0),
            Instruction::StoreGlobal(1),
            Instruction::LoadGlobal(1),
            Instruction::Return,
        ],
        2,
    );
    let mut read = function(vec![Instruction::LoadGlobal(0), Instruction::Return]);
    read.name = Some("read".to_owned());
    top.functions = vec![read];
    let generated = emit(&top).expect("a unit with a module global emits");
    let symbol = &generated.symbol;
    assert!(
        generated
            .source
            .contains(&format!("#define {symbol}_globals ({symbol}_unit_vals())")),
        "module globals are reached through the realm's block"
    );
    assert!(
        generated.source.contains("kiln_val *const kiln_mg = "),
        "a function reading its module's globals asks for the block once, at entry"
    );
    let wrapper = format!(
        r#"
static kiln_gc_heap *kiln_test_new_heap(void) {{
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    if (heap == NULL) return NULL;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    return heap;
}}
static void kiln_test_enter(kiln_gc_heap *heap) {{
    kiln_test_heap = heap;
    kiln_test_generation += 1;
}}
int main(void) {{
    kiln_gc_heap *first;
    kiln_gc_heap *second;
    kiln_value closure_one, closure_two, read_one, read_two, thrown = kiln_c99_undefined();
    kiln_val global_one, global_two;
    first = kiln_test_new_heap();
    second = kiln_test_new_heap();
    if (first == NULL || second == NULL) return 1;

    kiln_test_enter(first);
    if ({symbol}_body(NULL, &closure_one) != KILN_C99_OK) return 2;
    if ({symbol}_globals[1] == 0) return 20;
    global_one = {symbol}_globals[0];
    if (!KILN_VAL_IS_OBJECT(global_one)) return 3;
    if ({symbol}_unit.id == 0 || first->realm.units == NULL ||
        first->realm.units[{symbol}_unit.id] == NULL) return 4;

    /* A realm that has not run the body has an empty block of its own. */
    kiln_test_enter(second);
    if ({symbol}_globals[0] != 0) return 5;
    if ({symbol}_body(NULL, &closure_two) != KILN_C99_OK) return 6;
    global_two = {symbol}_globals[0];
    if (!KILN_VAL_IS_OBJECT(global_two) || global_two == global_one) return 7;

    /* Back in the first realm: its global, through its own closure and directly. */
    kiln_test_enter(first);
    if ({symbol}_globals[0] != global_one) return 8;
    closure_one = kiln_val_to_boundary({symbol}_globals[1]);
    if (kiln_c99_call(NULL, &read_one, closure_one, kiln_c99_undefined(), NULL, 0, 0, &thrown) !=
        KILN_C99_OK) return 9;
    if (read_one.bits != kiln_val_to_boundary(global_one).bits) return 10;
    kiln_test_enter(second);
    closure_two = kiln_val_to_boundary({symbol}_globals[1]);
    if (kiln_c99_call(NULL, &read_two, closure_two, kiln_c99_undefined(), NULL, 0, 0, &thrown) !=
        KILN_C99_OK) return 11;
    if (read_two.bits != kiln_val_to_boundary(global_two).bits) return 12;

    /* The block is the heap's: marked with it, freed with it. */
    if (first->realm.units[{symbol}_unit.id]->val_count != 2) return 13;
    kiln_c99_gc_teardown_begin(first);
    kiln_c99_gc_teardown_end(first);
    kiln_test_enter(second);
    if ({symbol}_globals[0] != global_two) return 14;
    kiln_c99_gc_teardown_begin(second);
    kiln_c99_gc_teardown_end(second);
    return 0;
}}
"#
    );
    let run = run_with_main(&generated, &wrapper, "two-realms-module-globals");
    assert!(
        run.status.success(),
        "two realms shared a unit's module globals ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Issue 20: a speculation asks about the built-ins it relies on. The object-granular answer
/// reads the realm's program record (with the prototypes every lookup falls through to folded
/// in); the member-granular one reads the built-in's own-property store, so `Math.foo = 1` leaves
/// `Math.sqrt` the table's. What a bootstrap stored is part of the built-ins, not a modification,
/// and a remembered member answer is dropped by the next store.
#[test]
fn a_speculation_asks_only_about_the_builtins_it_relies_on() {
    let generated = emit(&program(
        vec![Constant::String("sqrt".into())],
        vec![
            Instruction::LoadBuiltin(Builtin::Math),
            Instruction::GetProperty(0),
            Instruction::Return,
        ],
        0,
    ))
    .expect("a unit naming one key emits");
    let wrapper = r#"
#define KILN_TEST_MASK(id) KILN_C99_BIT_OF_BUILTIN(id, 0), KILN_C99_BIT_OF_BUILTIN(id, 1), 0
int main(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    kiln_value store;
    if (heap == NULL) return 1;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    kiln_test_heap = heap;
    kiln_test_generation += 1;

    /* Nothing stored: every answer is yes. */
    if (!kiln_c99_realm_builtins_untouched(KILN_TEST_MASK(KILN_C99_BUILTIN_MATH))) return 2;
    if (!kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_MATH, "sqrt", 4)) return 3;

    /* The bootstrap defines a member on Math: while it runs that is a store, and once it is done
     * it is what Math is. */
    if (kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_MATH), &store, 1) != KILN_C99_OK ||
        store.tag != KILN_VALUE_OBJECT)
        return 4;
    if (kiln_c99_define_method_key(store, kiln_c99_intern("boot", 4), kiln_c99_builtin(KILN_C99_BUILTIN_JSON)) != KILN_C99_OK)
        return 5;
    if (kiln_c99_realm_builtins_untouched(KILN_TEST_MASK(KILN_C99_BUILTIN_MATH))) return 6;
    if (kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_MATH, "boot", 4)) return 7;
    kiln_c99_builtin_bootstrap_done();
    if (!kiln_c99_realm_builtins_untouched(KILN_TEST_MASK(KILN_C99_BUILTIN_MATH))) return 8;
    if (!kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_MATH, "boot", 4)) return 9;

    /* The program writes `Math.foo`: Math is modified as a whole, but `Math.sqrt` is still the
     * table's, and Array is untouched. The answer about `sqrt` is remembered. */
    if (kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_MATH), &store, 1) != KILN_C99_OK) return 10;
    if (kiln_c99_define_method_key(store, kiln_c99_intern("foo", 3), kiln_c99_builtin(KILN_C99_BUILTIN_JSON)) != KILN_C99_OK)
        return 11;
    if (kiln_c99_realm_builtins_untouched(KILN_TEST_MASK(KILN_C99_BUILTIN_MATH))) return 12;
    if (!kiln_c99_realm_builtins_untouched(KILN_TEST_MASK(KILN_C99_BUILTIN_ARRAY))) return 13;
    if (!kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_MATH, "sqrt", 4)) return 14;
    if (kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_MATH, "foo", 3)) return 15;
    if (!kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_NUMBER, "isInteger", 9)) return 16;

    /* Then `Math.sqrt` itself: the remembered answer goes with the store. */
    if (kiln_c99_callable_store(kiln_c99_builtin(KILN_C99_BUILTIN_MATH), &store, 1) != KILN_C99_OK) return 17;
    if (kiln_c99_define_method_key(store, kiln_c99_intern("sqrt", 4), kiln_c99_builtin(KILN_C99_BUILTIN_JSON)) != KILN_C99_OK)
        return 18;
    if (kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_MATH, "sqrt", 4)) return 19;

    /* Object.prototype is where every lookup ends: writing it touches every object-granular
     * answer, and no member found on the built-in itself. */
    kiln_c99_realm_note_store();
    kiln_c99_realm_note_builtin_store(KILN_C99_BUILTIN_OBJECT_PROTOTYPE);
    if (kiln_c99_realm_builtins_untouched(KILN_TEST_MASK(KILN_C99_BUILTIN_ARRAY))) return 20;
    if (!kiln_c99_realm_builtin_member_untouched(KILN_C99_BUILTIN_NUMBER, "isInteger", 9)) return 21;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "narrow-builtin-guards");
    assert!(
        run.status.success(),
        "a speculation answering about built-ins it does not rely on ({:?}): {}{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Parallel issue 06's write detector, and the tier-2 readers under it (issue 07). With
/// `KILN_PARALLEL_PROTECT=1` a parallel section maps the realm heap's slabs and nursery regions
/// read-only: the readers answer own, inherited and absent names and a string's length through
/// it, in no-write mode, and a write to an object's header faults (in a child, so the test sees
/// the signal) until the section ends.
#[cfg(unix)]
#[test]
fn a_parallel_section_maps_the_heap_read_only_and_the_tier_2_readers_read_through_it() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
#include <sys/wait.h>
#include <unistd.h>
static kiln_gc_heap *kiln_test_new_heap(void) {
    kiln_gc_heap *heap = (kiln_gc_heap *)malloc(sizeof *heap);
    if (heap == NULL) return NULL;
    kiln_c99_gc_heap_init(heap);
    heap->enabled = 0;
    kiln_c99_gc_heap_register(heap);
    return heap;
}
static void kiln_test_enter(kiln_gc_heap *heap) {
    kiln_test_heap = heap;
    kiln_test_generation += 1;
}
int main(void) {
    kiln_gc_heap *heap = kiln_test_new_heap();
    kiln_value proto, object, text;
    kiln_c99_par_env env;
    kiln_c99_par_site own_site, inherited_site, absent_site;
    kiln_c99_par_frame frame;
    kiln_val out;
    const kiln_string *own, *inherited, *absent;
    pid_t child;
    int status;
    if (heap == NULL) return 1;
    kiln_test_enter(heap);
    if (kiln_c99_new_object(&proto) != KILN_C99_OK ||
        kiln_c99_put(NULL, proto, "inherited", 9, kiln_c99_number(2.0), 0, NULL) != KILN_C99_OK) return 2;
    if (kiln_c99_new_object(&object) != KILN_C99_OK ||
        kiln_c99_put(NULL, object, "own", 3, kiln_c99_number(1.0), 0, NULL) != KILN_C99_OK) return 3;
    kiln_c99_object_of(object)->prototype = proto;
    own = kiln_c99_intern("own", 3);
    inherited = kiln_c99_intern("inherited", 9);
    absent = kiln_c99_intern("absent", 6);
    text = kiln_c99_copy_text("plain", 5);
    if (own == NULL || inherited == NULL || absent == NULL || text.tag != KILN_VALUE_STRING) return 4;
    setenv("KILN_PARALLEL_PROTECT", "1", 1);
    memset(&env, 0, sizeof env);
    memset(&own_site, 0, sizeof own_site);
    memset(&inherited_site, 0, sizeof inherited_site);
    memset(&absent_site, 0, sizeof absent_site);
    env.heap = kiln_c99_par_prepare();
    if (env.heap != heap) return 5;

    kiln_c99_par_section(1);
    kiln_c99_par_enter(&env, &frame);
    if (!kiln_c99_no_write()) return 6;
    if (!kiln_c99_par_get(&env, &own_site, kiln_val_from_boundary(object), own, &out) ||
        out != kiln_c99_val_from_double(1.0)) return 7;
    /* The worker's own cache learned the slot; a second read hits it. */
    if (own_site.shape == NULL ||
        !kiln_c99_par_get(&env, &own_site, kiln_val_from_boundary(object), own, &out) ||
        out != kiln_c99_val_from_double(1.0)) return 8;
    if (!kiln_c99_par_get(&env, &inherited_site, kiln_val_from_boundary(object), inherited, &out) ||
        out != kiln_c99_val_from_double(2.0)) return 9;
    if (!kiln_c99_par_get(&env, &absent_site, kiln_val_from_boundary(object), absent, &out) ||
        out != KILN_VAL_UNDEFINED) return 10;
    if (!kiln_c99_par_length(&env, &absent_site, kiln_val_from_boundary(text), absent, &out) ||
        out != kiln_c99_val_from_double(5.0)) return 11;
    kiln_c99_par_leave(&frame);
    /* A write to the object's header, as a cache teaching itself would make, faults. */
    fflush(NULL);
    child = fork();
    if (child == 0) {
        kiln_c99_object_of(object)->layout_token = 7;
        _exit(0);
    }
    if (child < 0 || waitpid(child, &status, 0) != child) return 12;
    if (!WIFSIGNALED(status) || (WTERMSIG(status) != SIGSEGV && WTERMSIG(status) != SIGBUS)) return 13;
    kiln_c99_par_section(0);
    /* After the section the heap is writable again. */
    kiln_c99_object_of(object)->layout_token = 0;
    kiln_c99_gc_teardown_begin(heap);
    kiln_c99_gc_teardown_end(heap);
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "parallel-section-protect");
    assert!(
        run.status.success(),
        "the parallel section's write detector ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}
