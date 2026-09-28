use super::*;

#[test]
fn boxed_tag_round_trips_keep_pointer_kinds_and_nan_distinct() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    static int payload;
    const void *p = &payload;
    kiln_value object, array, text, result;
    kiln_val boxed;
    uint64_t realm;
    struct { uint64_t prefix; uint32_t tag; } direct[] = {
        { KILN_TAG_CLOSURE_PREFIX, KILN_VALUE_FUNCTION },
        { KILN_TAG_SYMBOL_PREFIX, KILN_VALUE_SYMBOL },
        { KILN_TAG_BIGINT_PREFIX, KILN_VALUE_BIGINT },
        { KILN_TAG_TEMPORAL_PREFIX, KILN_VALUE_TEMPORAL },
        { KILN_TAG_CELL_PREFIX, KILN_VALUE_CELL }
    };
    size_t i;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    for (i = 0; i < sizeof direct / sizeof direct[0]; ++i) {
        boxed = KILN_VAL_TAG_PTR(direct[i].prefix, p);
        result = kiln_val_to_boundary(boxed);
        if (result.tag != direct[i].tag || result.bits != (uint64_t)(uintptr_t)p ||
            kiln_val_from_boundary(result) != boxed) return 2;
    }
    if (kiln_c99_new_object(&object) != KILN_C99_OK ||
        kiln_c99_new_array(&array) != KILN_C99_OK) return 3;
    boxed = kiln_val_from_boundary(object);
    if (!KILN_VAL_IS_OBJECT(boxed) || kiln_val_to_boundary(boxed).tag != KILN_VALUE_OBJECT)
        return 4;
    boxed = kiln_val_from_boundary(array);
    if ((boxed & KILN_TAG_MASK) != KILN_TAG_BUFFER_PREFIX ||
        kiln_val_to_boundary(boxed).tag != KILN_VALUE_ARRAY) return 5;
    text = kiln_c99_copy_text("x", 1);
    boxed = kiln_val_from_boundary(text);
    if (!KILN_VAL_IS_STRING(boxed) || kiln_val_to_boundary(boxed).tag != KILN_VALUE_STRING)
        return 6;
    boxed = kiln_c99_val_from_double(NAN);
    result = kiln_val_to_boundary(boxed);
    if (result.tag != KILN_VALUE_NUMBER || !isnan(kiln_c99_as_number(result)) ||
        KILN_VAL_IS_OBJECT(boxed) || KILN_VAL_IS_STRING(boxed)) return 7;
    /* When a new boxed kind is reserved it must miss every existing object/array matcher. */
#ifdef KILN_TAG_SHARED_PREFIX
    boxed = KILN_VAL_TAG_PTR(KILN_TAG_SHARED_PREFIX, p);
    if (KILN_VAL_IS_NUMBER(boxed) || KILN_VAL_IS_OBJECT(boxed) ||
        (boxed & KILN_TAG_MASK) == KILN_TAG_BUFFER_PREFIX ||
        (boxed & KILN_TAG_MASK) == KILN_TAG_CELL_PREFIX ||
        !KILN_VAL_IS_SHARED(boxed)) return 8;
    result = kiln_val_to_boundary(boxed);
    if (result.tag != KILN_VALUE_SHARED || result.bits != (uint64_t)(uintptr_t)p ||
        kiln_val_from_boundary(result) != boxed) return 9;
#endif
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 10;
}
"#;
    let run = run_with_main(&generated, wrapper, "boxed-tag-roundtrip");
    assert!(
        run.status.success(),
        "boxed tag round-trip failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn native_shared_facade_cache_is_weak_and_releases_its_lease() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value first, same, after;
    kiln_shared_facade *payload;
    if (kiln_c99_new_shared_facade(17, 1, 0, &first) != KILN_C99_OK) return 1;
    if (kiln_c99_new_shared_facade(17, 1, 0, &same) != KILN_C99_OK) return 2;
    if (first.bits != same.bits || kiln_test_shared_retains != 1) return 3;
    payload = (kiln_shared_facade *)(uintptr_t)first.bits;
    kiln_c99_gc_finalize(kiln_c99_gc_heap(), ((kiln_gc_block *)payload) - 1);
    if (kiln_test_shared_releases != 1 ||
        kiln_c99_gc_heap()->shared_facades != NULL) return 4;
    if (kiln_c99_new_shared_facade(17, 1, 0, &after) != KILN_C99_OK) return 5;
    if (kiln_test_shared_retains != 2) return 6;
    payload = (kiln_shared_facade *)(uintptr_t)after.bits;
    kiln_c99_gc_finalize(kiln_c99_gc_heap(), ((kiln_gc_block *)payload) - 1);
    return kiln_test_shared_releases == 2 ? 0 : 7;
}
"#;
    let run = run_with_main(&generated, wrapper, "shared-facade-weak-cache");
    assert!(
        run.status.success(),
        "shared facade cache/lease failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn native_shared_facade_reads_copied_fields_and_realm_prototype() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value shared, answer, keys, descriptor, key;
    int status;
    int found = 0, accepted = 0;
    if (kiln_c99_new_shared_facade(17, 1, 0, &shared) != KILN_C99_OK) return 1;
    status = kiln_c99_get(NULL, shared, "x", 1, &answer, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_NUMBER ||
        kiln_c99_as_number(answer) != 7.0) return 2;
    status = kiln_c99_get(NULL, shared, "toString", 8, &answer, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_METHOD) return 3;
    status = kiln_c99_get(NULL, shared, "absent", 6, &answer, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_UNDEFINED) return 4;
    status = kiln_c99_all_own_keys(NULL, shared, &keys, 0, NULL);
    if (status != KILN_C99_OK || keys.tag != KILN_VALUE_ARRAY ||
        kiln_c99_array_of(keys)->count != 1 ||
        !kiln_test_units_equal(kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[0]),
                               "x", 1)) return 5;
    status = kiln_c99_own_keys(NULL, shared, &keys, 0, NULL);
    if (status != KILN_C99_OK || keys.tag != KILN_VALUE_ARRAY ||
        kiln_c99_array_of(keys)->count != 1) return 6;
    status = kiln_c99_describe_property(NULL, shared, "x", 1, &descriptor, &found, 0, NULL);
    if (status != KILN_C99_OK || !found || descriptor.tag != KILN_VALUE_OBJECT) return 7;
    status = kiln_c99_get(NULL, descriptor, "writable", 8, &answer, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_BOOLEAN || answer.bits != 0) return 8;
    status = kiln_c99_get(NULL, descriptor, "configurable", 12, &answer, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_BOOLEAN || answer.bits != 0) return 9;
    status = kiln_c99_invoke_method(NULL, &answer, KILN_C99_METHOD_OBJECT_IS_FROZEN,
                                    kiln_c99_undefined(), &shared, 1, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_BOOLEAN || answer.bits != 1) return 10;
    status = kiln_c99_invoke_method(NULL, &answer, KILN_C99_METHOD_OBJECT_IS_EXTENSIBLE,
                                    kiln_c99_undefined(), &shared, 1, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_BOOLEAN || answer.bits != 0) return 11;
    status = kiln_c99_invoke_method(NULL, &answer, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                                    kiln_c99_undefined(), &shared, 1, 0, NULL);
    if (status != KILN_C99_OK || answer.tag != KILN_VALUE_BUILTIN ||
        answer.bits != KILN_C99_BUILTIN_OBJECT_PROTOTYPE) return 12;
    key = kiln_c99_copy_text("x", 1);
    status = kiln_c99_has_computed(NULL, shared, key, &found, 0, NULL);
    if (status != KILN_C99_OK || !found) return 13;
    status = kiln_c99_delete_computed(NULL, shared, key, &accepted, 0, NULL);
    if (status != KILN_C99_OK || accepted) return 14;
    status = kiln_c99_reflect_set(NULL, shared, key, kiln_c99_number(8), &accepted, 0, NULL);
    return status == KILN_C99_OK && !accepted ? 0 : 15;
}
"#;
    let run = run_with_main(&generated, wrapper, "shared-facade-read");
    assert!(
        run.status.success(),
        "shared facade read failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn native_shared_facade_survives_only_through_an_ordinary_root() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_gc_heap heap;
    kiln_value parent, child, read_back;
    uint64_t releases;
    kiln_c99_gc_heap_init(&heap);
    kiln_test_heap = &heap;
    kiln_test_generation++;
    kiln_test_realm_active = 1;
    if (kiln_c99_new_shared_facade(17, 1, 0, &child) != KILN_C99_OK ||
        kiln_c99_new_object(&parent) != KILN_C99_OK ||
        kiln_c99_put(NULL, parent, "child", 5, child, 0, NULL) != KILN_C99_OK) return 1;
    kiln_test_gc_root = parent;
    releases = kiln_test_shared_releases;
    heap.enabled = 1;
    kiln_c99_gc_collect(&heap, 1);
    if (kiln_test_shared_releases != releases || heap.shared_facades == NULL) return 2;
    if (kiln_c99_get(NULL, parent, "child", 5, &read_back, 0, NULL) != KILN_C99_OK ||
        read_back.tag != KILN_VALUE_SHARED || read_back.bits != child.bits) return 3;
    kiln_test_gc_root = kiln_c99_undefined();
    kiln_c99_gc_collect(&heap, 1);
    if (kiln_test_shared_releases != releases + 1 || heap.shared_facades != NULL) return 4;
    heap.enabled = 0;
    kiln_test_heap = NULL;
    kiln_test_realm_active = 0;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "shared-facade-forced-gc");
    assert!(
        run.status.success(),
        "shared facade forced-GC root failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn native_shared_snapshot_distinguishes_symbol_from_colliding_string() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    uint64_t realm;
    kiln_value object, symbol, string, answer, own, enumerable;
    kiln_shared_source_node node = {0};
    kiln_shared_source_field first = {0}, second = {0};
    const kiln_string *key;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_new_symbol(&symbol, kiln_c99_undefined()) != KILN_C99_OK) return 2;
    key = kiln_c99_symbol_of(symbol)->key;
    string = kiln_c99_copy_text((const char *)key->data, key->unit_len);
    if (string.tag != KILN_VALUE_STRING || kiln_c99_new_object(&object) != KILN_C99_OK) return 3;
    if (kiln_c99_define_computed(object, symbol, kiln_c99_number(1)) != KILN_C99_OK ||
        kiln_c99_define_computed(object, string, kiln_c99_number(2)) != KILN_C99_OK) return 4;
    if (kiln_c99_get_computed(NULL, object, symbol, &answer, 0, NULL) != KILN_C99_OK ||
        answer.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(answer) != 1) return 5;
    if (kiln_c99_get_computed(NULL, object, string, &answer, 0, NULL) != KILN_C99_OK ||
        answer.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(answer) != 2) return 6;
    if (kiln_c99_all_own_keys(NULL, object, &own, 0, NULL) != KILN_C99_OK ||
        own.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(own)->count != 2 ||
        kiln_val_to_boundary(kiln_c99_array_of(own)->elements[0]).tag != KILN_VALUE_STRING ||
        kiln_val_to_boundary(kiln_c99_array_of(own)->elements[1]).tag != KILN_VALUE_SYMBOL)
        return 10;
    if (kiln_c99_own_keys(NULL, object, &enumerable, 0, NULL) != KILN_C99_OK ||
        enumerable.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(enumerable)->count != 1 ||
        kiln_val_to_boundary(kiln_c99_array_of(enumerable)->elements[0]).tag != KILN_VALUE_STRING)
        return 11;
    if (kiln_c99_shared_source_inspect(NULL, kiln_c99_shared_source_id(object), UINT64_MAX,
                                       &node, NULL) != 0 || node.field_count != 2) return 7;
    if (kiln_c99_shared_source_inspect(NULL, kiln_c99_shared_source_id(object), 0,
                                       NULL, &first) != 0 ||
        kiln_c99_shared_source_inspect(NULL, kiln_c99_shared_source_id(object), 1,
                                       NULL, &second) != 0 ||
        first.key_kind == second.key_kind ||
        (first.key_kind == 0 && !first.enumerable) ||
        (second.key_kind == 0 && !second.enumerable)) return 8;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 9;
}
"#;
    let run = run_with_main(&generated, wrapper, "shared-snapshot-symbol-string-collision");
    assert!(
        run.status.success(),
        "symbol/string collision failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn native_ordinary_double_at_string_is_an_enumerable_string_key() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    uint64_t realm;
    kiln_value object, name, answer, all, keys;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_new_object(&object) != KILN_C99_OK) return 2;
    if (kiln_c99_put(NULL, object, "@@x", 3, kiln_c99_number(11), 0, NULL) != KILN_C99_OK)
        return 8;
    name = kiln_c99_copy_text("@@x", 3);
    if (kiln_c99_define_computed(object, name, kiln_c99_number(7)) != KILN_C99_OK) return 3;
    if (kiln_c99_get(NULL, object, "@@x", 3, &answer, 0, NULL) != KILN_C99_OK ||
        answer.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(answer) != 11) return 9;
    if (kiln_c99_get_computed(NULL, object, name, &answer, 0, NULL) != KILN_C99_OK ||
        answer.tag != KILN_VALUE_NUMBER || kiln_c99_as_number(answer) != 7) return 4;
    if (kiln_c99_all_own_keys(NULL, object, &all, 0, NULL) != KILN_C99_OK ||
        all.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(all)->count != 1 ||
        kiln_val_to_boundary(kiln_c99_array_of(all)->elements[0]).tag != KILN_VALUE_STRING)
        return 5;
    if (kiln_c99_own_keys(NULL, object, &keys, 0, NULL) != KILN_C99_OK ||
        keys.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(keys)->count != 1 ||
        kiln_val_to_boundary(kiln_c99_array_of(keys)->elements[0]).tag != KILN_VALUE_STRING)
        return 6;
    { kiln_shared_source_node node = {0};
      if (kiln_c99_shared_source_inspect(NULL, kiln_c99_shared_source_id(object), UINT64_MAX,
                                         &node, NULL) == 0) return 10; }
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 7;
}
"#;
    let run = run_with_main(&generated, wrapper, "ordinary-double-at-key");
    assert!(
        run.status.success(),
        "ordinary double-at key failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}
#[test]
fn owned_boundary_strings_preserve_header_identity_across_conversions() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    uint64_t realm;
    const char bytes[] = {(char)0xc3, (char)0xa9, 0, (char)0xff};
    /* The byte string above decodes to three units: U+00E9, U+0000, and U+FFFD for the
     * lone 0xFF. The boundary carries units, not bytes, so identity is asserted in units. */
    static const uint16_t want[] = {0x00e9, 0x0000, 0xfffd};
    kiln_value owned = kiln_c99_copy_text(bytes, sizeof bytes);
    kiln_value borrowed;
    kiln_value unflagged;
    kiln_value second;
    kiln_val owned_value;
    const kiln_string *header;
    size_t index;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (owned.tag != KILN_VALUE_STRING || owned.flags != KILN_VALUE_FLAG_OWNED_STRING ||
        owned.len != 3 || owned.ptr == NULL) return 2;
    header = (const kiln_string *)owned.ptr;
    if (header->data == NULL || header->unit_len != 3) return 3;
    /* An owned boundary string crosses into a box by pointer: no copy. */
    owned_value = kiln_val_from_boundary(owned);
    if (!KILN_VAL_IS_STRING(owned_value) || (const kiln_string *)KILN_VAL_PTR(owned_value) != header)
        return 4;
    /* ... and back out with the flag recomputed from the shared header/payload layout. */
    borrowed = kiln_val_to_boundary(owned_value);
    if (borrowed.flags != KILN_VALUE_FLAG_OWNED_STRING || borrowed.ptr != owned.ptr ||
        borrowed.len != owned.len) return 5;
    /* The flag is observational: clearing it changes nothing about the crossing itself. */
    unflagged = kiln_c99_value(KILN_VALUE_STRING, 0, owned.ptr, owned.len);
    if (KILN_VAL_PTR(kiln_val_from_boundary(unflagged)) != KILN_VAL_PTR(owned_value)) return 6;
    /* A second construction of the same bytes is unit-identical, whatever its header. */
    second = kiln_c99_copy_text(bytes, sizeof bytes);
    if (second.tag != KILN_VALUE_STRING || second.len != 3) return 7;
    header = (const kiln_string *)second.ptr;
    if (header->unit_len != 3) return 8;
    for (index = 0; index < 3; ++index) {
        if (kiln_string_unit_at(header, index) != want[index]) return 8;
    }
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 9;
}
"#;
    let run = run_with_main(&generated, wrapper, "owned-boundary-string");
    assert!(
        run.status.success(),
        "owned boundary string test failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_numeric_operand_order_matches_symbol_conversion_rules() {
    for (operator, expected_value_of_calls) in [
        (BinaryOp::Multiply, 0),
        (BinaryOp::Add, 1),
        (BinaryOp::LessThan, 1),
    ] {
        let generated = emit(&Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::String("valueOf".to_owned().into()),
                Constant::String("called".to_owned().into()),
                Constant::Number(1.0f64.to_bits()),
            ],
            code: vec![
                Instruction::LoadBuiltin(Builtin::Symbol),
                Instruction::Call { argc: 0 },
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Binary(operator),
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![
                FunctionProto {
                    inferred_name: None,
                    span: kiln_ir::Span::default(),
                    prologue_end: 0,
                    asynchronous: false,
                    name: Some("right".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    is_method: false,
                    parameter_count: 0,
                    declared_length: 0,
                    mapped_arguments: false,
                    local_count: 0,
                    code: vec![
                        Instruction::Const(0),
                        Instruction::MakeFunction(1),
                        Instruction::NewObject { count: 1 },
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                    type_annotations: None,
                },
                FunctionProto {
                    inferred_name: None,
                    span: kiln_ir::Span::default(),
                    prologue_end: 0,
                    asynchronous: false,
                    name: Some("valueOf".to_owned()),
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    is_method: false,
                    parameter_count: 0,
                    declared_length: 0,
                    mapped_arguments: false,
                    local_count: 0,
                    code: vec![
                        Instruction::Const(1),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 1,
                        },
                        Instruction::Const(2),
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                    type_annotations: None,
                },
            ],
            template_sites: Vec::new(),
        })
        .expect("symbol conversion order probe emits");
        let wrapper = format!(
            r#"
static int value_of_calls;
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL ||
        arguments[0].tag != KILN_VALUE_STRING ||
        !kiln_test_units_equal(arguments[0], "called", 6)) return 90;
    ++value_of_calls;
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
}}
int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result = kiln_c99_undefined();
    int status = {symbol}(&host, &result);
    if (value_of_calls != {expected_value_of_calls}) return 10;
    return status == KILN_C99_THROWN ? 0 : 11;
}}
"#,
            symbol = generated.symbol,
            expected_value_of_calls = expected_value_of_calls
        );
        let run = run_with_main(&generated, &wrapper, "symbol-operand-order");
        assert!(
            run.status.success(),
            "{operator:?} operand order failed ({:?}): {}",
            run.status.code(),
            String::from_utf8_lossy(&run.stderr)
        );
    }
}

#[test]
fn executable_symbol_registry_is_shared_and_distinct_from_unique_symbols() {
    let generated = emit(&program(
        vec![Constant::String("fastify-error-generic".into())],
        vec![Instruction::Return],
        0,
    ))
    .expect("symbol registry probe emits");
    let wrapper = r#"
int main(void) {
    uint64_t realm;
    kiln_value key = kiln_c99_copy_text("fastify-error-generic", 21);
    kiln_value first;
    kiln_value second;
    kiln_value unique;
    kiln_value recovered;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_symbol_for(&first, key) != KILN_C99_OK) return 1;
    if (kiln_c99_symbol_for(&second, key) != KILN_C99_OK) return 2;
    if (!kiln_c99_strict_equal(first, second)) return 3;
    recovered = kiln_c99_symbol_key_for(first);
    if (!kiln_test_units_equal(recovered, "fastify-error-generic", 21)) return 4;
    if (kiln_c99_new_symbol(&unique, key) != KILN_C99_OK) return 5;
    if (kiln_c99_strict_equal(first, unique)) return 6;
    if (kiln_c99_symbol_key_for(unique).tag != KILN_VALUE_UNDEFINED) return 7;
    /* The symbol's property key is the registry slot (`@@symbol-for:<id>`), never the
     * description the registry round-trips above. */
    if (kiln_test_units_equal(kiln_c99_property_key(first), "fastify-error-generic", 21))
        return 8;
    if (!kiln_test_units_equal(kiln_c99_text(first), "Symbol(fastify-error-generic)", 29))
        return 9;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 10;
}
"#
    .to_string();
    let run = run_with_main(&generated, &wrapper, "symbol-registry");
    assert!(
        run.status.success(),
        "native symbol registry failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn emits_stable_source_and_path_independent_symbol() {
    let program = program(
        vec![Constant::Number(42.0f64.to_bits())],
        vec![Instruction::Const(0), Instruction::Return],
        0,
    );
    let first = emit(&program).expect("program emits");
    let second = emit(&program).expect("program emits again");
    assert_eq!(first, second);
    assert!(first.symbol.starts_with("kiln_module_"));
    assert_eq!(first.symbol.len(), "kiln_module_".len() + 32);
    assert!(
        first.symbol["kiln_module_".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert!(!first.source.contains("/Users/"));
    assert!(first.source.contains(&format!(
        "int {}(kiln_host *host, kiln_value *result)",
        first.symbol
    )));
}

#[test]
fn emits_every_scalar_constant_and_host_call_through_numeric_abi() {
    let program = program(
        vec![
            Constant::Undefined,
            Constant::Null,
            Constant::Boolean(true),
            Constant::Number((-0.0f64).to_bits()),
            Constant::String("a\0é".into()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Const(3),
            Instruction::Const(4),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 5,
            },
            Instruction::Return,
        ],
        0,
    );
    let generated = emit(&program).expect("program emits").source;
    assert!(generated.contains("UINT16_C(1), UINT16_C(5)"));
    assert!(generated.contains("typedef int (*kiln_host_call_fn)"));
    assert!(generated.contains("char kiln_module_") || generated.contains("_constant_4"));
    assert!(!generated.contains("printf(") || generated.contains("snprintf("));
    // Headed constants store exact units in a decimal array, so the old C-string-literal
    // hazards are gone by construction: no hex escape can swallow a following digit, and an
    // embedded NUL is a 0 unit rather than a terminator. `"a\0é"` must survive as all three
    // units in Latin1 storage.
    assert!(generated.contains("{ 97, 0, 233 }"));
    assert!(generated.contains("KILN_STRING_LATIN1"));
}

#[test]
fn generated_numeric_conversion_ignores_the_embedding_process_locale() {
    let program = program(
        vec![
            Constant::String("1.5".into()),
            Constant::Number(0.0_f64.to_bits()),
            Constant::String("value=".into()),
            Constant::Number(1.5_f64.to_bits()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Subtract),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Pop,
            Instruction::Const(2),
            Instruction::Const(3),
            Instruction::Binary(BinaryOp::Add),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        0,
    );
    let generated = emit(&program).expect("locale probe emits");
    // The string arrives as ASCII bytes borrowed from the headed value; the provider call
    // itself is unchanged, which is what keeps this free of `localeconv`.
    assert!(
        generated
            .source
            .contains("kiln_string_to_number(bytes, length)")
    );
    // A *call*, matching the printf check below. Both names still appear in the runtime's
    // prose, where they are naming the things that must not be reached for.
    assert!(!generated.source.contains("strtod("));
    assert!(!generated.source.contains("kiln_c99_strtod_c"));
    assert!(generated.source.contains("KILN_NUMBER_FORMAT_VERSION 1"));
    assert!(generated.source.contains("KILN_NUMBER_PARSE_VERSION 1"));
    // The spelling is `kiln_c99_double_to_js`, in the prelude's own C, rather than the provider
    // call this used to name: nothing it does consults `localeconv`, and the executable below
    // proves that under a comma-decimal locale rather than only asserting it.
    assert!(
        generated
            .source
            .contains("kiln_c99_double_to_js(number, buffer)")
    );
    assert!(!generated.source.contains("snprintf("));
    assert!(!generated.source.contains("setlocale("));
    let wrapper = format!(
        r#"
#include <locale.h>

static int calls = 0;

static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL)
        return KILN_C99_HOST_ERROR;
    if (calls == 0) {{
        if (arguments[0].tag != KILN_VALUE_NUMBER ||
            arguments[0].bits != UINT64_C({number_bits}))
            return KILN_C99_HOST_ERROR;
    }} else if (calls == 1) {{
        static const char expected[] = "value=1.5";
        if (arguments[0].tag != KILN_VALUE_STRING ||
            !kiln_test_units_equal(arguments[0], expected, sizeof(expected) - 1))
            return KILN_C99_HOST_ERROR;
    }} else {{
        return KILN_C99_HOST_ERROR;
    }}
    ++calls;
    *result = kiln_c99_undefined();
    return KILN_C99_OK;
}}

int main(void) {{
    static const char *candidates[] = {{
        "de_DE.UTF-8", "de_DE.utf8", "de_DE",
        "fr_FR.UTF-8", "fr_FR.utf8", "fr_FR",
        "it_IT.UTF-8", "es_ES.UTF-8", "pt_BR.UTF-8"
    }};
    kiln_host host = {{ host_call, NULL }};
    kiln_value result;
    size_t index;
    int found = 0;
    for (index = 0; index < sizeof(candidates) / sizeof(candidates[0]); ++index) {{
        struct lconv *conventions;
        if (setlocale(LC_NUMERIC, candidates[index]) == NULL) continue;
        conventions = localeconv();
        if (conventions != NULL && strcmp(conventions->decimal_point, ",") == 0) {{
            found = 1;
            break;
        }}
    }}
    if (!found) return 77;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    return calls == 2 ? 0 : 3;
}}
"#,
        number_bits = 1.5_f64.to_bits(),
        symbol = generated.symbol,
    );
    let output_path =
        std::env::temp_dir().join(format!("kiln-backend-c-locale-{}", std::process::id()));
    let mut child = Command::new("cc")
        .args(["-std=c11", "-x", "c", "-"])
        .arg("-lm")
        .arg("-o")
        .arg(&output_path)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("a C compiler is available for locale test");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(linked_source(&generated.source, &wrapper).as_bytes())
        .expect("locale source is accepted by cc stdin");
    let compile = child.wait_with_output().expect("cc exits");
    assert!(
        compile.status.success(),
        "locale-independent generated C did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&output_path)
        .output()
        .expect("locale probe runs");
    let _ = std::fs::remove_file(output_path);
    if run.status.code() == Some(77) {
        eprintln!("skipping hostile-locale execution: no comma-decimal locale is installed");
        return;
    }
    assert!(
        run.status.success(),
        "generated numeric conversion inherited the embedding locale: {:?}",
        run.status.code()
    );
}

#[test]
fn executable_array_buffer_is_view_recognizes_native_views() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value method;
    kiln_value view;
    kiln_value buffer;
    kiln_value result;
    kiln_value argument;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                     "isView", 6, &method, 0, &thrown) != KILN_C99_OK ||
        method.tag != KILN_VALUE_METHOD ||
        method.bits != KILN_C99_METHOD_ARRAY_BUFFER_IS_VIEW) return 1;
    if (kiln_c99_new_typed_buffer(&buffer, 8) != KILN_C99_OK) return 2;
    if (kiln_c99_new_typed_array(&view, KILN_TYPED_UINT8, buffer, 0, 8, 1,
                                 &thrown) != KILN_C99_OK) return 3;
    argument = view;
    if (kiln_c99_call(NULL, &result, method,
                      kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                      &argument, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || !result.bits) return 4;
    argument = buffer;
    if (kiln_c99_call(NULL, &result, method,
                      kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                      &argument, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || result.bits) return 5;
    argument = kiln_c99_undefined();
    if (kiln_c99_call(NULL, &result, method,
                      kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                      &argument, 1, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || result.bits) return 6;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "array-buffer-is-view");
    assert!(
        run.status.success(),
        "ArrayBuffer.isView failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_typed_array_tag_is_a_shared_prototype_accessor() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value descriptor;
    kiln_value getter;
    kiln_value view;
    kiln_value tag;
    kiln_value thrown = kiln_c99_undefined();
    int found = 0;
    if (kiln_c99_describe_property(NULL,
                                   kiln_c99_builtin(KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE),
                                   "@@toStringTag", 13, &descriptor, &found, 0, &thrown) !=
            KILN_C99_OK || !found) return 1;
    if (kiln_c99_get(NULL, descriptor, "get", 3, &getter, 0, &thrown) != KILN_C99_OK ||
        getter.tag != KILN_VALUE_METHOD ||
        getter.bits != KILN_C99_METHOD_TYPED_TO_STRING_TAG) return 2;
    if (kiln_c99_new_typed_array(&view, KILN_TYPED_INT8, kiln_c99_undefined(), 0, 0, 0,
                                 &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_call(NULL, &tag, getter, view, NULL, 0, 0, &thrown) != KILN_C99_OK ||
        tag.tag != KILN_VALUE_STRING ||
        !kiln_test_units_equal(tag, "Int8Array", 9)) return 4;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "typed-array-tag-accessor");
    assert!(
        run.status.success(),
        "typed-array tag accessor failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_array_from_maps_an_array_like_object() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
static int mapper_fn(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                     const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {
    (void)ctx; (void)upvalues; (void)receiver_ptr; (void)args; (void)argc;
    *result = KILN_VAL_UNDEFINED;
    return KILN_C99_OK;
}
static const kiln_string mapper_name = { 6, KILN_STRING_LATIN1, "mapper" };
int main(void) {
    uint64_t realm;
    kiln_value source;
    kiln_value method = kiln_c99_method(KILN_C99_METHOD_ARRAY_FROM);
    kiln_value mapper;
    kiln_value arguments[3];
    kiln_value result;
    kiln_value value;
    kiln_value thrown = kiln_c99_undefined();
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&mapper, mapper_fn, &mapper_name, NULL, 0, "<test>", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 1;
    if (kiln_c99_new_object(&source) != KILN_C99_OK) return 1;
    if (kiln_c99_put(NULL, source, "length", 6, kiln_c99_number(2), 0, &thrown) != KILN_C99_OK ||
        kiln_c99_put(NULL, source, "0", 1, kiln_c99_copy_text("a", 1), 0, &thrown) != KILN_C99_OK)
        return 2;
    arguments[0] = source;
    arguments[1] = mapper;
    arguments[2] = kiln_c99_undefined();
    if (kiln_c99_call(NULL, &result, method, kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY),
                      arguments, 3, 0, &thrown) != KILN_C99_OK) return 3;
    if (result.tag != KILN_VALUE_ARRAY || kiln_c99_array_of(result)->count != 2) return 4;
    value = kiln_val_to_boundary(kiln_c99_array_of(result)->elements[0]);
    if (value.tag != KILN_VALUE_UNDEFINED) return 5;
    value = kiln_val_to_boundary(kiln_c99_array_of(result)->elements[1]);
    if (value.tag != KILN_VALUE_UNDEFINED) return 6;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 7;
}
"#;
    let run = run_with_main(&generated, wrapper, "array-from-array-like");
    assert!(
        run.status.success(),
        "Array.from array-like failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_concat_accepts_a_null_pointer_for_an_empty_host_string() {
    let generated = emit(&program(
        vec![Constant::String("x".into())],
        vec![
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 0,
            },
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::Add),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        0,
    ))
    .expect("empty host string program emits");
    let executable = std::env::temp_dir().join(format!(
        "kiln-backend-c-empty-host-string-{}-{}",
        std::process::id(),
        generated.program_hash
    ));
    let wrapper = format!(
        r#"
static int calls = 0;

static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || result == NULL) return KILN_C99_HOST_ERROR;
    if (calls == 0 && argc == 0) {{
        *result = kiln_c99_value(KILN_VALUE_STRING, 0, NULL, 0);
    }} else if (calls == 1 && argc == 1 && arguments != NULL &&
               arguments[0].tag == KILN_VALUE_STRING &&
               kiln_test_units_equal(arguments[0], "x", 1)) {{
        *result = kiln_c99_undefined();
    }} else {{
        return KILN_C99_HOST_ERROR;
    }}
    ++calls;
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    return calls == 2 ? 0 : 3;
}}
"#,
        symbol = generated.symbol
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
        .expect("empty host string source is accepted by cc stdin");
    let compile = child.wait_with_output().expect("cc exits");
    assert!(
        compile.status.success(),
        "generated empty host string C did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&executable)
        .output()
        .expect("generated empty host string executable runs");
    let _ = std::fs::remove_file(&executable);
    assert!(run.status.success(), "empty host string concat failed");
}

#[test]
fn unreachable_weak_map_builtin_is_still_rendered_consistently() {
    let result = emit(&program(
        vec![Constant::Boolean(true)],
        vec![
            Instruction::Jump(2),
            Instruction::LoadBuiltin(kiln_ir::Builtin::WeakMap),
            Instruction::Const(0),
            Instruction::Return,
        ],
        0,
    ))
    .expect("WeakMap is a native builtin");
    assert!(result.source.contains("KILN_C99_BUILTIN_WEAK_MAP"));
}

#[test]
fn borrowed_array_map_and_filter_keep_generic_typed_array_semantics() {
    let callback = FunctionProto {
        inferred_name: None,
        /* Return the callback's third argument, the receiver. This makes a snapshot receiver
         * observably different from the original typed view. */
        code: vec![
            Instruction::LoadArguments,
            Instruction::Const(8),
            Instruction::GetComputed,
            Instruction::Return,
        ],
        ..function(Vec::new())
    };
    let constants = vec![
        Constant::Number(1.0f64.to_bits()),
        Constant::Number(2.0f64.to_bits()),
        Constant::String("prototype".into()),
        Constant::String("map".into()),
        Constant::String("filter".into()),
        Constant::String("call".into()),
        Constant::String("constructor".into()),
        Constant::String("length".into()),
        Constant::String("2".into()),
        Constant::Number(0.0f64.to_bits()),
        Constant::Boolean(true),
        Constant::Boolean(false),
    ];
    let mut code = vec![
        // typed = new Uint8Array([1, 2])
        Instruction::LoadBuiltin(Builtin::Uint8Array),
        Instruction::Const(0),
        Instruction::Const(1),
        Instruction::NewArray { count: 2 },
        Instruction::New { argc: 1 },
        Instruction::StoreLocal(0),
        // mapped = Array.prototype.map.call(typed, callback)
        Instruction::LoadBuiltin(Builtin::Array),
        Instruction::GetProperty(2),
        Instruction::GetProperty(3),
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(0),
        Instruction::CallMethod { name: 5, argc: 2 },
        Instruction::StoreLocal(1),
        // The borrowed method returns an ordinary Array and passes the original typed view.
        Instruction::LoadLocal(1),
        Instruction::GetProperty(6),
        Instruction::LoadBuiltin(Builtin::Array),
        Instruction::Binary(BinaryOp::StrictEqual),
    ];
    let mut failed = Vec::new();
    failed.push(code.len());
    code.push(Instruction::JumpIfFalse(0));
    code.extend([
        Instruction::LoadLocal(1),
        Instruction::Const(9),
        Instruction::GetComputed,
        Instruction::LoadLocal(0),
        Instruction::Binary(BinaryOp::StrictEqual),
    ]);
    failed.push(code.len());
    code.push(Instruction::JumpIfFalse(0));
    code.extend([
        // filtered = Array.prototype.filter.call(typed, callback)
        Instruction::LoadBuiltin(Builtin::Array),
        Instruction::GetProperty(2),
        Instruction::GetProperty(4),
        Instruction::LoadLocal(0),
        Instruction::MakeFunction(0),
        Instruction::CallMethod { name: 5, argc: 2 },
        Instruction::StoreLocal(2),
        Instruction::LoadLocal(2),
        Instruction::GetProperty(6),
        Instruction::LoadBuiltin(Builtin::Array),
        Instruction::Binary(BinaryOp::StrictEqual),
    ]);
    failed.push(code.len());
    code.push(Instruction::JumpIfFalse(0));
    code.extend([
        Instruction::LoadLocal(2),
        Instruction::GetProperty(7),
        Instruction::Const(1),
        Instruction::Binary(BinaryOp::StrictEqual),
    ]);
    failed.push(code.len());
    code.push(Instruction::JumpIfFalse(0));
    code.extend([Instruction::Const(10), Instruction::Return]);
    let failure = code.len() as u32;
    code.extend([Instruction::Const(11), Instruction::Return]);
    for jump in failed {
        code[jump] = Instruction::JumpIfFalse(failure);
    }

    let generated = emit(&Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 3,
        functions: vec![callback],
        template_sites: Vec::new(),
    })
    .expect("borrowed typed-array methods emit");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "typed-array-borrowed-generic");
    assert!(
        run.status.success(),
        "borrowed Array.prototype map/filter changed typed-array semantics (exit {:?})",
        run.status.code()
    );
}

#[test]
fn executable_array_string_iterator_and_named_enumeration_are_distinct() {
    let generated = emit(&program(
        vec![Constant::String("@@iterator".into())],
        vec![
            Instruction::NewArray { count: 0 },
            Instruction::Const(0),
            Instruction::GetComputed,
            Instruction::Return,
        ],
        0,
    ))
    .expect("array string iterator probe emits");
    let wrapper = format!(
        r#"
int main(void) {{
    kiln_value result;
    int status = {symbol}(NULL, &result);
    if (status != KILN_C99_OK) {{ fprintf(stderr, "status=%d detail=%s\n", status,
        kiln_c99_diagnostic_detail == NULL ? "none" : kiln_c99_diagnostic_detail); return 2; }}
    return result.tag == KILN_VALUE_UNDEFINED ? 0 : 1;
}}
"#,
        symbol = generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "array-string-iterator");
    assert!(
        run.status.success(),
        "an absent ordinary @@iterator key must not expose Symbol.iterator ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );

    let constants = vec![
        Constant::Number(0.0f64.to_bits()),
        Constant::Number(1.0f64.to_bits()),
        Constant::Number(2.0f64.to_bits()),
        Constant::Number(3.0f64.to_bits()),
        Constant::String("a".into()),
        Constant::String("b".into()),
        Constant::String("keys".into()),
        Constant::String("values".into()),
        Constant::String("entries".into()),
    ];
    let array = |code: &mut Vec<Instruction>| {
        code.extend([
            Instruction::Const(1),
            Instruction::NewArray { count: 1 },
            Instruction::Dup,
            Instruction::Const(5),
            Instruction::Const(2),
            Instruction::SetComputed,
            Instruction::Pop,
            Instruction::Dup,
            Instruction::Const(4),
            Instruction::Const(3),
            Instruction::SetComputed,
            Instruction::Pop,
        ]);
    };
    let probes = [
        (6, 2, 4, "keys"),    // Object.keys(array)[2] == "a".
        (7, 2, 3, "values"),  // Object.values(array)[2] == 3.
        (8, 1, 5, "entries"), // Object.entries(array)[1][0] == "b".
    ];
    for (method, index, expected, label) in probes {
        let mut code = vec![Instruction::LoadBuiltin(kiln_ir::Builtin::Object)];
        array(&mut code);
        code.push(Instruction::CallMethod {
            name: method,
            argc: 1,
        });
        code.push(Instruction::Const(index));
        code.push(Instruction::GetComputed);
        if method == 8 {
            code.push(Instruction::Const(0));
            code.push(Instruction::GetComputed);
            code.push(Instruction::Const(5));
        } else {
            code.push(Instruction::Const(expected));
        }
        code.extend([
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::Return,
        ]);
        let generated =
            emit(&program(constants.clone(), code, 0)).expect("array enumeration probe emits");
        let wrapper = format!(
            "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
            generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, label);
        assert!(
            run.status.success(),
            "array {label} omitted named string properties"
        );
    }

    let mut code = Vec::new();
    array(&mut code);
    code.push(Instruction::EnumerateKeys);
    code.push(Instruction::Const(2));
    code.push(Instruction::GetComputed);
    code.push(Instruction::Const(4));
    code.extend([
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ]);
    let generated = emit(&program(constants, code, 0)).expect("for-in probe emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 1; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "array-for-in");
    assert!(
        run.status.success(),
        "for-in omitted named string properties"
    );
}

#[test]
fn executable_array_named_property_is_visible_to_in() {
    let generated = emit(&program(
        vec![
            Constant::Number(1.0f64.to_bits()),
            Constant::String("total".into()),
            Constant::Number(9.0f64.to_bits()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::NewArray { count: 1 },
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::SetComputed,
            Instruction::Pop,
            Instruction::Const(1),
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::In),
            Instruction::Return,
        ],
        1,
    ))
    .expect("array named-property probe emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "array-named-in");
    assert!(
        run.status.success(),
        "the `in` operator must see an array's named property (exit {:?})",
        run.status.code()
    );
}

/// A receiver with no shape has no receiver row. An unclaimed row's key is NULL too, and its
/// columns are the absent sentinel, so a `Date` -- whose payload reads as no shape -- used to find
/// the first empty row and index `slots[UINT32_MAX]` (parallel-builtins issue 18).
#[test]
fn executable_a_receiver_without_a_shape_matches_no_empty_row() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0)).expect("emits");
    let wrapper = format!(
        r#"
int main(void) {{
    const kiln_shape *shapes[4] = {{ NULL, NULL, NULL, NULL }};
    uint32_t slots[4][2] = {{ {{ UINT32_MAX, UINT32_MAX }}, {{ UINT32_MAX, UINT32_MAX }},
                              {{ UINT32_MAX, UINT32_MAX }}, {{ UINT32_MAX, UINT32_MAX }} }};
    if (kiln_c99_recv_row(shapes, &slots[0][0], 2, NULL) != NULL) return 1;
    return 0;
}}
"#
    );
    let run = run_with_main(&generated, &wrapper, "recv-row-null-shape");
    assert!(
        run.status.success(),
        "a NULL shape found a receiver row (exit {:?})",
        run.status.code()
    );
}

#[test]
fn executable_typed_array_views_cover_all_element_kinds_and_aliasing() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0)).expect("emits");
    let wrapper = format!(
        r#"
int main(void) {{
    kiln_value buffer;
    kiln_value bytes;
    kiln_value words;
    kiln_value source;
    kiln_value sub;
    kiln_value iterator;
    kiln_value step;
    kiln_value keys;
    kiln_value prototype;
    kiln_value common;
    kiln_value value;
    kiln_value args[3];
    kiln_value thrown = kiln_c99_undefined();
    kiln_typed_kind kinds[] = {{KILN_TYPED_INT8, KILN_TYPED_UINT8, KILN_TYPED_UINT8_CLAMPED,
        KILN_TYPED_INT16, KILN_TYPED_UINT16, KILN_TYPED_INT32, KILN_TYPED_UINT32,
        KILN_TYPED_FLOAT32, KILN_TYPED_FLOAT64}};
    double inputs[] = {{200.0, 256.0, 300.0, 70000.0, 70000.0, 4294967297.0,
        -1.0, 1.1, 1.1}};
    double expected[] = {{-56.0, 0.0, 255.0, 4464.0, 4464.0, 1.0,
        4294967295.0, 1.1, 1.1}};
    size_t index;
    int found;
    int enumerable;
    /* Headed keys for the renamed `typed_own_key`: the view keeps no copy. */
    static const kiln_string typed_key_0 = {{ 1, KILN_STRING_LATIN1, "0" }};
    static const kiln_string typed_key_length = {{ 6, KILN_STRING_LATIN1, "length" }};
    if (kiln_c99_new_typed_buffer(&buffer, 8) != KILN_C99_OK) return 2;
    if (kiln_c99_new_typed_array(&bytes, KILN_TYPED_UINT8, buffer, 0, 0, 0, &thrown) != KILN_C99_OK) return 3;
    if (kiln_c99_new_typed_array(&words, KILN_TYPED_UINT32, buffer, 0, 0, 0, &thrown) != KILN_C99_OK) return 4;
    if (kiln_c99_typed_array_of(bytes)->length != 8 || kiln_c99_typed_array_of(words)->length != 2) return 5;
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 0, 1.0);
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 1, 2.0);
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 2, 3.0);
    kiln_c99_typed_set(kiln_c99_typed_array_of(bytes), 3, 4.0);
    if (kiln_c99_typed_get(kiln_c99_typed_array_of(words), 0) != 67305985.0) return 6;
    if (kiln_c99_new_typed_array(&source, KILN_TYPED_UINT8, buffer, 0, 0, 0, &thrown) != KILN_C99_OK) return 7;
    /* A second view over bytes 1..3 of the same buffer aliases the first. */
    if (kiln_c99_new_typed_array(&sub, KILN_TYPED_UINT8, buffer, 1, 2, 1, &thrown) != KILN_C99_OK)
        return 8;
    kiln_c99_typed_set(kiln_c99_typed_array_of(sub), 0, 9.0);
    if (kiln_c99_typed_get(kiln_c99_typed_array_of(bytes), 1) != 9.0) return 9;
    for (index = 0; index < sizeof kinds / sizeof *kinds; ++index) {{
        if (kiln_c99_new_array(&source) != KILN_C99_OK ||
            kiln_c99_array_push(source, kiln_c99_number(inputs[index])) != KILN_C99_OK ||
            kiln_c99_new_typed_array(&value, kinds[index], source, 0, 0, 0, &thrown) != KILN_C99_OK) return 10;
        if (kinds[index] == KILN_TYPED_FLOAT32) {{
            if (fabs(kiln_c99_typed_get(kiln_c99_typed_array_of(value), 0) - expected[index]) > 0.000001) return 11;
        }} else if (kiln_c99_typed_get(kiln_c99_typed_array_of(value), 0) != expected[index]) return 12;
    }}
    if (kiln_c99_new_typed_iterator(&iterator, source, 0) != KILN_C99_OK ||
        kiln_c99_call(NULL, &step, kiln_c99_method(KILN_C99_METHOD_ITERATOR_NEXT), iterator,
                      NULL, 0, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_get(NULL, step, "value", 5, &value, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_as_number(value) != inputs[sizeof inputs / sizeof *inputs - 1]) return 13;
    /* ToIndex must reject before an out-of-range floating-to-size_t cast, while NaN is zero. */
    if (kiln_c99_to_index(kiln_c99_number(NAN), &index, &thrown) != KILN_C99_OK || index != 0)
        return 14;
    if (kiln_c99_to_index(kiln_c99_number(-1.0), &index, &thrown) != KILN_C99_THROWN ||
        kiln_c99_to_index(kiln_c99_number(1e300), &index, &thrown) != KILN_C99_THROWN)
        return 15;
    /* Extra offset/length arguments are ignored for an array source without being coerced. */
    if (kiln_c99_new_array(&source) != KILN_C99_OK ||
        kiln_c99_array_push(source, kiln_c99_number(7.0)) != KILN_C99_OK) return 16;
    args[0] = source;
    args[1] = kiln_c99_number(1e300);
    if (kiln_c99_construct(NULL, &value, kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_ARRAY),
                           args, 2, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_typed_array_of(value)->length != 1 ||
        kiln_c99_typed_get(kiln_c99_typed_array_of(value), 0) != 7.0) return 17;
    args[0] = kiln_c99_number(-1.0);
    if (kiln_c99_construct(NULL, &value, kiln_c99_builtin(KILN_C99_BUILTIN_ARRAY_BUFFER),
                           args, 1, 0, &thrown) != KILN_C99_THROWN) return 18;
    /* The view's indices are own and enumerable. Metadata and methods are inherited. */
    kiln_c99_typed_own_key(bytes, &typed_key_0, &found, &enumerable);
    if (!found || !enumerable) return 19;
    kiln_c99_typed_own_key(bytes, &typed_key_length, &found, &enumerable);
    if (found || enumerable) return 20;
    if (kiln_c99_own_keys(NULL, bytes, &keys, 0, NULL) != KILN_C99_OK ||
        kiln_c99_array_of(keys)->count != 8) return 21;
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_UINT8_ARRAY), "prototype", 9,
                     &prototype, 0, &thrown) != KILN_C99_OK ||
        prototype.tag != KILN_VALUE_BUILTIN ||
        prototype.bits != KILN_C99_BUILTIN_UINT8_ARRAY_PROTOTYPE) return 22;
    if (kiln_c99_get(NULL, prototype, "constructor", 11, &value, 0, &thrown) != KILN_C99_OK ||
        value.tag != KILN_VALUE_BUILTIN || value.bits != KILN_C99_BUILTIN_UINT8_ARRAY) return 23;
    if (kiln_c99_get(NULL, prototype, "BYTES_PER_ELEMENT", 17, &value, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_as_number(value) != 1.0) return 24;
    args[0] = bytes;
    if (kiln_c99_invoke_method(NULL, &value, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        value.tag != prototype.tag || value.bits != prototype.bits) return 26;
    args[0] = prototype;
    if (kiln_c99_invoke_method(NULL, &common, KILN_C99_METHOD_OBJECT_GET_PROTOTYPE_OF,
                               kiln_c99_undefined(), args, 1, 0, &thrown) != KILN_C99_OK ||
        common.tag != KILN_VALUE_BUILTIN ||
        common.bits != KILN_C99_BUILTIN_TYPED_ARRAY_PROTOTYPE) return 27;
    if (kiln_c99_get(NULL, kiln_c99_builtin(KILN_C99_BUILTIN_SYMBOL), "toStringTag", 11,
                     &value, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_get_computed(NULL, bytes, value, &step, 0, &thrown) != KILN_C99_OK ||
        step.tag != KILN_VALUE_STRING ||
        !kiln_test_units_equal(step, "Uint8Array", 10)) return 28;
    return 0;
}}
/* {} */
"#,
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "typed-array-core");
    assert!(
        run.status.success(),
        "typed-array C core failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_inspection_skips_symbol_properties_without_separators() {
    let generated = emit(&program(
        vec![
            Constant::String("iterator".into()),
            Constant::Number(1.0f64.to_bits()),
            Constant::String("a".into()),
            Constant::Number(2.0f64.to_bits()),
        ],
        vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Symbol),
            Instruction::GetProperty(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Const(3),
            Instruction::NewObject { count: 2 },
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        0,
    ))
    .expect("symbol inspection probe emits");
    let wrapper = format!(
        r#"
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != UINT16_C(1) || argc != 1 || arguments == NULL || result == NULL ||
        arguments[0].tag != KILN_VALUE_STRING ||
        !kiln_test_units_equal(arguments[0], "{{ a: 2 }}", 8)) return 1;
    *result = kiln_c99_undefined();
    return 0;
}}
int main(void) {{
    kiln_host host = {{host_call, NULL}};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 2;
}}
"#,
        symbol = generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "symbol-inspection");
    assert!(
        run.status.success(),
        "symbol-only properties must not leave malformed inspection separators"
    );
}

#[test]
fn executable_buffer_reuses_uint8_storage_with_node_semantics() {
    let generated = emit(&program(
        vec![
            Constant::String("kiln ".into()),
            Constant::String("streams".into()),
            Constant::String("from".into()),
            Constant::String("concat".into()),
            Constant::String("toString".into()),
            Constant::String("kiln streams".into()),
            Constant::String("isBuffer".into()),
            Constant::Boolean(false),
        ],
        vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Buffer),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::CallMethod { name: 2, argc: 1 },
            Instruction::LoadLocal(0),
            Instruction::Const(1),
            Instruction::CallMethod { name: 2, argc: 1 },
            Instruction::NewArray { count: 2 },
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(2),
            Instruction::CallMethod { name: 3, argc: 1 },
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::CallMethod { name: 4, argc: 0 },
            Instruction::Const(5),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::JumpIfFalse(26),
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::CallMethod { name: 6, argc: 1 },
            Instruction::Return,
            Instruction::Const(7),
            Instruction::Return,
            Instruction::Const(7),
            Instruction::Return,
            Instruction::Const(7),
            Instruction::Return,
        ],
        3,
    ))
    .expect("Buffer probe emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "buffer-facade");
    assert!(
        run.status.success(),
        "native Buffer facade failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn loop_induction_and_string_length_emit_assigned_twins() {
    let proto = FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 1,
        name: Some("test_loop_and_len".into()),
        parameter_count: 1,
        local_count: 3,
        code: vec![
            Instruction::Const(0),
            Instruction::StoreLocal(1),
            Instruction::Const(0),
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(2),
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::LessThan),
            Instruction::JumpIfFalse(21),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(2),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::Const(2),
            Instruction::GetProperty(3),
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(2),
            Instruction::Unary(UnaryOp::Increment),
            Instruction::StoreLocal(2),
            Instruction::Jump(4),
            Instruction::LoadLocal(1),
            Instruction::Return,
        ],
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::Number(0),
            Constant::Number(1),
            Constant::String("hello".into()),
            Constant::String("length".into()),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto],
        template_sites: Vec::new(),
    };
    let generated = emit(&prog).expect("emits");
    assert!(generated.source.contains("str->unit_len"));
}

#[test]
fn object_profile_selects_only_simple_array_parameter_receivers() {
    let make_proto = |declared_length, code| FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length,
        name: Some("array_length".into()),
        inferred_name: None,
        parameter_count: 1,
        local_count: 1,
        code,
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let proto = make_proto(
        1,
        vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::GetComputed,
            Instruction::LoadLocal(0),
            Instruction::GetProperty(1),
            Instruction::Return,
        ],
    );
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::Number(0.0f64.to_bits()),
            Constant::String("length".into()),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };
    let graph = build_function_graph(&proto, &program).unwrap();
    let profile = FunctionTypeProfile {
        index: 0,
        name: Some("array_length".into()),
        parameters: vec!["object".into()],
        return_type: Some("number".into()),
    };
    assert_eq!(
        crate::graph::candidate_array_parameters_with_profile(
            &graph,
            &proto,
            Some(&profile),
            &program,
        ),
        vec![0]
    );
    let pure_indexed = make_proto(
        1,
        vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::GetComputed,
            Instruction::Return,
        ],
    );
    let pure_indexed_program = Program {
        functions: vec![pure_indexed.clone()],
        ..program.clone()
    };
    let pure_indexed_graph = build_function_graph(&pure_indexed, &pure_indexed_program).unwrap();
    assert!(
        crate::graph::candidate_array_parameters_with_profile(
            &pure_indexed_graph,
            &pure_indexed,
            Some(&profile),
            &pure_indexed_program,
        )
        .is_empty(),
        "a pure indexed receiver does not pay for the parameter-array clone"
    );
    assert!(
        crate::graph::candidate_array_parameters_with_profile(&graph, &proto, None, &program)
            .is_empty()
    );

    let reassigned = make_proto(
        1,
        vec![
            Instruction::LoadLocal(0),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(1),
            Instruction::Return,
        ],
    );
    let reassigned_program = Program {
        functions: vec![reassigned.clone()],
        ..program.clone()
    };
    let reassigned_graph = build_function_graph(&reassigned, &reassigned_program).unwrap();
    assert!(
        crate::graph::candidate_array_parameters_with_profile(
            &reassigned_graph,
            &reassigned,
            Some(&profile),
            &reassigned_program,
        )
        .is_empty()
    );

    let emitted = emit_with_policy(
        &program,
        EmissionPolicy {
            type_profile: Some(std::sync::Arc::new(TypeProfile {
                version: 1,
                functions: vec![profile],
                sites: Vec::new(),
            })),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        emitted
            .source
            .contains("Speculation entry guard [parameter_array]")
    );
    assert!(emitted.source.contains("KILN_GC_ARRAY"));
    let function_start = emitted
        .source
        .find("/* function 0: array_length */")
        .expect("array function emitted");
    let function_end = emitted.source[function_start..]
        .find("/* entrypoint */")
        .map(|offset| function_start + offset)
        .unwrap_or(emitted.source.len());
    let function_source = &emitted.source[function_start..function_end];
    assert!(function_source.contains("arr->elements"));
}

#[test]
fn executable_math_and_array_push_optimizations_execute_correctly() {
    // Array push, Math.imul, and String.prototype.charCodeAt
    // const arr = [];
    // arr.push(10);
    // arr.push(20);
    // const m = Math.imul(6, 7); // 42
    // const c = "abc".charCodeAt(1); // 98
    // return (arr[0] + arr[1] + m + c) === (10 + 20 + 42 + 98) === 170
    let constants = vec![
        Constant::String("push".into()),       // 0
        Constant::Number(10.0f64.to_bits()),   // 1
        Constant::Number(20.0f64.to_bits()),   // 2
        Constant::String("imul".into()),       // 3
        Constant::Number(6.0f64.to_bits()),    // 4
        Constant::Number(7.0f64.to_bits()),    // 5
        Constant::String("abc".into()),        // 6
        Constant::String("charCodeAt".into()), // 7
        Constant::Number(1.0f64.to_bits()),    // 8
        Constant::Number(0.0f64.to_bits()),    // 9
        Constant::Number(170.0f64.to_bits()),  // 10
    ];

    let code = vec![
        // arr = []
        Instruction::NewArray { count: 0 },
        Instruction::StoreLocal(0), // arr
        // arr.push(10)
        Instruction::LoadLocal(0),
        Instruction::Const(1), // 10
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Pop,
        // arr.push(20)
        Instruction::LoadLocal(0),
        Instruction::Const(2), // 20
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Pop,
        // m = Math.imul(6, 7)
        Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
        Instruction::Const(4), // 6
        Instruction::Const(5), // 7
        Instruction::CallMethod { name: 3, argc: 2 },
        Instruction::StoreLocal(1), // m
        // c = "abc".charCodeAt(1)
        Instruction::Const(6), // "abc"
        Instruction::Const(8), // 1
        Instruction::CallMethod { name: 7, argc: 1 },
        Instruction::StoreLocal(2), // c
        // sum = arr[0] + arr[1] + m + c
        Instruction::LoadLocal(0),
        Instruction::Const(9), // 0
        Instruction::GetComputed,
        Instruction::LoadLocal(0),
        Instruction::Const(8), // 1
        Instruction::GetComputed,
        Instruction::Binary(BinaryOp::Add),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::Add),
        Instruction::LoadLocal(2),
        Instruction::Binary(BinaryOp::Add),
        Instruction::Const(10), // 170
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 3,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let generated = emit(&program).expect("program emits");
    assert!(
        generated.source.contains("arr->count < arr->capacity"),
        "expected inline push fast path in generated source:\n{}",
        generated.source
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "math-and-push-optimizations");
    assert!(
        run.status.success(),
        "math and push optimizations execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_chained_intrinsic_array_and_string_devirtualization() {
    let constants = vec![
        Constant::Number(10.0f64.to_bits()),    // 0
        Constant::Number(20.0f64.to_bits()),    // 1
        Constant::Number(30.0f64.to_bits()),    // 2
        Constant::Number(40.0f64.to_bits()),    // 3
        Constant::Number(1.0f64.to_bits()),     // 4
        Constant::Number(0.0f64.to_bits()),     // 5
        Constant::Number(5.0f64.to_bits()),     // 6
        Constant::Number(4.0f64.to_bits()),     // 7 ("o" is at index 4 in "hello")
        Constant::String("push".into()),        // 8
        Constant::String("slice".into()),       // 9
        Constant::String("join".into()),        // 10
        Constant::String(",".into()),           // 11
        Constant::String("20,30,40".into()),    // 12
        Constant::String("hello-world".into()), // 13
        Constant::String("substring".into()),   // 14
        Constant::String("indexOf".into()),     // 15
        Constant::String("o".into()),           // 16
        Constant::Boolean(false),               // 17
    ];

    let code = vec![
        // arr = [10, 20, 30]
        Instruction::Const(0),
        Instruction::Const(1),
        Instruction::Const(2),
        Instruction::NewArray { count: 3 },
        Instruction::StoreLocal(0), // arr
        // arr.push(40)
        Instruction::LoadLocal(0),
        Instruction::Const(3), // 40
        Instruction::CallMethod { name: 8, argc: 1 },
        Instruction::Pop,
        // sub = arr.slice(1)
        Instruction::LoadLocal(0),
        Instruction::Const(4), // 1
        Instruction::CallMethod { name: 9, argc: 1 },
        Instruction::StoreLocal(1), // sub
        // joined = sub.join(",")
        Instruction::LoadLocal(1),
        Instruction::Const(11), // ","
        Instruction::CallMethod { name: 10, argc: 1 },
        Instruction::StoreLocal(2), // joined
        // s = "hello-world"
        Instruction::Const(13),
        Instruction::StoreLocal(3), // s
        // part = s.substring(0, 5)
        Instruction::LoadLocal(3),
        Instruction::Const(5), // 0
        Instruction::Const(6), // 5
        Instruction::CallMethod { name: 14, argc: 2 },
        Instruction::StoreLocal(4), // part
        // idx = part.indexOf("o")
        Instruction::LoadLocal(4),
        Instruction::Const(16), // "o"
        Instruction::CallMethod { name: 15, argc: 1 },
        Instruction::StoreLocal(5), // idx
        // check: (joined === "20,30,40") && (idx === 4)
        Instruction::LoadLocal(2),
        Instruction::Const(12),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::JumpIfFalse(36),
        Instruction::LoadLocal(5),
        Instruction::Const(7),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
        Instruction::Const(17),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 6,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let generated = emit(&program).expect("program emits");
    assert!(
        generated
            .source
            .contains("kiln_array *arr = (kiln_array *)KILN_VAL_PTR("),
        "expected direct kiln_array cast in generated source:\n{}",
        generated.source
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "chained-intrinsic-devirtualization");
    assert!(
        run.status.success(),
        "chained intrinsic devirtualization execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}

#[test]
fn executable_math_all_pure_functions_unboxed() {
    // const s = Math.sin(0.0); // 0.0
    // const c = Math.cos(0.0); // 1.0
    // const h = Math.hypot(3.0, 4.0); // 5.0
    // const l = Math.log2(8.0); // 3.0
    // return (s === 0.0) && (c === 1.0) && (h === 5.0) && (l === 3.0);
    let constants = vec![
        Constant::String("sin".into()),     // 0
        Constant::Number(0.0f64.to_bits()), // 1
        Constant::String("cos".into()),     // 2
        Constant::String("hypot".into()),   // 3
        Constant::Number(3.0f64.to_bits()), // 4
        Constant::Number(4.0f64.to_bits()), // 5
        Constant::String("log2".into()),    // 6
        Constant::Number(8.0f64.to_bits()), // 7
        Constant::Number(1.0f64.to_bits()), // 8
        Constant::Number(5.0f64.to_bits()), // 9
    ];

    let code = vec![
        // s = Math.sin(0.0)
        Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
        Instruction::Const(1),
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Const(1),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(0),
        // c = Math.cos(0.0)
        Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
        Instruction::Const(1),
        Instruction::CallMethod { name: 2, argc: 1 },
        Instruction::Const(8),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(1),
        // h = Math.hypot(3.0, 4.0)
        Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
        Instruction::Const(4),
        Instruction::Const(5),
        Instruction::CallMethod { name: 3, argc: 2 },
        Instruction::Const(9),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(2),
        // l = Math.log2(8.0)
        Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
        Instruction::Const(7),
        Instruction::CallMethod { name: 6, argc: 1 },
        Instruction::Const(4),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(3),
        // s && c && h && l (all are boolean true)
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(2),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(3),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 4,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let generated = emit(&program).expect("program emits");
    assert!(
        generated.source.contains("sin(") && generated.source.contains("hypot("),
        "expected inline sin and hypot in generated source:\n{}",
        generated.source
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "math-pure-unboxed");
    assert!(
        run.status.success(),
        "math pure unboxed execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_static_builtin_predicates_array_is_array() {
    let constants = vec![
        Constant::String("isArray".into()),  // 0
        Constant::Number(42.0f64.to_bits()), // 1
        Constant::String("hello".into()),    // 2
        Constant::Boolean(true),             // 3
        Constant::Null,                      // 4
    ];

    let code = vec![
        // r0 = Array.isArray([]) -> true
        Instruction::LoadBuiltin(kiln_ir::Builtin::Array),
        Instruction::NewArray { count: 0 },
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::StoreLocal(0),
        // r1 = Array.isArray(42) -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Array),
        Instruction::Const(1),
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(1),
        // r2 = Array.isArray("hello") -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Array),
        Instruction::Const(2),
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(2),
        // r3 = Array.isArray(true) -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Array),
        Instruction::Const(3),
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(3),
        // r4 = Array.isArray(null) -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Array),
        Instruction::Const(4),
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(4),
        // r0 && r1 && r2 && r3 && r4
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(2),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(3),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(4),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 5,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let generated = emit(&program).expect("program emits");
    assert!(
        generated.source.contains("KILN_C99_BUILTIN_ARRAY"),
        "expected KILN_C99_BUILTIN_ARRAY in generated source:\n{}",
        generated.source
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "array-is-array");
    assert!(
        run.status.success(),
        "array isArray execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_static_builtin_predicates_number() {
    let constants = vec![
        Constant::String("isFinite".into()),               // 0
        Constant::Number(100.0f64.to_bits()),              // 1
        Constant::Number(f64::INFINITY.to_bits()),         // 2
        Constant::String("isNaN".into()),                  // 3
        Constant::Number(f64::NAN.to_bits()),              // 4
        Constant::String("isInteger".into()),              // 5
        Constant::Number(42.5f64.to_bits()),               // 6
        Constant::String("isSafeInteger".into()),          // 7
        Constant::Number(9007199254740991.0f64.to_bits()), // 8: 2^53 - 1
        Constant::Number(9007199254740992.0f64.to_bits()), // 9: 2^53
        Constant::String("42".into()),                     // 10: string "42"
    ];

    let code = vec![
        // r0 = Number.isFinite(100) -> true
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(1),
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::StoreLocal(0),
        // r1 = Number.isFinite(Infinity) -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(2),
        Instruction::CallMethod { name: 0, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(1),
        // r2 = Number.isNaN(NaN) -> true
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(4),
        Instruction::CallMethod { name: 3, argc: 1 },
        Instruction::StoreLocal(2),
        // r3 = Number.isNaN(100) -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(1),
        Instruction::CallMethod { name: 3, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(3),
        // r4 = Number.isInteger(100) -> true
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(1),
        Instruction::CallMethod { name: 5, argc: 1 },
        Instruction::StoreLocal(4),
        // r5 = Number.isInteger(42.5) -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(6),
        Instruction::CallMethod { name: 5, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(5),
        // r6 = Number.isSafeInteger(9007199254740991) -> true
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(8),
        Instruction::CallMethod { name: 7, argc: 1 },
        Instruction::StoreLocal(6),
        // r7 = Number.isSafeInteger(9007199254740992) -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(9),
        Instruction::CallMethod { name: 7, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(7),
        // r8 = Number.isInteger("42") -> false
        Instruction::LoadBuiltin(kiln_ir::Builtin::Number),
        Instruction::Const(10),
        Instruction::CallMethod { name: 5, argc: 1 },
        Instruction::Unary(UnaryOp::Not),
        Instruction::StoreLocal(8),
        // Check all are true: r0 && r1 && r2 && r3 && r4 && r5 && r6 && r7 && r8
        Instruction::LoadLocal(0),
        Instruction::LoadLocal(1),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(2),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(3),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(4),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(5),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(6),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(7),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(8),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 9,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let generated = emit(&program).expect("program emits");
    assert!(
        generated.source.contains("KILN_C99_BUILTIN_NUMBER"),
        "expected KILN_C99_BUILTIN_NUMBER in generated source:\n{}",
        generated.source
    );
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "number-predicates");
    assert!(
        run.status.success(),
        "number predicates execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn executable_array_indexing_and_bounds() {
    let constants = vec![
        Constant::Number(100.0f64.to_bits()), // 0: 100
        Constant::Number(200.0f64.to_bits()), // 1: 200
        Constant::Number(0.0f64.to_bits()),   // 2: 0
        Constant::Number(1.0f64.to_bits()),   // 3: 1
        Constant::Number(999.0f64.to_bits()), // 4: 999
        Constant::Number(5.0f64.to_bits()),   // 5: 5 (out of bounds)
        Constant::Undefined,                  // 6: undefined
    ];

    let code = vec![
        // arr = [100, 200]
        Instruction::Const(0),
        Instruction::Const(1),
        Instruction::NewArray { count: 2 },
        Instruction::StoreLocal(0),
        // r1 = arr[0] (should be 100)
        Instruction::LoadLocal(0),
        Instruction::Const(2),
        Instruction::GetComputed,
        Instruction::StoreLocal(1),
        // check1 = (r1 === 100)
        Instruction::LoadLocal(1),
        Instruction::Const(0),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(2),
        // arr[1] = 999
        Instruction::LoadLocal(0),
        Instruction::Const(3),
        Instruction::Const(4),
        Instruction::SetComputed,
        Instruction::Pop,
        // r3 = arr[1] (should be 999)
        Instruction::LoadLocal(0),
        Instruction::Const(3),
        Instruction::GetComputed,
        Instruction::StoreLocal(3),
        // check2 = (r3 === 999)
        Instruction::LoadLocal(3),
        Instruction::Const(4),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(4),
        // r5 = arr[5] (out-of-bounds, should be undefined)
        Instruction::LoadLocal(0),
        Instruction::Const(5),
        Instruction::GetComputed,
        Instruction::StoreLocal(5),
        // check3 = (r5 === undefined)
        Instruction::LoadLocal(5),
        Instruction::Const(6),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::StoreLocal(6),
        // result = check1 && check2 && check3
        Instruction::LoadLocal(2),
        Instruction::LoadLocal(4),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::LoadLocal(6),
        Instruction::Binary(BinaryOp::StrictEqual),
        Instruction::Return,
    ];

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count: 7,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let generated = emit(&program).expect("array indexing program emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; return (result.tag == KILN_VALUE_BOOLEAN && result.bits == 1) ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "array-indexing");
    assert!(
        run.status.success(),
        "array indexing execution failed ({:?}):\nstdout: {}\nstderr: {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
