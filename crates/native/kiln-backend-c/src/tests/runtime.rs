use super::*;

#[test]
fn class_field_data_descriptor_uses_canonical_definition_rules() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value target = kiln_c99_undefined();
    kiln_value descriptor = kiln_c99_undefined();
    kiln_value object_value = kiln_c99_undefined();
    kiln_value value = kiln_c99_undefined();
    kiln_value thrown = kiln_c99_undefined();
    int found = 0;
    if (kiln_c99_new_object(&target) != KILN_C99_OK ||
        kiln_c99_new_object(&object_value) != KILN_C99_OK) return 1;
    if (kiln_c99_create_data_property(NULL, target, kiln_c99_number(17.0),
                                      object_value, 0, &thrown) != KILN_C99_OK) return 2;
    if (kiln_c99_describe_property(NULL, target, "17", 2, &descriptor, &found, 0,
                                   &thrown) != KILN_C99_OK || !found) return 3;
    if (kiln_c99_get(NULL, descriptor, "value", 5, &value, 0, &thrown) != KILN_C99_OK ||
        value.tag != KILN_VALUE_OBJECT || value.ptr != object_value.ptr) return 4;
    if (kiln_c99_get(NULL, descriptor, "writable", 8, &value, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(value)) return 5;
    if (kiln_c99_get(NULL, descriptor, "enumerable", 10, &value, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(value)) return 6;
    if (kiln_c99_get(NULL, descriptor, "configurable", 12, &value, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(value)) return 7;

    kiln_c99_object_of(target)->not_extensible = 1;
    thrown = kiln_c99_undefined();
    if (kiln_c99_create_data_property(NULL, target, kiln_c99_number(18.0),
                                      kiln_c99_number(43.0), 0, &thrown) != KILN_C99_THROWN) return 8;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "class-field-prepared-descriptor");
    assert!(
        run.status.success(),
        "class field prepared descriptor changed own attributes/refusal ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn collection_for_each_validates_callback_before_empty_history_fast_return() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value map = kiln_c99_undefined();
    kiln_value set = kiln_c99_undefined();
    kiln_value args[1];
    kiln_value result = kiln_c99_undefined();
    kiln_value thrown = kiln_c99_undefined();
    kiln_value for_each = kiln_c99_method(KILN_C99_METHOD_COLLECTION_FOR_EACH);
    int status;
    if (kiln_c99_new_collection(&map, 1, 0) != KILN_C99_OK ||
        kiln_c99_new_collection(&set, 0, 0) != KILN_C99_OK) return 1;

    args[0] = kiln_c99_number(1.0);
    status = kiln_c99_call(NULL, &result, for_each, map, args, 1, 0, &thrown);
    if (status != KILN_C99_THROWN) return 2;
    thrown = kiln_c99_undefined();
    status = kiln_c99_call(NULL, &result, for_each, set, args, 1, 0, &thrown);
    if (status != KILN_C99_THROWN) return 3;

    if (kiln_c99_collection_put(map, kiln_c99_number(1.0), kiln_c99_number(10.0)) != KILN_C99_OK ||
        kiln_c99_collection_put(map, kiln_c99_number(2.0), kiln_c99_number(20.0)) != KILN_C99_OK ||
        kiln_c99_collection_put(set, kiln_c99_number(1.0), kiln_c99_number(1.0)) != KILN_C99_OK ||
        kiln_c99_collection_put(set, kiln_c99_number(2.0), kiln_c99_number(2.0)) != KILN_C99_OK) return 4;
    args[0] = kiln_c99_number(1.0);
    if (kiln_c99_call(NULL, &result, kiln_c99_method(KILN_C99_METHOD_COLLECTION_DELETE),
                      map, args, 1, 0, &thrown) != KILN_C99_OK || !kiln_c99_truthy(result)) return 5;
    if (kiln_c99_call(NULL, &result, kiln_c99_method(KILN_C99_METHOD_COLLECTION_DELETE),
                      set, args, 1, 0, &thrown) != KILN_C99_OK || !kiln_c99_truthy(result)) return 6;
    args[0] = kiln_c99_number(2.0);
    if (kiln_c99_call(NULL, &result, kiln_c99_method(KILN_C99_METHOD_COLLECTION_DELETE),
                      map, args, 1, 0, &thrown) != KILN_C99_OK || !kiln_c99_truthy(result)) return 7;
    if (kiln_c99_call(NULL, &result, kiln_c99_method(KILN_C99_METHOD_COLLECTION_DELETE),
                      set, args, 1, 0, &thrown) != KILN_C99_OK || !kiln_c99_truthy(result)) return 8;

    args[0] = kiln_c99_number(1.0);
    if (kiln_c99_call(NULL, &result, for_each, map, args, 1, 0, &thrown) != KILN_C99_THROWN) return 9;
    thrown = kiln_c99_undefined();
    if (kiln_c99_call(NULL, &result, for_each, set, args, 1, 0, &thrown) != KILN_C99_THROWN) return 10;

    /* This callable would throw if called on the callback arguments, but an empty history must
     * return normally without invoking it. */
    args[0] = kiln_c99_method(KILN_C99_METHOD_NUMBER_TO_STRING);
    if (kiln_c99_call(NULL, &result, for_each, map, args, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_UNDEFINED) return 11;
    if (kiln_c99_call(NULL, &result, for_each, set, args, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_UNDEFINED) return 12;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "collection-for-each-empty-history");
    assert!(
        run.status.success(),
        "forEach callback validation / empty history fast path failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn text_decoder_rejects_other_external_owner_context_before_reading_it() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int tiny_visit(void *context, kiln_gc_visit_root_fn visit, void *visit_context) {
    (void)context; (void)visit; (void)visit_context; return 0;
}
static void tiny_release(void *context) { (void)context; }
extern void *kiln_c99_text_decoder_state(kiln_value receiver);
int main(void) {
    unsigned char tiny_context = 0;
    void *owner = kiln_c99_external_owner_create(&tiny_context, tiny_visit, tiny_release);
    kiln_value object = kiln_c99_undefined();
    if (owner == NULL || kiln_c99_new_object(&object) != KILN_C99_OK) return 1;
    if (kiln_c99_object_set_external_owner(object, owner) != KILN_C99_OK) return 2;
    if (kiln_c99_text_decoder_state(object) != NULL) return 3;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "text-decoder-wrong-owner");
    assert!(
        run.status.success(),
        "wrong external owner was not safely rejected ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn object_initializer_clears_external_owner_in_poisoned_memory() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_object object;
    memset(&object, 0xa5, sizeof object);
    kiln_c99_object_init(&object, 0);
    return object.external_owner == NULL ? 0 : 1;
}
"#;
    let run = run_with_main(&generated, wrapper, "object-external-owner-initialized");
    assert!(
        run.status.success(),
        "object initializer retained poisoned external_owner ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn parallel_arena_initializers_clear_runtime_metadata() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_c99_par_env env = {0};
    kiln_c99_par_arena *arena;
    kiln_val object_value = KILN_VAL_UNDEFINED;
    kiln_val array_value = KILN_VAL_UNDEFINED;
    kiln_object *object;
    kiln_array *array;
    int result = 0;
    env.heap = kiln_c99_gc_heap();
    if (!kiln_c99_par_arenas_open(&env, 1)) return 1;
    arena = kiln_c99_par_arena_claim(&env);
    if (arena == NULL || !kiln_c99_par_arena_refill(arena)) {
        (void)kiln_c99_par_arenas_join(&env, 0);
        return 2;
    }
    memset(arena->bump + sizeof(kiln_gc_block), 0xa5, sizeof(kiln_object));
    if (!kiln_c99_par_object(arena, &object_value)) {
        (void)kiln_c99_par_arenas_join(&env, 0);
        return 3;
    }
    object = (kiln_object *)KILN_VAL_PTR(object_value);
    if (object->external_owner != NULL || object->raw_json_brand != 0 ||
        object->error_stack_frames.tag != KILN_VALUE_UNDEFINED ||
        object->prototype.tag != KILN_VALUE_UNDEFINED || object->properties != NULL ||
        object->shape == NULL || object->slots == NULL) result = 4;

    memset(arena->bump + sizeof(kiln_gc_block), 0xa5, sizeof(kiln_array));
    if (!kiln_c99_par_array(arena, NULL, 0, &array_value)) {
        (void)kiln_c99_par_arenas_join(&env, 0);
        return 5;
    }
    array = (kiln_array *)KILN_VAL_PTR(array_value);
    if (array->has_index_descriptors != 0 || array->properties.tag != KILN_VALUE_UNDEFINED ||
        array->match_index != -1 || array->match_input.tag != KILN_VALUE_UNDEFINED ||
        array->match_groups.tag != KILN_VALUE_UNDEFINED || array->elements != NULL ||
        array->count != 0 || array->capacity != 0) result = 6;
    (void)kiln_c99_par_arenas_join(&env, 0);
    return result;
}
"#;
    let run = run_with_main(&generated, wrapper, "parallel-arena-initializer-poison");
    assert!(
        run.status.success(),
        "parallel arena initializer retained poisoned metadata ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn poisoned_parallel_chunk_child_survives_minor_collection() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int poisoned_chunk_minor_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_c99_par_arena arena = {0};
    kiln_value parent = kiln_c99_undefined();
    kiln_value registry = kiln_c99_undefined();
    kiln_val child = KILN_VAL_UNDEFINED;
    kiln_val child_element = KILN_VAL_TRUE;
    kiln_array *parent_array;
    kiln_array *child_array;
    kiln_gc_block *child_block;
    kiln_nursery_chunk *chunk;
    char *region;
    size_t capacity;
    size_t minor_before;
    (void)context;
    if (heap == NULL) return 1;
    heap->enabled = 1;
    heap->fallback = 0;
    heap->stress = 0;
    heap->force_pending = 0;
    heap->floor = SIZE_MAX;
    heap->since_collection = 0;
    heap->minor_audit_enabled = 1;
    if (kiln_c99_new_array_with_capacity(&parent, 1, 0) != KILN_C99_OK ||
        kiln_c99_new_collection(&registry, 1, 0) != KILN_C99_OK) return 2;
    heap->realm.isolated_graphs = registry;
    if (kiln_c99_collection_put(registry, parent, parent) != KILN_C99_OK) return 3;

    capacity = heap->nursery.capacity;
    region = (char *)kiln_c99_gc_aligned_alloc(capacity, capacity);
    if (region == NULL) return 4;
    memset(region, 0xa5, capacity);
    chunk = kiln_c99_chunk_lay_out(region, heap->nursery.bitmap_words);
    arena.heap = heap;
    arena.current = chunk;
    arena.bump = chunk->base;
    arena.limit = region + capacity;
    arena.budget = capacity;
    arena.taken = capacity;
    if (!kiln_c99_par_array(&arena, &child_element, 1, &child)) {
        kiln_c99_gc_aligned_free(region);
        return 5;
    }
    kiln_c99_par_arena_seal(&arena);
    if (!kiln_c99_heap_adopt_chunks(heap, arena.filled)) return 6;
    parent_array = (kiln_array *)(uintptr_t)parent.bits;
    kiln_c99_heap_store_val(parent_array, &parent_array->elements[0], child);
    parent_array->count = 1;
    minor_before = heap->minor_audit_collections;
    kiln_c99_gc_collect_minor_mid_turn(heap);

    child_block = kiln_c99_gc_lookup(heap, KILN_VAL_PTR(child));
    if (heap->minor_audit_collections != minor_before + 1 || child_block == NULL) return 7;
    child_array = (kiln_array *)(uintptr_t)KILN_VAL_PTR(child);
    if (child_array->count != 1 || child_array->elements[0] != KILN_VAL_TRUE) return 8;
    return 0;
}

int main(void) {
    uint64_t realm;
    int status;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 9;
    status = kiln_realm_enter(realm, poisoned_chunk_minor_entry, NULL);
    if (kiln_realm_destroy(realm) != KILN_C99_OK) return 10;
    return status;
}
"#;
    let run = run_with_main(&generated, wrapper, "poisoned-parallel-chunk-minor");
    assert!(
        run.status.success(),
        "rooted child in poisoned parallel chunk did not survive minor collection ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn runtime_module_materializer_stays_inside_the_backend_value_abi() {
    let module = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![kiln_ir::ExportBinding {
            exported: "probe".to_owned(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let emitted = emit_runtime_module_with_logical_path(
        &module,
        ".kiln/builtins/probe.builtin",
        "exports[0] = kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK);",
    )
    .expect("runtime module emits");

    assert!(
        emitted
            .source
            .contains("exports[0] = kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK);")
    );
    assert!(emitted.source.contains("static int kiln_module_path_"));
    assert!(emitted.source.contains("kiln_realm_enter"));
    assert!(!emitted.source.contains("extern kiln_value"));

    let decoupled = emit_runtime_module_with_logical_path_and_policy(
        &module,
        ".kiln/builtins/probe.builtin",
        "exports[0] = kiln_c99_method(KILN_C99_METHOD_QUEUE_MICROTASK);",
        EmissionPolicy {
            decoupled_prelude: true,
            ..EmissionPolicy::default()
        },
    )
    .expect("decoupled runtime module emits");
    assert!(
        decoupled
            .source
            .contains(&format!("#include \"{RUNTIME_PRELUDE_HEADER_NAME}\""))
    );
    assert!(!decoupled.source.contains("enum kiln_value_tag"));
}

#[test]
fn method_metadata_is_registry_generated_and_unknown_codes_are_zero() {
    let source = runtime_prelude_source();
    assert!(source.contains("typedef struct kiln_c99_method_metadata"));
    assert!(source.contains("kiln_c99_method_metadata_for(uint64_t code)"));
    assert!(source.contains(
        "case KILN_C99_METHOD_ARRAY_COPY_WITHIN: return (kiln_c99_method_metadata){UINT32_C(2), UINT32_C(0x7), UINT32_C(0)};"
    ));
    assert!(source.contains(
        "case KILN_C99_METHOD_ESCAPE: return (kiln_c99_method_metadata){UINT32_C(8), UINT32_C(0), UINT32_C(0x1)};"
    ));
    assert!(
        source
            .contains("return (kiln_c99_method_metadata){UINT32_C(0), UINT32_C(0), UINT32_C(0)};")
    );
    assert!(source.contains("code >= KILN_C99_METHOD_TEMPORAL_FROM"));
    assert!(source.contains("KILN_C99_GROUP_TEMPORAL | KILN_C99_GROUP_TEMPORAL_OP"));
    assert!(!source.contains("kiln_c99_numeric_arguments(uint64_t code)"));
    assert!(!source.contains("kiln_c99_textual_arguments(uint64_t code)"));
}

#[test]
fn executable_namespace_wrapper_materializes_sorted_immutable_namespace() {
    let module = ModuleProgram {
        program: Program {
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            source: String::new().into(),
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let generated = emit_module_with_logical_path(&module, "src/namespace.js")
        .expect("namespace helper module emits");
    let same = emit_module_with_logical_path(&module, "src/namespace.js")
        .expect("namespace helper module emits deterministically");
    assert_eq!(generated, same);
    let wrapper = format!(
        r#"
int main(void) {{
    static const char *names[] = {{"z", "a"}};
    static const size_t lengths[] = {{1, 1}};
    static const char *numeric_names[] = {{"2", "10"}};
    static const size_t numeric_lengths[] = {{1, 2}};
    static const char *reserved_names[] = {{"@@toStringTag"}};
    static const size_t reserved_lengths[] = {{13}};
    kiln_value values[2] = {{kiln_c99_number(2.0), kiln_c99_number(1.0)}};
    kiln_value namespace_object;
    kiln_value numeric_namespace;
    kiln_value empty_namespace;
    kiln_value cell = values[0];
    kiln_value read;
    kiln_value descriptor;
    kiln_value thrown = kiln_c99_undefined();
    kiln_value args[3];
    int found;
    if (kiln_c99_make_cell(&cell) != KILN_C99_OK) return 1;
    *kiln_c99_cell_of(cell) = kiln_c99_val_from_double(9.0);
    values[0] = cell;
    if ({symbol}_namespace_wrapper(&namespace_object, names, lengths, values, 2) != KILN_C99_OK)
        return 2;
    if (namespace_object.tag != KILN_VALUE_OBJECT) return 3;
    if (kiln_c99_object_of(namespace_object)->prototype.tag != KILN_VALUE_NULL) return 4;
    if (!kiln_c99_object_of(namespace_object)->module_namespace ||
        kiln_c99_object_of(namespace_object)->frozen) return 5;
    /* Extending an already-created namespace with no new exports is a valid no-op. */
    if ({symbol}_namespace_wrapper(&namespace_object, NULL, NULL, NULL, 0) != KILN_C99_OK)
        return 28;
    if (kiln_c99_object_of(namespace_object)->count != 3 ||
        !kiln_test_key_equal(kiln_c99_object_of(namespace_object)->properties[0].key, "a", 1) ||
        !kiln_test_key_equal(kiln_c99_object_of(namespace_object)->properties[1].key, "z", 1) ||
        !kiln_test_key_equal(kiln_c99_object_of(namespace_object)->properties[2].key,
                             "@@toStringTag", 13))
        return 6;
    if (kiln_c99_object_of(namespace_object)->properties[0].refused ||
        kiln_c99_object_of(namespace_object)->properties[0].symbol_hidden ||
        kiln_c99_object_of(namespace_object)->properties[1].refused ||
        kiln_c99_object_of(namespace_object)->properties[1].symbol_hidden ||
        !kiln_c99_object_of(namespace_object)->properties[2].symbol_hidden)
        return 28;
    if (kiln_c99_get(NULL, namespace_object, "z", 1, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(read) != 9.0) return 7;
    if (kiln_c99_describe_property(NULL, namespace_object, "a", 1, &descriptor, &found, 0,
                                   &thrown) != KILN_C99_OK || !found) return 8;
    if (kiln_c99_get(NULL, descriptor, "writable", 8, &read, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(read)) return 9;
    if (kiln_c99_get(NULL, descriptor, "enumerable", 10, &read, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(read)) return 10;
    if (kiln_c99_get(NULL, descriptor, "configurable", 12, &read, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_truthy(read)) return 11;
    args[0] = namespace_object;
    args[1] = kiln_c99_copy_text("a", 1);
    args[2] = descriptor;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY,
                               kiln_c99_undefined(), args, 3, 0, &thrown) != KILN_C99_OK)
        return 12;
    if (kiln_c99_put(NULL, descriptor, "writable", 8, kiln_c99_boolean(0), 0, &thrown) !=
        KILN_C99_OK ||
        kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_DEFINE_PROPERTY,
                               kiln_c99_undefined(), args, 3, 0, &thrown) != KILN_C99_THROWN)
        return 13;
    if (kiln_c99_get(NULL, namespace_object, "@@toStringTag", 13, &read, 0, &thrown) !=
            KILN_C99_OK ||
        read.tag != KILN_VALUE_STRING || !kiln_test_units_equal(read, "Module", 6))
        return 13;
    if (kiln_c99_put_mode(NULL, namespace_object, "a", 1, kiln_c99_number(3.0), 1, 0,
                          &thrown) != KILN_C99_THROWN)
        return 14;
    if (kiln_c99_get(NULL, thrown, "message", 7, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_STRING || !kiln_test_units_contain(read, "module namespace"))
        return 15;
    if (kiln_c99_write_would_accept(namespace_object, "a", 1)) return 16;
    if (kiln_c99_delete(NULL, namespace_object, "a", 1, &found, 0, &thrown) != KILN_C99_OK ||
        found != 0) return 17;
    if (kiln_c99_delete(NULL, namespace_object, "missing", 7, &found, 0, &thrown) !=
            KILN_C99_OK ||
        found != 1) return 18;
    args[0] = namespace_object;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_IS_FROZEN,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_truthy(read)) return 19;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_FREEZE,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_THROWN)
        return 20;
    if (kiln_c99_set_prototype(namespace_object, kiln_c99_undefined()) !=
        KILN_C99_CONTROL_ERROR) return 21;
    if ({symbol}_namespace_wrapper(&numeric_namespace, numeric_names, numeric_lengths, values, 2)
        != KILN_C99_OK) return 22;
    if (kiln_c99_object_of(numeric_namespace)->count != 3 ||
        !kiln_test_key_equal(kiln_c99_object_of(numeric_namespace)->properties[0].key, "10", 2) ||
        !kiln_test_key_equal(kiln_c99_object_of(numeric_namespace)->properties[1].key, "2", 1))
        return 23;
    if ({symbol}_namespace_wrapper(&read, reserved_names, reserved_lengths, values, 1) !=
        KILN_C99_INVALID_ARGUMENT) return 24;
    if ({symbol}_namespace_wrapper(&empty_namespace, NULL, NULL, NULL, 0) != KILN_C99_OK)
        return 25;
    args[0] = empty_namespace;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_FREEZE,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_strict_equal(read, empty_namespace)) return 26;
    if (kiln_c99_invoke_method(NULL, &read, KILN_C99_METHOD_OBJECT_IS_FROZEN,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        !kiln_c99_truthy(read)) return 27;
    return 0;
}}
"#,
        symbol = generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "namespace-wrapper");
    assert!(
        run.status.success(),
        "generated namespace wrapper failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_runtime_event_emitter_preserves_order_once_and_removal() {
    let module = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![kiln_ir::ExportBinding {
            exported: "EventEmitter".to_owned(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let generated = emit_runtime_module_with_logical_path(
        &module,
        ".kiln/builtins/events.builtin",
        "exports[0] = kiln_c99_method(KILN_C99_METHOD_NODE_EVENT_EMITTER);",
    )
    .expect("events runtime module emits");
    let wrapper = format!(
        r#"
static int calls = 0;
static int listener(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                    const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{
    (void)ctx; (void)upvalues; (void)receiver_ptr;
    if (argc != 1 || !KILN_VAL_IS_NUMBER(args[0])) return 91;
    double d; memcpy(&d, &args[0], sizeof(d));
    calls = calls * 10 + (int)d;
    *result = KILN_VAL_UNDEFINED;
    return KILN_C99_OK;
}}
static int child_constructor(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                             const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {{
    (void)ctx; (void)upvalues; (void)args; (void)argc;
    *result = receiver_ptr ? *receiver_ptr : KILN_VAL_UNDEFINED;
    return KILN_C99_OK;
}}
/* Headed closure names: the closure keeps the pointer. */
static const kiln_string listener_name = {{ 8, KILN_STRING_LATIN1, "listener" }};
static const kiln_string child_name = {{ 5, KILN_STRING_LATIN1, "Child" }};
int main(void) {{
    uint64_t realm;
    kiln_value exports[1];
    kiln_value result;
    kiln_value emitter;
    kiln_value callback;
    kiln_value child;
    kiln_value child_instance;
    kiln_value child_prototype;
    kiln_value parent_prototype;
    kiln_value inherited;
    kiln_value args[2];
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&callback, listener, &listener_name, NULL, 0, "<test>", 0, 1, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 1;
    if ({symbol}(NULL, &result, NULL, 0, exports, 1) != KILN_C99_OK) return 2;
    if (kiln_c99_construct(NULL, &emitter, exports[0], NULL, 0, 0,
                           &thrown) != KILN_C99_OK) return 3;
    if (emitter.tag != KILN_VALUE_OBJECT) return 31;
    {{ kiln_value keys;
       if (kiln_c99_own_keys(NULL, emitter, &keys, 0, &thrown) != KILN_C99_OK) return 32;
       if (kiln_c99_array_of(keys)->count != 0) return 33;
    }}
    if (kiln_c99_make_closure(&child, child_constructor, &child_name, NULL, 0, "<test>", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 34;
    if (kiln_c99_callable_prototype(child, &child_prototype) != KILN_C99_OK ||
        kiln_c99_callable_prototype(exports[0], &parent_prototype) != KILN_C99_OK ||
        kiln_c99_set_prototype(child_prototype, parent_prototype) != KILN_C99_OK) return 35;
    if (kiln_c99_construct(NULL, &child_instance, child, NULL, 0, 0,
                           &thrown) != KILN_C99_OK) return 36;
    if (kiln_c99_get(NULL, child_instance, "on", 2, &inherited, 0,
                     &thrown) != KILN_C99_OK ||
        inherited.tag != KILN_VALUE_METHOD ||
        inherited.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 37;
    if (kiln_c99_callable_prototype(child, &child_prototype) != KILN_C99_OK ||
        kiln_c99_callable_prototype(exports[0], &parent_prototype) != KILN_C99_OK ||
        kiln_c99_object_of(child_prototype)->prototype.bits != parent_prototype.bits) return 38;
    args[0] = kiln_c99_copy_text("tick", 4); args[1] = callback;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_ON, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK) return 4;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_ONCE, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK) return 5;
    args[1] = kiln_c99_number(7.0);
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_EMIT, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK || !kiln_c99_truthy(result)) return 6;
    args[1] = kiln_c99_number(8.0);
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_EMIT, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK || calls != 778) return 7;
    args[1] = callback;
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_REMOVE_LISTENER,
                               emitter, args, 2, 0, &thrown) != KILN_C99_OK) return 8;
    args[1] = kiln_c99_number(9.0);
    if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_NODE_EVENT_EMIT, emitter, args, 2,
                               0, &thrown) != KILN_C99_OK || kiln_c99_truthy(result)) return 9;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 39;
}}
"#,
        symbol = generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "runtime-event-emitter");
    assert!(
        run.status.success(),
        "runtime EventEmitter failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_server_response_inherits_writable_through_the_native_graph() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value constructor = kiln_c99_method(KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE);
    kiln_value prototype;
    kiln_value writable_prototype;
    kiln_value instance;
    kiln_value method;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_callable_prototype(constructor, &prototype) != KILN_C99_OK) return 1;
    if (kiln_c99_callable_prototype(kiln_c99_method(KILN_C99_METHOD_NODE_WRITABLE),
                                    &writable_prototype) != KILN_C99_OK) return 2;
    if (kiln_c99_object_of(prototype)->prototype.bits != writable_prototype.bits) return 3;
    if (kiln_c99_get(NULL, prototype, "writeHead", 9, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_RESPONSE_WRITE_HEAD) return 4;
    if (kiln_c99_get(NULL, prototype, "on", 2, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD || method.bits != KILN_C99_METHOD_NODE_EVENT_ON) return 5;
    if (kiln_c99_construct(NULL, &instance, constructor, NULL, 0, 0, &thrown) != KILN_C99_OK)
        return 6;
    if (kiln_c99_get(NULL, instance, "write", 5, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_NODE_RESPONSE_WRITE) return 7;
    return 0;
}
"#;
    let run = run_with_main(
        &generated,
        wrapper,
        "native-server-response-prototype-graph",
    );
    assert!(
        run.status.success(),
        "native ServerResponse prototype graph failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Every value kind that answers "yes" to `kiln_c99_is_callable` converts to text.
///
/// `Function.prototype.toString` reaches a value by two routes -- `String(f)` through the
/// text conversion and `f.toString()` through a member lookup -- and both end at
/// `kiln_c99_text`. `kiln_c99_is_callable` listed six tags and `kiln_c99_text` handled three,
/// so a bound function, a promise settler and a `require` all fell to `default:` and answered
/// `undefined`. Nothing failed: `String(f)` was right for a plain function, and no probe
/// called the method on any of the other three.
///
/// This is the pairing, not the symptom. Adding a callable tag without a text case is now a
/// test failure here rather than a wrong answer discovered later by whoever happens to call
/// `toString` on the one shape nobody wrote a probe for.
#[test]
fn callable_text_covers_every_callable_tag() {
    let runtime = HEADER;

    fn body<'a>(runtime: &'a str, signature: &str) -> &'a str {
        let start = runtime
            .split(signature)
            .nth(1)
            .unwrap_or_else(|| panic!("{signature} must exist"));
        // The C function ends at the first line that closes it in column zero.
        let end = start.find("\n}").expect("function must be closed");
        &start[..end]
    }

    let callable = body(
        runtime,
        "static int kiln_c99_is_callable(kiln_value value) {",
    );
    let text = body(
        runtime,
        "static kiln_value kiln_c99_text(kiln_value value) {",
    );

    let tags: Vec<&str> = callable
        .split("value.tag == ")
        .skip(1)
        .filter_map(|rest| {
            rest.split(|c: char| !c.is_ascii_uppercase() && c != '_')
                .next()
        })
        .filter(|tag| tag.starts_with("KILN_VALUE_"))
        .collect();
    // A guard that has quietly stopped reading the thing it checks is worse than none.
    assert!(
        tags.len() >= 5,
        "read only {tags:?} out of `kiln_c99_is_callable`; the parse has stopped working"
    );
    assert!(
        text.contains("case KILN_VALUE_FUNCTION:"),
        "the parse of `kiln_c99_text` has stopped working"
    );

    let missing: Vec<&&str> = tags
        .iter()
        .filter(|tag| !text.contains(&format!("case {tag}:")))
        .collect();
    assert!(
        missing.is_empty(),
        "`kiln_c99_is_callable` accepts {missing:?}, which `kiln_c99_text` has no case for. \
             Each would fall to `default:` and answer `undefined`, so `String(x)` and \
             `x.toString()` on that shape would report a value the program never wrote. A \
             callable with no source text of its own takes the NativeFunction form, which is what \
             the interpreter answers and what the specification allows for a built-in."
    );
}

#[test]
fn every_mutable_c99_static_has_an_explicit_owner() {
    let generated =
        emit(&program(vec![], vec![Instruction::Return], 0)).expect("empty program emits");
    // Declarations are gathered as *statements*, not lines. A mutable static split over two
    // lines -- by a long type, a `sizeof` expression, or a human -- used to be invisible here,
    // which is the one failure this guard exists to prevent: unregistered mutable state that
    // nobody is told about. A function definition is dropped when its brace arrives, and its
    // parameter list would exclude it below in any case.
    let mut statements: Vec<String> = Vec::new();
    let mut pending = String::new();
    for line in generated.source.lines() {
        let line = line.trim();
        if pending.is_empty() {
            if !line.starts_with("static ") {
                continue;
            }
            pending.push_str(line);
        } else {
            pending.push(' ');
            pending.push_str(line);
        }
        if pending.ends_with(';') && pending.matches('{').count() == pending.matches('}').count() {
            statements.push(std::mem::take(&mut pending));
        } else if pending
            .split('{')
            .next()
            .is_some_and(|before| before.contains('(') && pending.contains('{'))
        {
            pending.clear();
        }
    }
    let mut discovered = statements
        .iter()
        .filter_map(|line| {
            let line = line.trim();
            let declaration = line.split('=').next().unwrap_or(line);
            // An anonymous struct can contain domain-prefixed types and fields. Its
            // declarator follows the closing brace; for named types, the variable is
            // the last identifier before an optional array suffix.
            let declarator = declaration.rsplit('}').next().unwrap_or(declaration);
            let declarator = declarator.split('[').next().unwrap_or(declarator);
            let name = declarator
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .filter(|word| word.starts_with("kiln_c99_"))
                .next_back()?;
            let name_start = declaration.rfind(name)?;
            let before_name = &declaration[..name_start];
            let immutable = if before_name.contains('*') {
                // `*const` and `* const` declare the same thing, so the guard must not depend
                // on which one somebody wrote.
                before_name
                    .split_whitespace()
                    .collect::<String>()
                    .ends_with("*const")
            } else {
                before_name.starts_with("static const ")
            };
            if !line.starts_with("static ")
                || immutable
                || declaration.contains('(')
                || !line.contains("kiln_c99_")
                || !line.ends_with(';')
            {
                return None;
            }
            Some(name.to_owned())
        })
        .collect::<Vec<_>>();
    discovered.sort();
    discovered.dedup();
    let mut classified = C99_MUTABLE_STATE_OWNERS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    classified.sort();
    assert_eq!(
        discovered, classified,
        "mutable generated-C state must say which of the frame, a per-thread cache, the process or \
         a test fixture owns it: realm state is not a static (ADR 0070; `kiln_c99_realm_state`)"
    );
    for (name, owner) in C99_MUTABLE_STATE_OWNERS {
        if !owner.per_thread() {
            continue;
        }
        let declaration = statements
            .iter()
            .find(|statement| {
                statement.split('=').next().is_some_and(|head| {
                    head.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .any(|word| word == *name)
                })
            })
            .unwrap_or_else(|| panic!("`{name}` is declared"));
        assert!(
            declaration.contains("_Thread_local"),
            "`{name}` is owned by {owner:?}, which is per thread, but is declared `{declaration}`: \
             a plain static would be one copy that every realm thread reads and writes"
        );
    }
    assert!(
        !generated
            .source
            .contains("static kiln_value kiln_c99_globals;"),
        "globalThis identity must live in the active realm slot, not one module"
    );
    assert!(
        generated.source.contains("kiln_realm_value_slot("),
        "generated value operations must reach realm singleton identity through the typed slot ABI"
    );
}

#[test]
fn an_instruction_replaces_the_function_body_fallback_at_the_same_depth() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("diagnostic ownership probe emits");
    let wrapper = r#"
int main(void) {
    kiln_c99_diagnostic_site = "callee.js";
    kiln_c99_diagnostic_instruction = 0;
    kiln_c99_diagnostic_operation = "FunctionBody";
    kiln_c99_diagnostic_depth = 2;
    kiln_c99_diagnostic_function = "callee";
    kiln_c99_diagnostic_function_len = 6;
    kiln_c99_diagnostic_receiver_tag = KILN_VALUE_FUNCTION;
    kiln_c99_diagnostic_detail =
        "generated function body refused before an instruction owned the failure";
    kiln_c99_note_refusal("callee.js", 37, "CallMethod(listen)", 2);
    if (kiln_c99_diagnostic_instruction != 37) return 1;
    if (strcmp(kiln_c99_diagnostic_operation, "CallMethod(listen)") != 0) return 2;
    if (kiln_c99_diagnostic_function != NULL) return 3;
    if (kiln_c99_diagnostic_detail != NULL) return 4;
    if (kiln_c99_diagnostic_receiver_tag != 0) return 5;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "instruction-replaces-body-fallback");
    assert!(
        run.status.success(),
        "diagnostic ownership failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
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
fn refuses_to_make_a_realm_by_name_rather_than_answering_from_its_only_one() {
    // The backend's global object is one copy built in C. A program that reached the host's
    // realm hook would get an object whose intrinsics were secretly this realm's, and every
    // cross-realm identity question would be answered confidently and wrongly -- so the hook
    // is refused where it is named, not where its answer is used.
    for code in [
        vec![
            Instruction::RequireGlobalBinding(0),
            Instruction::Pop,
            Instruction::Return,
        ],
        vec![
            Instruction::LoadGlobalBindingOrUndefined(0),
            Instruction::Pop,
            Instruction::Return,
        ],
    ] {
        let refused = emit(&program(
            vec![Constant::String(HOST_CREATE_REALM.to_owned().into())],
            code,
            0,
        ));
        let Err(EmitError::Unsupported { feature }) = refused else {
            panic!("the backend must refuse {HOST_CREATE_REALM}, not compile it");
        };
        assert!(
            feature.contains(HOST_CREATE_REALM),
            "the refusal must name what it refused, got {feature}"
        );
        let refused_eval = emit(&program(
            vec![Constant::String(HOST_EVAL_SCRIPT.to_owned().into())],
            vec![
                Instruction::LoadGlobalBindingOrUndefined(0),
                Instruction::Pop,
                Instruction::Return,
            ],
            0,
        ));
        let Err(EmitError::Unsupported { feature }) = refused_eval else {
            panic!("the backend must refuse {HOST_EVAL_SCRIPT}, not compile it");
        };
        assert!(
            feature.contains(HOST_EVAL_SCRIPT),
            "the refusal must name what it refused, got {feature}"
        );
        // And each of the eight agent hooks, by its own name. A backend that quietly compiled
        // `$262AgentStart` into a no-op would answer every `Atomics.wait` "timed-out" while
        // the interpreter answered "ok", which is the divergence this refusal prevents.
        for hook in HOST_AGENT_HOOKS {
            let refused_agent = emit(&program(
                vec![Constant::String(hook.to_owned().into())],
                vec![
                    Instruction::LoadGlobalBindingOrUndefined(0),
                    Instruction::Pop,
                    Instruction::Return,
                ],
                0,
            ));
            let Err(EmitError::Unsupported { feature }) = refused_agent else {
                panic!("the backend must refuse {hook}, not compile it");
            };
            assert!(
                feature.contains(hook),
                "the refusal must name what it refused, got {feature}"
            );
        }
        // `ShadowRealm` is refused for a sharper reason than the two hooks above: it is a
        // language built-in a program may name without any host granting it, and this
        // backend has one global object. Compiling it would answer `new ShadowRealm()` with
        // a realm whose intrinsics were secretly this one's.
        let refused_shadow = emit(&program(
            vec![Constant::String(SHADOW_REALM.to_owned().into())],
            vec![
                Instruction::LoadGlobalBindingOrUndefined(0),
                Instruction::Pop,
                Instruction::Return,
            ],
            0,
        ));
        let Err(EmitError::Unsupported { feature }) = refused_shadow else {
            panic!("the backend must refuse {SHADOW_REALM}, not compile it");
        };
        assert!(
            feature.contains(SHADOW_REALM),
            "the refusal must name what it refused, got {feature}"
        );
        // The instruction that actually carries `ShadowRealm` in a compiled program. A name
        // this backend has no global for never reaches the lookup arms above: the compiler
        // emits the miss as `ThrowReference`, so refusing only the lookup let
        // `new ShadowRealm()` compile and answer `ReferenceError` at run time. That is a
        // claim about the *language* rather than about this backend, and the two engines
        // diverged on it.
        let refused_reference = emit(&program(
            vec![Constant::String(SHADOW_REALM.to_owned().into())],
            vec![Instruction::ThrowReference(0), Instruction::Return],
            0,
        ));
        let Err(EmitError::Unsupported { feature }) = refused_reference else {
            panic!("the backend must refuse a {SHADOW_REALM} reference, not compile it");
        };
        assert!(
            feature.contains(SHADOW_REALM),
            "the refusal must name what it refused, got {feature}"
        );
    }
    // An ordinary global of another name is untouched: this refuses one hook, not a class of
    // instruction the backend already supports.
    assert!(
        emit(&program(
            vec![Constant::String("Array".to_owned().into())],
            vec![
                Instruction::LoadGlobalBindingOrUndefined(0),
                Instruction::Pop,
                Instruction::Return,
            ],
            0,
        ))
        .is_ok()
    );
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
        .args([
            "-std=c99",
            "-pedantic",
            "-Werror=implicit-function-declaration",
            "-fsyntax-only",
            "-x",
            "c",
            "-",
        ])
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
fn executable_promise_with_resolvers_exposes_and_settles_one_deferred() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value method;
    kiln_value deferred;
    kiln_value promise;
    kiln_value resolve;
    kiln_value value = kiln_c99_number(42.0);
    kiln_value result;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE),
                     "withResolvers", 13, &method, 0, &thrown) != KILN_C99_OK) return 1;
    if (method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_PROMISE_WITH_RESOLVERS) return 2;
    if (kiln_c99_call(NULL, &deferred, method, kiln_c99_builtin(KILN_C99_BUILTIN_PROMISE),
                      NULL, 0, 0, &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_get(NULL, deferred, "promise", 7, &promise, 0, &thrown) != KILN_C99_OK ||
        promise.tag != KILN_VALUE_PROMISE) return 4;
    if (kiln_c99_get(NULL, deferred, "resolve", 7, &resolve, 0, &thrown) != KILN_C99_OK ||
        resolve.tag != KILN_VALUE_SETTLER) return 5;
    if (kiln_c99_call(NULL, &result, resolve, kiln_c99_undefined(), &value, 1, 0,
                      &thrown) != KILN_C99_OK) return 6;
    if (kiln_c99_promise_of(promise)->state != KILN_C99_FULFILLED ||
        kiln_c99_as_number(kiln_c99_promise_of(promise)->value) != 42.0) return 7;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "promise-with-resolvers");
    assert!(
        run.status.success(),
        "Promise.withResolvers failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_accessor_definition_bypasses_an_inherited_setter() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value parent;
    kiln_value child;
    kiln_value inherited = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
    kiln_value own = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_FORMAT);
    kiln_value descriptor;
    kiln_value read;
    kiln_value thrown = kiln_c99_undefined();
    int found = 0;
    if (kiln_c99_new_object(&parent) != KILN_C99_OK ||
        kiln_c99_new_object(&child) != KILN_C99_OK) return 1;
    if (kiln_c99_define_accessor(parent, "value", 5, kiln_c99_undefined(), inherited, 0) !=
        KILN_C99_OK) return 2;
    if (kiln_c99_set_prototype(child, parent) != KILN_C99_OK) return 3;
    if (kiln_c99_define_accessor(child, "value", 5, own, kiln_c99_undefined(), 1) !=
        KILN_C99_OK) return 4;
    if (kiln_c99_describe_property(NULL, child, "value", 5, &descriptor, &found, 0, &thrown) !=
            KILN_C99_OK || !found) return 5;
    if (kiln_c99_get(NULL, descriptor, "get", 3, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_METHOD || read.bits != own.bits) return 6;
    if (kiln_c99_get(NULL, descriptor, "set", 3, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_UNDEFINED) return 7;
    if (kiln_c99_get(NULL, descriptor, "enumerable", 10, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_BOOLEAN || read.bits != 1) return 8;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "accessor-definition-own-slot");
    assert!(
        run.status.success(),
        "accessor definition failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
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
        (Constant::String(String::new().into()), number(0.0), true),
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
fn the_reference_operators_emit_rather_than_answering_undefined() {
    for operator in [BinaryOp::In, BinaryOp::InstanceOf] {
        let result = emit(&program(
            vec![Constant::Null],
            vec![
                Instruction::Const(0),
                Instruction::Const(0),
                Instruction::Binary(operator),
                Instruction::Return,
            ],
            0,
        ));
        assert!(result.is_ok(), "{operator:?} should emit");
    }
}

#[test]
fn dictionary_cache_index_encoding_preserves_empty_and_extreme_indices() {
    let generated = emit(&program(
        vec![Constant::Undefined],
        vec![Instruction::Const(0), Instruction::Return],
        0,
    ))
    .expect("minimal program emits");
    let run = run_with_main(
        &generated,
        r#"
int main(void) {
    static kiln_c99_site site = {0};
    if (kiln_c99_site_decode_dictionary_index(site.own) != UINT32_MAX) return 1;
    if (kiln_c99_site_decode_dictionary_index(site.proto) != UINT32_MAX) return 2;
    if (kiln_c99_site_encode_dictionary_index(0) != 1) return 3;
    if (kiln_c99_site_encode_dictionary_index(1) != 2) return 4;
    if (kiln_c99_site_decode_dictionary_index(1) != 0) return 5;
    if (kiln_c99_site_decode_dictionary_index(2) != 1) return 6;
    if (kiln_c99_site_encode_dictionary_index((size_t)UINT32_MAX - 1) != UINT32_MAX) return 7;
    if (kiln_c99_site_decode_dictionary_index(UINT32_MAX) != UINT32_MAX - 1) return 8;
    if (kiln_c99_site_encode_dictionary_index((size_t)UINT32_MAX) != 0) return 9;
    return 0;
}
"#,
        "dictionary-cache-indices",
    );
    assert!(
        run.status.success(),
        "index encoding failed: {:?}",
        run.status
    );
}

/// An arrow has no call-time receiver. It snapshots the receiver of the frame that creates
/// it, even when a later plain call supplies `undefined` as `this`.
#[test]
fn executable_arrow_captures_its_defining_receiver() {
    let arrow = FunctionProto {
        inferred_name: None,
        prologue_end: 0,
        arrow: true,
        code: vec![Instruction::LoadThis, Instruction::Return],
        ..function(Vec::new())
    };
    let maker = FunctionProto {
        inferred_name: None,
        prologue_end: 0,
        code: vec![Instruction::MakeFunction(1), Instruction::Return],
        ..function(Vec::new())
    };
    let generated = emit(&Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::String("make".into())],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(0),
            Instruction::NewObject { count: 0 },
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(0),
            Instruction::SetProperty(0),
            Instruction::Pop,
            Instruction::LoadLocal(1),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(2),
            Instruction::Call { argc: 0 },
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::Return,
        ],
        local_count: 3,
        functions: vec![maker, arrow],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    })
    .expect("arrow receiver program emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits == 1 ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "arrow-receiver");
    assert!(
        run.status.success(),
        "an arrow observed its call-time receiver instead of its defining receiver (exit {:?})",
        run.status.code()
    );
}

/// Recursion is bounded rather than left to exhaust the C stack, which aborts the process
/// instead of reporting anything. It reports as a *thrown* `RangeError` rather than a
/// status, because that is what a real engine raises and what a program that recurses on its
/// input has to be able to catch.
#[test]
fn executable_runaway_recursion_reports_rather_than_crashing() {
    let forever = FunctionProto {
        inferred_name: None,
        prologue_end: 0,
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
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
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
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    })
    .expect("program emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; return {}(NULL, &result) == KILN_C99_THROWN ? 0 : 1; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "depth");
    assert!(
        run.status.success(),
        "unbounded recursion did not report a thrown RangeError (exit {:?})",
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

/// The explanation for the `KILN_C99_ABSENT` half of the `const` gate above, which can only
/// say *that* the count moved.
///
/// This says which file, which line and which enclosing function, and it checks the half the
/// gate cannot see: that the sites are in the functions `ABSENT_ANSWER_SITES` claims and in
/// the numbers it claims. A row is a promise that a membership predicate decides that site,
/// and a site in a function no row names is a place a read answers `undefined` with nobody
/// having said why.
#[test]
fn absent_answers_are_where_they_say() {
    let sources = crate::prelude_source_parts();

    let mut by_function: Vec<(String, usize)> = Vec::new();
    let mut by_hand = Vec::new();
    for (path, source) in sources {
        let lines: Vec<&str> = source.lines().collect();
        // A top-level definition starts in column zero; everything inside one is indented.
        let mut function = String::from("<file scope>");
        for (index, line) in lines.iter().enumerate() {
            if line.starts_with(|first: char| first.is_ascii_alphabetic())
                && let Some((head, _)) = line.split_once('(')
                && let Some(name) = head
                    .split(|byte: char| !(byte.is_alphanumeric() || byte == '_'))
                    .next_back()
            {
                function = name.to_owned();
            }
            if line.trim() == "*out = kiln_c99_undefined();"
                && lines
                    .get(index + 1)
                    .is_some_and(|next| next.trim() == "return KILN_C99_OK;")
            {
                by_hand.push(format!("{path}:{} in {function}", index + 1));
            }
            if line.contains("KILN_C99_ABSENT(") {
                match by_function.iter_mut().find(|(name, _)| *name == function) {
                    Some((_, count)) => *count += 1,
                    None => by_function.push((function.clone(), 1)),
                }
            }
        }
    }

    assert!(
        by_hand.is_empty(),
        "these prelude sites answer a read with `undefined` by hand, so nothing there says \
         why the name is absent rather than unimplemented -- which is the difference between \
         what the language answers and a silent wrong answer a feature test walks past. Use \
         KILN_C99_ABSENT with the predicate that proves absence:\n  {}",
        by_hand.join("\n  ")
    );

    for row in ABSENT_ANSWER_SITES {
        let found = by_function
            .iter()
            .find(|(name, _)| name == row.function)
            .map(|(_, count)| *count)
            .unwrap_or(0);
        assert_eq!(
            found, row.sites,
            "ABSENT_ANSWER_SITES says {} answers `undefined` at {} sites, proved absent by {}, \
             and the prelude holds {}",
            row.function, row.sites, row.proved_by, found
        );
    }
    for (function, count) in &by_function {
        assert!(
            ABSENT_ANSWER_SITES
                .iter()
                .any(|row| row.function == function.as_str()),
            "{function} answers `undefined` at {count} sites and no ABSENT_ANSWER_SITES row \
             says what proves those names absent"
        );
    }
}

#[test]
fn executable_dynamic_function_construction_and_evaluation() {
    let generated =
        emit(&program(vec![], vec![Instruction::Return], 0)).expect("empty program emits");
    let wrapper = format!(
        r#"
int main(void) {{
    uint64_t realm = 0;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 99;
    kiln_test_realm_active = 1;

    kiln_value dummy;
    int mod_status = {}(NULL, &dummy);
    if (mod_status != KILN_C99_OK) return 100;

    kiln_value thrown = kiln_c99_undefined();
    kiln_value result = kiln_c99_undefined();

    // 1. Dynamic function with no arguments returning constant:
    kiln_value args1[1] = {{ kiln_c99_copy_text("return 42;", 10) }};
    int status = kiln_c99_dynamic_new_function(NULL, &result, args1, 1, 0, &thrown);
    if (status != KILN_C99_OK || result.tag != KILN_VALUE_FUNCTION) return 1;

    kiln_value call_res = kiln_c99_undefined();
    status = kiln_c99_call(NULL, &call_res, result, kiln_c99_undefined(), NULL, 0, 0, &thrown);
    if (status != KILN_C99_OK) return 2;
    if (call_res.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(call_res) != 42.0) return 3;

    // 2. Dynamic function with two parameters:
    kiln_value args2[3] = {{
        kiln_c99_copy_text("a", 1),
        kiln_c99_copy_text("b", 1),
        kiln_c99_copy_text("return a + b;", 13)
    }};
    status = kiln_c99_dynamic_new_function(NULL, &result, args2, 3, 0, &thrown);
    if (status != KILN_C99_OK) return 4;

    kiln_value call_args2[2] = {{ kiln_c99_number(100.0), kiln_c99_number(23.0) }};
    status = kiln_c99_call(NULL, &call_res, result, kiln_c99_undefined(), call_args2, 2, 0, &thrown);
    if (status != KILN_C99_OK || kiln_c99_as_number(call_res) != 123.0) return 5;

    // 3. Dynamic function mutating an object on the native heap:
    kiln_value my_obj;
    status = kiln_c99_new_object(&my_obj);
    if (status != KILN_C99_OK) return 6;
    status = kiln_c99_put(NULL, my_obj, "count", 5, kiln_c99_number(10.0), 0, &thrown);
    if (status != KILN_C99_OK) return 7;

    kiln_value args3[2] = {{
        kiln_c99_copy_text("obj", 3),
        kiln_c99_copy_text("obj.count = obj.count + 5; return obj.count;", 44)
    }};
    status = kiln_c99_dynamic_new_function(NULL, &result, args3, 2, 0, &thrown);
    if (status != KILN_C99_OK) return 8;

    kiln_value call_args3[1] = {{ my_obj }};
    status = kiln_c99_call(NULL, &call_res, result, kiln_c99_undefined(), call_args3, 1, 0, &thrown);
    if (status != KILN_C99_OK || kiln_c99_as_number(call_res) != 15.0) return 9;

    kiln_value read_back;
    status = kiln_c99_get(NULL, my_obj, "count", 5, &read_back, 0, &thrown);
    if (status != KILN_C99_OK || kiln_c99_as_number(read_back) != 15.0) return 10;

    // 4. Generated schema functions bind aliased properties from the runtime object.
    kiln_value runtime_obj;
    status = kiln_c99_new_object(&runtime_obj);
    if (status != KILN_C99_OK) return 11;
    status = kiln_c99_put(NULL, runtime_obj, "invalid", 7, kiln_c99_number(17.0), 0, &thrown);
    if (status != KILN_C99_OK) return 12;
    status = kiln_c99_put(NULL, runtime_obj, "getCheckIssues", 14,
                          kiln_c99_number(25.0), 0, &thrown);
    if (status != KILN_C99_OK) return 13;
    const char generated_body[] =
        "const {{invalid:I,getCheckIssues:K}}=R; return I + K;";
    kiln_value generated_args[2] = {{
        kiln_c99_copy_text("R", 1),
        kiln_c99_copy_text(generated_body, sizeof generated_body - 1)
    }};
    status = kiln_c99_dynamic_new_function(NULL, &result, generated_args, 2, 0, &thrown);
    if (status != KILN_C99_OK) return 14;
    kiln_value call_args4[1] = {{ runtime_obj }};
    status = kiln_c99_call(NULL, &call_res, result, kiln_c99_undefined(),
                           call_args4, 1, 0, &thrown);
    if (status != KILN_C99_OK || call_res.tag != KILN_VALUE_NUMBER ||
        kiln_c99_as_number(call_res) != 42.0) return 15;

    // 5. Object-pattern lexical bindings enter their TDZ before the initializer runs.
    kiln_value globals;
    status = kiln_c99_global_object(&globals);
    if (status != KILN_C99_OK) return 16;
    status = kiln_c99_put(NULL, globals, "x", 1, kiln_c99_number(99.0), 0, &thrown);
    if (status != KILN_C99_OK) return 17;
    const char tdz_body[] = "const {{x}}=x; return x;";
    kiln_value tdz_args[1] = {{ kiln_c99_copy_text(tdz_body, sizeof tdz_body - 1) }};
    status = kiln_c99_dynamic_new_function(NULL, &result, tdz_args, 1, 0, &thrown);
    if (status != KILN_C99_OK) return 18;
    thrown = kiln_c99_undefined();
    status = kiln_c99_call(NULL, &call_res, result, kiln_c99_undefined(), NULL, 0, 0, &thrown);
    if (status == KILN_C99_OK || thrown.tag == KILN_VALUE_UNDEFINED) return 19;

    // 6. Even an empty object pattern performs RequireObjectCoercible.
    const char null_body[] = "const {{}}=null; return 1;";
    kiln_value null_args[1] = {{ kiln_c99_copy_text(null_body, sizeof null_body - 1) }};
    status = kiln_c99_dynamic_new_function(NULL, &result, null_args, 1, 0, &thrown);
    if (status != KILN_C99_OK) return 20;
    thrown = kiln_c99_undefined();
    status = kiln_c99_call(NULL, &call_res, result, kiln_c99_undefined(), NULL, 0, 0, &thrown);
    if (status == KILN_C99_OK || thrown.tag == KILN_VALUE_UNDEFINED) return 21;

    // 7. Duplicate bound names fail before the initializer can run.
    const char duplicate_body[] = "const {{x,y:x}}=({{}}); return x;";
    kiln_value duplicate_args[1] = {{ kiln_c99_copy_text(duplicate_body, sizeof duplicate_body - 1) }};
    thrown = kiln_c99_undefined();
    status = kiln_c99_dynamic_new_function(NULL, &result, duplicate_args, 1, 0, &thrown);
    // Function construction validates its entire body before returning a callable.
    if (status != KILN_C99_THROWN || thrown.tag == KILN_VALUE_UNDEFINED) return 22;

    // 8. Var patterns may bind the same identifier more than once; the last property wins.
    const char var_alias_body[] = "var {{x:a,y:a}}={{x:1,y:2}}; return a;";
    kiln_value var_alias_args[1] = {{
        kiln_c99_copy_text(var_alias_body, sizeof var_alias_body - 1)
    }};
    thrown = kiln_c99_undefined();
    status = kiln_c99_dynamic_new_function(NULL, &result, var_alias_args, 1, 0, &thrown);
    if (status != KILN_C99_OK) return 24;
    status = kiln_c99_call(NULL, &call_res, result, kiln_c99_undefined(), NULL, 0, 0, &thrown);
    if (status != KILN_C99_OK || call_res.tag != KILN_VALUE_NUMBER ||
        kiln_c99_as_number(call_res) != 2.0) return 25;

    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 101;
}}
"#,
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "dynamic-function");
    assert!(
        run.status.success(),
        "dynamic function execution failed with exit code {:?}:\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn string_code_generation_policy_guards_eval_and_function_only_for_strings() {
    let generated =
        emit(&program(vec![], vec![Instruction::Return], 0)).expect("empty program emits");
    let wrapper = format!(
        r#"
extern int kiln_test_string_code_generation_allowed;
static int conversions = 0;
static int codegen_to_string(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                             const kiln_val *receiver, const kiln_val *args, size_t argc) {{
    (void)ctx; (void)upvalues; (void)receiver; (void)args; (void)argc;
    conversions++;
    *result = kiln_val_from_boundary(kiln_c99_copy_text("return 1;", 9));
    return KILN_C99_OK;
}}
static const kiln_string codegen_to_string_name = {{ 12, KILN_STRING_LATIN1, "toStringBody" }};
int main(void) {{
    uint64_t realm = 0;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 99;
    kiln_test_realm_active = 1;
    kiln_value dummy, out = kiln_c99_undefined(), thrown = kiln_c99_undefined();
    if ({}(NULL, &dummy) != KILN_C99_OK) return 100;
    kiln_test_string_code_generation_allowed = 0;

    kiln_value source = kiln_c99_copy_text("(", 1);
    if (kiln_c99_dynamic_indirect_eval(NULL, 1, &source, &out, 0, &thrown) != KILN_C99_THROWN)
        return 1;
    kiln_value kind = kiln_c99_undefined();
    if (kiln_c99_get(NULL, thrown, "@@error", 7, &kind, 0, &thrown) != KILN_C99_OK ||
        kind.tag != KILN_VALUE_NUMBER ||
        kiln_c99_as_number(kind) != (double)KILN_C99_BUILTIN_EVAL_ERROR) return 2;
    kiln_value message = kiln_c99_undefined();
    if (kiln_c99_get(NULL, thrown, "message", 7, &message, 0, &thrown) != KILN_C99_OK ||
        message.tag != KILN_VALUE_STRING ||
        !kiln_test_units_contain(message, "Code generation from strings is disabled")) return 20;

    thrown = kiln_c99_undefined();
    source = kiln_c99_copy_text("1+1", 3);
    if (kiln_c99_dynamic_eval_expression(NULL, source, NULL, NULL, NULL, NULL, 0, 0,
                                         &out, 0, &thrown) != KILN_C99_THROWN) return 6;
    kind = kiln_c99_undefined();
    if (kiln_c99_get(NULL, thrown, "@@error", 7, &kind, 0, &thrown) != KILN_C99_OK ||
        kind.tag != KILN_VALUE_NUMBER ||
        kiln_c99_as_number(kind) != (double)KILN_C99_BUILTIN_EVAL_ERROR) return 7;

    thrown = kiln_c99_undefined();
    kiln_value function_args[1] = {{ kiln_c99_copy_text("return 1", 8) }};
    if (kiln_c99_dynamic_new_function(NULL, &out, function_args, 1, 0, &thrown) !=
        KILN_C99_THROWN) return 3;
    kind = kiln_c99_undefined();
    if (kiln_c99_get(NULL, thrown, "@@error", 7, &kind, 0, &thrown) != KILN_C99_OK ||
        kind.tag != KILN_VALUE_NUMBER ||
        kiln_c99_as_number(kind) != (double)KILN_C99_BUILTIN_EVAL_ERROR) return 4;

    kiln_value coercible, coercer;
    if (kiln_c99_new_object(&coercible) != KILN_C99_OK ||
        kiln_c99_make_closure(&coercer, codegen_to_string, &codegen_to_string_name,
                              NULL, 0, "<test>", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK ||
        kiln_c99_put(NULL, coercible, "toString", 8, coercer, 0, &thrown) != KILN_C99_OK)
        return 8;
    thrown = kiln_c99_undefined();
    if (kiln_c99_dynamic_new_function(NULL, &out, &coercible, 1, 0, &thrown) !=
        KILN_C99_THROWN || conversions != 1) return 9;

    kiln_value non_string = kiln_c99_number(17.0);
    thrown = kiln_c99_undefined();
    if (kiln_c99_dynamic_indirect_eval(NULL, 1, &non_string, &out, 0, &thrown) != KILN_C99_OK ||
        out.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(out) != 17.0) return 5;

    // This is the runtime source-module evaluator, not eval of a user string. Linked module
    // execution stays available when string compilation is disabled.
    kiln_value module_source = kiln_c99_copy_text("export const value = 1;", 23);
    kiln_value module_url = kiln_c99_copy_text("generated.js", 12);
    thrown = kiln_c99_undefined();
    if (kiln_c99_dynamic_eval_module(NULL, module_source, module_url, &out, 0, &thrown) !=
        KILN_C99_OK || out.tag != KILN_VALUE_OBJECT) return 10;

    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 101;
}}
"#,
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "string-code-generation-policy");
    assert!(
        run.status.success(),
        "string code generation policy failed with exit code {:?}:\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_native_alu_and_bitwise_operations_execute_correctly() {
    let constants = vec![
        Constant::String("imul".into()),             // 0: "imul"
        Constant::Number(3.0f64.to_bits()),          // 1: 3
        Constant::Number(4.0f64.to_bits()),          // 2: 4
        Constant::Number(5.0f64.to_bits()),          // 3: 5
        Constant::Number(2.0f64.to_bits()),          // 4: 2
        Constant::Number(1.0f64.to_bits()),          // 5: 1
        Constant::Number((-16.0f64).to_bits()),      // 6: -16
        Constant::Number(1073741947.0f64.to_bits()), // 7: expected sum
    ];

    let code = vec![
        // local 0 = Math.imul(3, 4) (= 12), local 1 = 5
        Instruction::LoadIntrinsicBuiltin(kiln_ir::Builtin::Math),
        Instruction::Const(1),
        Instruction::Const(2),
        Instruction::CallMethod { name: 0, argc: 2 },
        Instruction::StoreLocal(0),
        Instruction::Const(3),
        Instruction::StoreLocal(1),
        // (a & b)
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::BitwiseAnd),
        // + (a | b)
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::BitwiseOr),
        Instruction::Binary(BinaryOp::Add),
        // + (a ^ b)
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::BitwiseXor),
        Instruction::Binary(BinaryOp::Add),
        // + (a << 2)
        Instruction::LoadLocal(0),
        Instruction::Const(4),
        Instruction::Binary(BinaryOp::ShiftLeft),
        Instruction::Binary(BinaryOp::Add),
        // + (a >> 1)
        Instruction::LoadLocal(0),
        Instruction::Const(5),
        Instruction::Binary(BinaryOp::ShiftRight),
        Instruction::Binary(BinaryOp::Add),
        // + ((-16) >>> 2)
        Instruction::Const(6),
        Instruction::Const(4),
        Instruction::Binary(BinaryOp::UnsignedShiftRight),
        Instruction::Binary(BinaryOp::Add),
        // + (a * b)
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::Multiply),
        Instruction::Binary(BinaryOp::Add),
        // + (~a)
        Instruction::LoadLocal(0),
        Instruction::Unary(UnaryOp::BitwiseNot),
        Instruction::Binary(BinaryOp::Add),
        // == 1073741947
        Instruction::Const(7),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants,
        code,
        local_count: 2,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let generated = emit(&program).expect("program emits");
    assert!(
        generated.source.contains("__builtin_mul_overflow"),
        "expected __builtin_mul_overflow in generated source:\n{}",
        generated.source
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "alu-and-bitwise-optimizations");
    assert!(
        run.status.success(),
        "alu and bitwise optimizations execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// A versioned body's generic clone reads a speculated value boxed, and the fast clone keeps
/// that box (parallel-builtins issue 18).
///
/// ```js
/// function f(list, needle) {
///   let kept = "";
///   kept += list.slice(0, 1);                              // a string: the fast clone exits
///   return list.indexOf("x", 1 + needle.length) + kept.length;
/// }
/// f("axbxcx", "ab") // 3 + 1
/// ```
///
/// `1 + needle.length` is a number only by speculation. The fast clone exits at the `slice`
/// (its result feeds `+=`, so it was speculated a number), and the generic clone computes the
/// sum boxed -- `needle.length` could be anything there. The `indexOf` argument read the sum's
/// double twin, which the generic clone never writes, and searched from 0: this answered 2, and
/// `headerListRemove`'s loop in `web.ts` never ended.
#[test]
fn executable_generic_clone_reads_a_speculated_sum_boxed_after_an_exit() {
    let mut callee = function(vec![
        Instruction::Const(0), // ""
        Instruction::StoreLocal(2),
        Instruction::LoadLocal(2),
        Instruction::LoadLocal(0),
        Instruction::Const(1),                        // 0
        Instruction::Const(2),                        // 1
        Instruction::CallMethod { name: 3, argc: 2 }, // list.slice(0, 1)
        Instruction::Binary(BinaryOp::Add),
        Instruction::StoreLocal(2),
        Instruction::LoadLocal(0),
        Instruction::Const(5), // "x"
        Instruction::Const(2), // 1
        Instruction::LoadLocal(1),
        Instruction::GetProperty(6), // needle.length
        Instruction::Binary(BinaryOp::Add),
        Instruction::CallMethod { name: 4, argc: 2 }, // list.indexOf("x", 1 + needle.length)
        Instruction::LoadLocal(2),
        Instruction::GetProperty(6), // kept.length
        Instruction::Binary(BinaryOp::Add),
        Instruction::Return,
    ]);
    callee.name = Some("f".into());
    callee.parameter_count = 2;
    callee.declared_length = 2;
    callee.local_count = 3;
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::String("".into()),        // 0
            Constant::Number(0.0f64.to_bits()), // 1
            Constant::Number(1.0f64.to_bits()), // 2
            Constant::String("slice".into()),   // 3
            Constant::String("indexOf".into()), // 4
            Constant::String("x".into()),       // 5
            Constant::String("length".into()),  // 6
            Constant::String("axbxcx".into()),  // 7
            Constant::String("ab".into()),      // 8
        ],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Const(7),
            Instruction::Const(8),
            Instruction::Call { argc: 2 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![callee],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit(&program).expect("program emits");
    assert!(
        generated
            .source
            .contains("/* Clone A: guarded fast path */"),
        "the callee must be versioned for this test to mean anything"
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; if (result.tag != KILN_VALUE_NUMBER) return 3; double d; memcpy(&d, &result.bits, sizeof d); return d == 4.0 ? 0 : (int)(10.0 + d); }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "speculated-sum-after-exit");
    assert!(
        run.status.success(),
        "the generic clone read a twin it never wrote ({:?}; 12 is indexOf from 0):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// The other half of the same contract: a speculated value the fast clone computed *before* the
/// exit, and never boxed because nothing in the fast clone read it boxed, reaches the generic
/// clone boxed -- the exit writes it from its twin on the way out.
///
/// ```js
/// function g(o, s) {
///   const n = o.a * 4;           // speculated; the fast clone reads it only as a twin
///   let k = "";
///   k += s.slice(0, 1);          // exits
///   return n * 2 + k.length;     // the generic clone reads `n` boxed
/// }
/// g({ a: 1 }, "axbx") // 8 + 1
/// ```
#[test]
fn executable_fast_clone_exit_boxes_a_speculated_value_it_never_boxed() {
    let mut callee = function(vec![
        Instruction::LoadLocal(0),
        Instruction::GetProperty(6), // o.a
        Instruction::Const(9),       // 4
        Instruction::Binary(BinaryOp::Multiply),
        Instruction::StoreLocal(2), // n
        Instruction::Const(0),      // ""
        Instruction::StoreLocal(3),
        Instruction::LoadLocal(3),
        Instruction::LoadLocal(1),
        Instruction::Const(1),                        // 0
        Instruction::Const(2),                        // 1
        Instruction::CallMethod { name: 3, argc: 2 }, // s.slice(0, 1)
        Instruction::Binary(BinaryOp::Add),
        Instruction::StoreLocal(3),
        Instruction::LoadLocal(2),
        Instruction::Const(10), // 2
        Instruction::Binary(BinaryOp::Multiply),
        Instruction::LoadLocal(3),
        Instruction::GetProperty(7), // k.length
        Instruction::Binary(BinaryOp::Add),
        Instruction::Return,
    ]);
    callee.name = Some("g".into());
    callee.parameter_count = 2;
    callee.declared_length = 2;
    callee.local_count = 4;
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::String("".into()),        // 0
            Constant::Number(0.0f64.to_bits()), // 1
            Constant::Number(1.0f64.to_bits()), // 2
            Constant::String("slice".into()),   // 3
            Constant::String("indexOf".into()), // 4
            Constant::String("x".into()),       // 5
            Constant::String("a".into()),       // 6
            Constant::String("length".into()),  // 7
            Constant::String("axbx".into()),    // 8
            Constant::Number(4.0f64.to_bits()), // 9
            Constant::Number(2.0f64.to_bits()), // 10
        ],
        code: vec![
            Instruction::NewObject { count: 0 },
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Const(2), // 1
            Instruction::SetProperty(6),
            Instruction::Pop,
            Instruction::MakeFunction(0),
            Instruction::LoadLocal(0),
            Instruction::Const(8),
            Instruction::Call { argc: 2 },
            Instruction::Return,
        ],
        local_count: 1,
        functions: vec![callee],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit(&program).expect("program emits");
    assert!(
        generated.source.contains("kiln_fast_exit_"),
        "the fast clone's exits must box `n` on the way out"
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; if (result.tag != KILN_VALUE_NUMBER) return 3; double d; memcpy(&d, &result.bits, sizeof d); return d == 9.0 ? 0 : 4; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "speculated-value-before-exit");
    assert!(
        run.status.success(),
        "the generic clone read a box the fast clone never wrote ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// Issue 27: an arithmetic condition (`i % 2 ? a : b`) must test a value the emitter wrote.
/// A proven numeric condition can use its integer or double twin directly, including zero and
/// NaN's false answers. The generic path still tests the box for values without that proof.
#[test]
fn executable_branch_on_a_number_remainder_reads_a_live_twin() {
    let mut callee = function(vec![
        Instruction::LoadLocal(0),
        Instruction::Const(1), // 2
        Instruction::Binary(BinaryOp::Remainder),
        Instruction::JumpIfFalse(6),
        Instruction::Const(2), // 7
        Instruction::Return,
        Instruction::Const(3), // 11
        Instruction::Return,
    ]);
    callee.name = Some("f".into());
    callee.parameter_count = 1;
    callee.declared_length = 1;
    callee.local_count = 1;
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(0.0f64.to_bits()),   // 0
            Constant::Number(2.0f64.to_bits()),   // 1
            Constant::Number(7.0f64.to_bits()),   // 2
            Constant::Number(11.0f64.to_bits()),  // 3
            Constant::Number(100.0f64.to_bits()), // 4
            Constant::Number(1.0f64.to_bits()),   // 5
        ],
        code: vec![
            // f(0) * 100 + f(1): 11 * 100 + 7
            Instruction::MakeFunction(0),
            Instruction::Const(0),
            Instruction::Call { argc: 1 },
            Instruction::Const(4),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::MakeFunction(0),
            Instruction::Const(5),
            Instruction::Call { argc: 1 },
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![callee],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit(&program).expect("program emits");
    assert!(
        generated
            .source
            .contains("/* Clone A: guarded fast path */"),
        "the callee must be versioned for this test to mean anything"
    );
    let fast = generated
        .source
        .split("/* Clone A: guarded fast path */")
        .nth(1)
        .and_then(|source| source.split("/* Clone B: generic fallback path */").next())
        .expect("both clones emit");
    assert!(
        !fast.contains("kiln_c99_val_truthy(v2)"),
        "the proven numeric condition must test its twin without boxing"
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; if (result.tag != KILN_VALUE_NUMBER) return 3; double d; memcpy(&d, &result.bits, sizeof d); return d == 1107.0 ? 0 : 4; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "branch-on-remainder");
    assert!(
        run.status.success(),
        "the branch on `i % 2` read a value the fast clone never wrote ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

/// The same contract for `+x`: a unary plus of a proven number copies `x`'s box into its own,
/// so when the result is read boxed, `x` is too. It was treated as twin-only, and `+(a * 2)`
/// copied an unwritten box.
#[test]
fn executable_unary_plus_of_a_product_copies_a_written_box() {
    let mut callee = function(vec![
        Instruction::LoadLocal(0),
        Instruction::Const(1), // 2
        Instruction::Binary(BinaryOp::Multiply),
        Instruction::Unary(UnaryOp::Plus),
        Instruction::Return,
    ]);
    callee.name = Some("g".into());
    callee.parameter_count = 1;
    callee.declared_length = 1;
    callee.local_count = 1;
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(2.5f64.to_bits()), // 0
            Constant::Number(2.0f64.to_bits()), // 1
            Constant::Number(0.0f64.to_bits()), // 2
        ],
        code: vec![
            // g([2.5][0]): an argument read from an array is not proven a number, so the call
            // takes the boxed entry and its guarded fast clone rather than the typed entry.
            Instruction::MakeFunction(0),
            Instruction::Const(0),
            Instruction::NewArray { count: 1 },
            Instruction::Const(2),
            Instruction::GetComputed,
            Instruction::Call { argc: 1 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![callee],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit(&program).expect("program emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; if (result.tag != KILN_VALUE_NUMBER) return 3; double d; memcpy(&d, &result.bits, sizeof d); return d == 5.0 ? 0 : 4; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "unary-plus-of-product");
    assert!(
        run.status.success(),
        "`+(a * 2)` copied a box the product never wrote ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_length_specialization() {
    let constants = vec![
        Constant::String("length".into()),   // 0: "length"
        Constant::Number(10.0f64.to_bits()), // 1: 10
        Constant::Number(20.0f64.to_bits()), // 2: 20
        Constant::Number(30.0f64.to_bits()), // 3: 30
        Constant::Number(3.0f64.to_bits()),  // 4: expected array length
        Constant::String("hello".into()),    // 5: "hello"
        Constant::Number(5.0f64.to_bits()),  // 6: expected string length
    ];

    let code = vec![
        // arr = [10, 20, 30]
        Instruction::Const(1),
        Instruction::Const(2),
        Instruction::Const(3),
        Instruction::NewArray { count: 3 },
        Instruction::StoreLocal(0),
        // len1 = arr.length
        Instruction::LoadLocal(0),
        Instruction::GetProperty(0),
        Instruction::StoreLocal(1),
        // check len1 === 3
        Instruction::LoadLocal(1),
        Instruction::Const(4),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(2),
        // str = "hello"
        Instruction::Const(5),
        Instruction::StoreLocal(3),
        // len2 = str.length
        Instruction::LoadLocal(3),
        Instruction::GetProperty(0),
        Instruction::StoreLocal(4),
        // check len2 === 5
        Instruction::LoadLocal(4),
        Instruction::Const(6),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(5),
        // result = check1 && check2
        Instruction::LoadLocal(2),
        Instruction::LoadLocal(5),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants,
        code,
        local_count: 6,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let generated = emit(&program).expect("length program emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "length-specialization");
    assert!(
        run.status.success(),
        "length specialization execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_namespace_constructor_records_candidate_exports() {
    let module = ModuleProgram {
        program: program(vec![], vec![Instruction::Return], 0),
        ..Default::default()
    };
    let generated = emit_module_with_logical_path(&module, "src/namespace-ledger.js")
        .expect("namespace module emits");
    let wrapper = r#"
static int namespace_ledger_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_value root, namespace_object;
    kiln_c99_candidate_group *group;
    kiln_nursery_chunk **batch = NULL;
    size_t count = 0;
    static const char *names[] = {"value"};
    static const size_t lengths[] = {5};
    (void)context;
    heap->fallback = 0;
    heap->enabled = 0;
    if (!kiln_c99_actor_candidate_begin(heap) ||
        kiln_c99_new_object(&root) != KILN_C99_OK) return 1;
    group = heap->actor_candidate_group;
    if (!kiln_c99_actor_candidate_end(heap, 1)) return 2;
    kiln_c99_actor_candidate_seal(heap, root);
    if (!kiln_c99_candidate_group_snapshot_owner(heap, group, &batch, &count)) return 3;
    if (MODULE_namespace_wrapper(&namespace_object, names, lengths, &root, 1) !=
            KILN_C99_OK || group->incoming_owners.count != 1 ||
        kiln_c99_graph_scan_incoming_heap_checked(heap, group, batch, count, 1) !=
            KILN_C99_GRAPH_OUTSIDE_EDGE_ALIAS) return 4;
    /* The fixture releases the snapshot's edge directly; public namespaces are immutable. */
    kiln_object *owner = kiln_c99_object_of(namespace_object);
    kiln_c99_heap_store_value(owner, &owner->properties[0].value, kiln_c99_undefined());
    if (kiln_c99_graph_scan_incoming_heap_checked(heap, group, batch, count, 1) !=
            KILN_C99_GRAPH_ROOTS_UNENUMERATED) return 5;
    free(batch);
    return 0;
}
int main(void) {
    uint64_t realm;
    int status;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 6;
    status = kiln_realm_enter(realm, namespace_ledger_entry, NULL);
    if (kiln_realm_destroy(realm) != KILN_C99_OK) return 7;
    return status;
}
"#
    .replace(
        "MODULE_namespace_wrapper",
        &format!("{}_namespace_wrapper", generated.symbol),
    );
    let run = run_with_main(&generated, &wrapper, "namespace-constructor-ledger");
    assert!(
        run.status.success(),
        "namespace ledger failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(!String::from_utf8_lossy(&run.stderr).contains("candidate ledger disagrees"));
}

#[test]
fn requested_minor_collection_keeps_active_and_pending_candidate_provenance() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int keep_candidate_root(kiln_gc_heap *heap, kiln_value candidate) {
    kiln_value registry;
    if (kiln_c99_new_collection(&registry, 0, 0) != KILN_C99_OK) return 0;
    heap->realm.isolated_graphs = registry;
    return kiln_c99_collection_put(registry, candidate, candidate) == KILN_C99_OK;
}

static int candidate_minor_entry(void *context) {
    kiln_gc_heap *heap = kiln_c99_gc_heap_uncached();
    kiln_value candidate = kiln_c99_undefined();
    kiln_c99_candidate_group *group;
    kiln_gc_block *object_block;
    kiln_nursery_chunk *object_chunk;
    uint64_t id;
    uint64_t collections;
    uint64_t audited_minors;
    (void)context;
    if (heap == NULL) return 1;
    heap->fallback = 0;
    heap->enabled = 1;
    heap->force_pending = 0;
    heap->minor_audit_enabled = 1;
    heap->stress_every = 1; /* Force the active-candidate collection past its normal deferral. */
    if (!kiln_c99_actor_candidate_begin(heap) ||
        kiln_c99_new_object(&candidate) != KILN_C99_OK ||
        !keep_candidate_root(heap, candidate)) return 2;
    id = heap->actor_candidate_id;
    group = heap->actor_candidate_group;
    collections = heap->collections;
    audited_minors = heap->minor_audit_collections;
    kiln_c99_gc_collect_minor_mid_turn(heap);
    object_block = kiln_c99_gc_lookup(heap, (void *)(uintptr_t)candidate.bits);
    object_chunk = object_block != NULL && (object_block->kind & KILN_GC_FLAG_NURSERY) != 0
        ? kiln_c99_nursery_chunk_of(heap, object_block) : NULL;
    if (heap->collections != collections + 1 ||
        heap->minor_audit_collections != audited_minors ||
        heap->actor_candidate_id != id || heap->actor_candidate_group != group ||
        heap->candidate_ledger_groups != 1 || object_chunk == NULL ||
        object_chunk->candidate_id != id || object_chunk->candidate_state != 3 ||
        !kiln_c99_candidate_group_contains(group, object_chunk) || object_block == NULL)
        return 3;
    if (!kiln_c99_actor_candidate_end(heap, 0)) return 4;

    heap->stress_every = 0;
    heap->realm.isolated_graphs = kiln_c99_undefined();
    if (!kiln_c99_actor_candidate_begin(heap) ||
        kiln_c99_new_object(&candidate) != KILN_C99_OK ||
        !keep_candidate_root(heap, candidate) || !kiln_c99_actor_candidate_end(heap, 1)) return 5;
    id = heap->actor_candidate_pending_id;
    group = heap->actor_candidate_group;
    collections = heap->collections;
    audited_minors = heap->minor_audit_collections;
    kiln_c99_gc_collect_minor_mid_turn(heap);
    object_block = kiln_c99_gc_lookup(heap, (void *)(uintptr_t)candidate.bits);
    object_chunk = object_block != NULL && (object_block->kind & KILN_GC_FLAG_NURSERY) != 0
        ? kiln_c99_nursery_chunk_of(heap, object_block) : NULL;
    if (heap->collections != collections + 1 ||
        heap->minor_audit_collections != audited_minors ||
        heap->actor_candidate_pending_id != id || heap->actor_candidate_group != group ||
        heap->candidate_ledger_groups != 1 || object_chunk == NULL ||
        object_chunk->candidate_id != id || object_chunk->candidate_state != 1 ||
        !kiln_c99_candidate_group_contains(group, object_chunk) || object_block == NULL)
        return 6;
    kiln_c99_actor_candidate_finish(heap, 0);
    heap->realm.isolated_graphs = kiln_c99_undefined();
    return 0;
}

int main(void) {
    uint64_t realm;
    int status;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 7;
    status = kiln_realm_enter(realm, candidate_minor_entry, NULL);
    if (kiln_realm_destroy(realm) != KILN_C99_OK) return 8;
    return status;
}
"#;
    let run = run_with_main(&generated, wrapper, "candidate-minor-eligibility");
    assert!(
        run.status.success(),
        "a requested minor collection must fall back to full collection while candidate provenance is live ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_zero_argument_string_construction_uses_a_valid_header() {
    let generated =
        emit(&program(vec![], vec![Instruction::Return], 0)).expect("empty wrapper fixture emits");
    let wrapper = format!(
        r#"
int main(void) {{
    uint64_t realm = 0;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 99;
    kiln_test_realm_active = 1;
    kiln_value dummy, empty, wrapped, length, thrown = kiln_c99_undefined();
    if ({}(NULL, &dummy) != KILN_C99_OK) return 1;
    kiln_value constructor = kiln_c99_builtin(KILN_C99_BUILTIN_STRING);
    if (kiln_c99_call(NULL, &empty, constructor, kiln_c99_undefined(), NULL, 0, 0, &thrown) != KILN_C99_OK) return 2;
    if (empty.tag != KILN_VALUE_STRING || empty.ptr == NULL ||
        ((const kiln_string *)empty.ptr)->unit_len != 0) return 3;
    if (kiln_c99_construct(NULL, &wrapped, constructor, NULL, 0, 0, &thrown) != KILN_C99_OK) return 4;
    if (kiln_c99_get(NULL, wrapped, "length", 6, &length, 0, &thrown) != KILN_C99_OK ||
        length.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(length) != 0) return 5;
    kiln_value primitive;
    if (!kiln_c99_unwrap_primitive_wrapper(wrapped, KILN_C99_WRAPPER_STRING, &primitive) ||
        !kiln_c99_same_value_zero(empty, primitive)) return 6;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 7;
}}
"#,
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "zero-argument-string-wrapper");
    assert!(
        run.status.success(),
        "empty String wrapper failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}
