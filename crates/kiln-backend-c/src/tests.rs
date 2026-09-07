use super::*;
use kiln_ir::Builtin;
use std::io::Write as _;
use std::process::{Command, Stdio};

#[test]
fn default_emission_policy_is_inert_and_host_clock_changes_identity() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };
    let legacy = emit(&program).expect("default program emits");
    let explicit_default =
        emit_with_policy(&program, EmissionPolicy::default()).expect("default policy emits");
    let host_clock = emit_with_policy(
        &program,
        EmissionPolicy {
            host_clock: true,
            ..EmissionPolicy::default()
        },
    )
    .expect("host-clock policy emits");
    let seeded = emit_with_policy(
        &program,
        EmissionPolicy {
            entropy: EmissionEntropyPolicy::Seeded(7),
            ..EmissionPolicy::default()
        },
    )
    .expect("seeded policy emits");
    let host_entropy = emit_with_policy(
        &program,
        EmissionPolicy {
            entropy: EmissionEntropyPolicy::Host,
            ..EmissionPolicy::default()
        },
    )
    .expect("host entropy policy emits");
    let named_timezone = emit_with_policy(
        &program,
        EmissionPolicy {
            timezone: EmissionStringPolicy::Named("America/Denver".into()),
            ..EmissionPolicy::default()
        },
    )
    .expect("named timezone policy emits");
    let host_timezone = emit_with_policy(
        &program,
        EmissionPolicy {
            timezone: EmissionStringPolicy::Host,
            ..EmissionPolicy::default()
        },
    )
    .expect("host timezone policy emits");
    let named_locale = emit_with_policy(
        &program,
        EmissionPolicy {
            locale: EmissionStringPolicy::Named("de-DE".into()),
            ..EmissionPolicy::default()
        },
    )
    .expect("named locale policy emits");
    let host_locale = emit_with_policy(
        &program,
        EmissionPolicy {
            locale: EmissionStringPolicy::Host,
            ..EmissionPolicy::default()
        },
    )
    .expect("host locale policy emits");

    assert_eq!(legacy, explicit_default);
    assert_ne!(legacy.symbol, host_clock.symbol);
    assert_ne!(legacy.program_hash, host_clock.program_hash);
    assert!(!legacy.source.contains("#define KILN_REALM_HOST_CLOCK"));
    assert!(
        host_clock
            .source
            .contains("#define KILN_REALM_HOST_CLOCK 1")
    );
    assert!(host_clock.source.contains("UINT16_C(2)"));
    assert_ne!(legacy.symbol, seeded.symbol);
    assert_ne!(seeded.symbol, host_entropy.symbol);
    assert!(
        seeded
            .source
            .contains("#define KILN_REALM_ENTROPY_SEED UINT64_C(7)")
    );
    assert!(
        host_entropy
            .source
            .contains("#define KILN_REALM_HOST_ENTROPY 1")
    );
    assert!(host_entropy.source.contains("UINT16_C(3)"));
    assert_ne!(legacy.symbol, named_timezone.symbol);
    assert_ne!(named_timezone.symbol, host_timezone.symbol);
    assert_ne!(legacy.symbol, named_locale.symbol);
    assert_ne!(named_locale.symbol, host_locale.symbol);
    assert_ne!(named_timezone.symbol, named_locale.symbol);
}

#[test]
fn runtime_module_materializer_stays_inside_the_backend_value_abi() {
    let module = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
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
fn decoupled_modules_include_one_external_runtime_header() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };
    let policy = EmissionPolicy {
        decoupled_prelude: true,
        ..EmissionPolicy::default()
    };
    let emitted = emit_with_policy(&program, policy).expect("decoupled module emits");

    assert!(
        emitted
            .source
            .contains(&format!("#include \"{RUNTIME_PRELUDE_HEADER_NAME}\"")),
        "the module must name the header materialized by kiln-native"
    );
    assert!(!emitted.source.contains("enum kiln_value_tag"));
    assert!(
        emitted.source.len() < runtime_prelude_header().len(),
        "a decoupled module must not carry a copy of the runtime header"
    );
    assert!(
        !runtime_prelude_header().contains("static uint32_t kiln_c99_method_groups("),
        "the runtime-only method classifier must not be parsed by every module"
    );
    assert!(
        runtime_prelude_source().contains("uint32_t kiln_c99_method_groups("),
        "the shared prelude must retain the method classifier implementation"
    );
}

#[test]
fn compact_emission_defers_regex_compilation_to_the_provider() {
    let program = program(
        vec![Constant::String("a+".into()), Constant::String("g".into())],
        vec![
            Instruction::NewRegExp {
                pattern: 0,
                flags: 1,
            },
            Instruction::Return,
        ],
        0,
    );
    let normal = emit(&program).expect("normal regexp emits");
    let compact = emit_with_policy(
        &program,
        EmissionPolicy {
            compact: true,
            ..EmissionPolicy::default()
        },
    )
    .expect("compact regexp emits");

    assert!(normal.source.contains(&format!("{}_re_0_1", normal.symbol)));
    assert!(
        !compact
            .source
            .contains(&format!("{}_re_0_1", compact.symbol))
    );
    assert!(compact.source.contains(&format!(
        "kiln_c99_new_regexp(&built, {}_constant_0",
        compact.symbol
    )));
}

#[test]
fn executable_namespace_wrapper_materializes_sorted_immutable_namespace() {
    let module = ModuleProgram {
        program: Program {
            version: BYTECODE_VERSION,
            source: String::new().into(),
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 0,
            functions: Vec::new(),
            template_sites: Vec::new(),
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
    if (kiln_c99_object_of(namespace_object)->count != 3 ||
        strcmp(kiln_c99_object_of(namespace_object)->properties[0].key, "a") != 0 ||
        strcmp(kiln_c99_object_of(namespace_object)->properties[1].key, "z") != 0 ||
        strcmp(kiln_c99_object_of(namespace_object)->properties[2].key, "@@toStringTag") != 0)
        return 6;
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
        read.tag != KILN_VALUE_STRING || read.len != 6 || memcmp(read.ptr, "Module", 6) != 0)
        return 13;
    if (kiln_c99_put_mode(NULL, namespace_object, "a", 1, kiln_c99_number(3.0), 1, 0,
                          &thrown) != KILN_C99_THROWN)
        return 14;
    if (kiln_c99_get(NULL, thrown, "message", 7, &read, 0, &thrown) != KILN_C99_OK ||
        read.tag != KILN_VALUE_STRING || strstr(read.ptr, "module namespace") == NULL) return 15;
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
        strcmp(kiln_c99_object_of(numeric_namespace)->properties[0].key, "10") != 0 ||
        strcmp(kiln_c99_object_of(numeric_namespace)->properties[1].key, "2") != 0)
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
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
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
    if (kiln_c99_make_closure(&callback, listener, "listener", 8, NULL, 0, "<test>", 0, 1, 0, 0, 0, 0, 0,
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
    if (kiln_c99_make_closure(&child, child_constructor, "Child", 5, NULL, 0, "<test>", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 34;
    args[0] = child; args[1] = exports[0];
    if (kiln_c99_node_util(NULL, &result, KILN_C99_METHOD_NODE_UTIL_INHERITS, args, 2, 0,
                           &thrown) != KILN_C99_OK) return 35;
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
int main(void) {
    uint64_t realm;
    kiln_value readable = kiln_c99_method(KILN_C99_METHOD_NODE_READABLE);
    kiln_value child;
    kiln_value result;
    kiln_value instance;
    kiln_value method;
    kiln_value readable_prototype;
    kiln_value emitter_prototype;
    kiln_value arguments[2];
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
    if (kiln_c99_make_closure(&child, readable_child, "Child", 5, "<test>", 6, "<test>", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 6;
    arguments[0] = child;
    arguments[1] = readable;
    if (kiln_c99_node_util(NULL, &result, KILN_C99_METHOD_NODE_UTIL_INHERITS, arguments, 2, 0,
                           &thrown) != KILN_C99_OK) return 7;
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
                Constant::String("valueOf".to_owned()),
                Constant::String("called".to_owned()),
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
        arguments[0].tag != KILN_VALUE_STRING || arguments[0].len != 6 ||
        memcmp(arguments[0].ptr, "called", 6) != 0) return 90;
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
    if (recovered.tag != KILN_VALUE_STRING || recovered.len != key.len ||
        memcmp(recovered.ptr, key.ptr, key.len) != 0) return 4;
    if (kiln_c99_new_symbol(&unique, key) != KILN_C99_OK) return 5;
    if (kiln_c99_strict_equal(first, unique)) return 6;
    if (kiln_c99_symbol_key_for(unique).tag != KILN_VALUE_UNDEFINED) return 7;
    if (kiln_c99_property_key(first).len == key.len &&
        memcmp(kiln_c99_property_key(first).ptr, key.ptr, key.len) == 0) return 8;
    if (kiln_c99_text(first).len != 29 ||
        memcmp(kiln_c99_text(first).ptr, "Symbol(fastify-error-generic)", 29) != 0) return 9;
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

#[test]
fn logical_path_diagnostics_name_source_and_operation() {
    let module = ModuleProgram {
        program: program(
            vec![Constant::String("defineProperty".into())],
            vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Object),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::Return,
            ],
            0,
        ),
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let generated = emit_module_with_logical_path(&module, "node_modules/@fastify/error/index.js")
        .expect("logical module emits");
    assert!(
            generated
                .source
                .contains(
                    "kiln_c99_note_refusal(\"node_modules/@fastify/error/index.js\", (size_t)1, \"CallMethod(defineProperty)\", (size_t)kiln_c99_diagnostic_active_depth)"
                )
        );
    assert!(
        !generated
            .source
            .contains("kiln_c99_diagnostic_site = kiln_c99_diagnostic_site"),
        "diagnostic reset must not be substituted into the site expression"
    );
    assert!(
        generated
            .source
            .contains("kiln_c99_diagnostic_detail = NULL;")
    );
    assert!(
        generated
            .source
            .contains("Object.defineProperty invalid key or arity")
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
            &["KILN_VALUE_OBJECT", "KILN_VALUE_ARRAY"][..],
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
#[ignore = "ADR 0036 migration gate: value-kernel state still ships in every module"]
fn emitted_modules_do_not_own_realm_value_kernel_state() {
    let generated =
        emit(&program(vec![], vec![Instruction::Return], 0)).expect("empty program emits");
    for declaration in [
        "static kiln_value kiln_c99_globals;",
        "static kiln_value kiln_c99_process;",
        "static kiln_value kiln_c99_atomics;",
        "static kiln_value *kiln_c99_symbols;",
        "static kiln_method_store *kiln_c99_fixture_method_stores;",
        "static kiln_native_constructor kiln_c99_native_constructors[]",
    ] {
        assert!(
            !generated.source.contains(declaration),
            "realm state remains private to one generated translation unit: {declaration}"
        );
    }
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
        if pending.ends_with(';') {
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
            let name_start = line.find("kiln_c99_")?;
            let before_name = &line[..name_start];
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
            let tail = &line[name_start..];
            let end = tail
                .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .unwrap_or(tail.len());
            Some(tail[..end].to_owned())
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
        "mutable generated-C state must say whether the realm, dynamic frame, or test fixture owns it"
    );
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
fn structural_instruction_refusals_always_name_their_owner() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("structural refusal probe emits");
    assert!(
        generated.source.contains("kiln_c99_note_refusal"),
        "runtime must include the refusal provenance boundary"
    );
    let emitter = include_str!("tail.rs");
    let templates = emitter
        .split("fn emit_tail_instruction(")
        .nth(1)
        .expect("tail emitter")
        .split("match instruction {")
        .nth(1)
        .expect("instruction match")
        .split("let refusal = format!(")
        .next()
        .expect("instruction templates");
    assert!(
        !templates
            .lines()
            .any(|line| line.contains("return KILN_C99_CONTROL_ERROR;")),
        "instruction templates must route structural refusals through direct_refusal"
    );
}

#[test]
// `f16round` rather than an invented name: an absent name on a table-driven built-in is
// `undefined` now, exactly as the language says, so a made-up one no longer refuses and this
// probe would have been testing nothing. `Math.f16round` is a real member this backend has
// not built, so it is in the backlog and still refuses by name -- which is the provenance
// this test is about.
fn nested_call_refusal_preserves_the_failing_instruction_without_blaming_the_callee() {
    let generated = emit(&Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![Constant::String("f16round".into())],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![FunctionProto {
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("read_unimplemented_math_property".into()),
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![
                Instruction::LoadBuiltin(Builtin::Math),
                Instruction::GetProperty(0),
                Instruction::Return,
            ],
            upvalues: Vec::new(),
            type_annotations: None,
        }],
        template_sites: Vec::new(),
    })
    .expect("nested refusal probe emits");
    let wrapper = format!(
        r#"
int main(void) {{
    kiln_value result = kiln_c99_undefined();
    int status = {symbol}(NULL, &result);
    if (status != KILN_C99_CONTROL_ERROR) return 1;
    if (kiln_c99_diagnostic_site == NULL || kiln_c99_diagnostic_operation == NULL) return 2;
    if (kiln_c99_diagnostic_instruction != (size_t)1) return 3;
    if (strcmp(kiln_c99_diagnostic_operation, "GetProperty(f16round)") != 0) return 4;
    if (kiln_c99_diagnostic_detail == NULL ||
        strcmp(kiln_c99_diagnostic_detail,
               "a member the language puts on this built-in and this backend has not built")
            != 0) return 5;
    if (kiln_c99_diagnostic_receiver_tag != KILN_VALUE_BUILTIN) return 6;
    return 0;
}}
"#,
        symbol = generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "nested-call-refusal-provenance");
    assert!(
        run.status.success(),
        "nested refusal provenance failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn unowned_function_refusal_names_its_generated_source_and_function() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("diagnostic source probe emits");
    let wrapper = r#"
static int refuses_before_instruction(kiln_call_context *ctx, kiln_val *result,
                                      kiln_val *upvalues, const kiln_val *receiver_ptr,
                                      const kiln_val *args, size_t argc) {
    (void)ctx; (void)result; (void)upvalues; (void)receiver_ptr; (void)args; (void)argc;
    return KILN_C99_CONTROL_ERROR;
}
int main(void) {
    uint64_t realm;
    kiln_value function;
    kiln_value result = kiln_c99_undefined();
    kiln_value thrown = kiln_c99_undefined();
    kiln_value diagnostic;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&function, refuses_before_instruction, "initialize", 10,
                              NULL, 0, "node_modules/example/index.js", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 1;
    if (kiln_c99_call(NULL, &result, function, kiln_c99_undefined(), NULL, 0, 0,
                      &thrown) != KILN_C99_CONTROL_ERROR) return 2;
    diagnostic = kiln_c99_control_diagnostic();
    if (diagnostic.tag != KILN_VALUE_STRING) return 3;
    if (strstr(diagnostic.ptr, "module=node_modules/example/index.js") == NULL) return 4;
    if (strstr(diagnostic.ptr, "operation=FunctionBody") == NULL) return 5;
    if (strstr(diagnostic.ptr, "function=initialize") == NULL) return 6;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 7;
}
"#;
    let run = run_with_main(&generated, wrapper, "function-refusal-source");
    assert!(
        run.status.success(),
        "function refusal source failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
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

fn program(constants: Vec<Constant>, code: Vec<Instruction>, local_count: u32) -> Program {
    Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants,
        code,
        local_count,
        functions: Vec::new(),
        template_sites: Vec::new(),
    }
}

fn module(
    program: Program,
    imports: Vec<kiln_ir::ImportBinding>,
    exports: Vec<kiln_ir::ExportBinding>,
) -> ModuleProgram {
    ModuleProgram {
        program,
        dependencies: Vec::new(),
        imports,
        exports,
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    }
}

/// The engine half of the provider ABI, for tests that link the vendored C number runtime.
///
/// `kiln-native-number`'s C is a *test fixture*: production links the Rust provider archive,
/// which defines these. Tests here exercise the emitted C's own logic and none of them
/// compiles a pattern or does arbitrary-precision arithmetic -- so a fixture that refuses
/// every one is honest, and a test that did need one would get a visible error rather than a
/// quiet wrong answer. Such a test belongs in `conformance/native`, where the real archive is
/// linked.
const REGEXP_FIXTURE: &str = r#"
int32_t kiln_regexp_compile(const unsigned char *pattern, size_t pattern_len,
                            const unsigned char *flags, size_t flags_len, void **out_handle) {
    (void)pattern; (void)pattern_len; (void)flags; (void)flags_len; (void)out_handle;
    return -1;
}
void kiln_regexp_free(void *handle) { (void)handle; }
int32_t kiln_regexp_search(void *handle, const uint16_t *subject, size_t subject_len, size_t start,
                           int64_t *out_ranges, size_t out_capacity) {
    (void)handle; (void)subject; (void)subject_len; (void)start; (void)out_ranges;
    (void)out_capacity;
    return -1;
}
size_t kiln_regexp_group_count(const void *handle) { (void)handle; return 0; }
int32_t kiln_regexp_group_name(const void *handle, size_t index, unsigned char *buffer,
                               size_t capacity) {
    (void)handle; (void)index; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_bigint_parse(const unsigned char *text, size_t len, uint32_t radix, void **out) {
    (void)text; (void)len; (void)radix; (void)out;
    return -1;
}
int32_t kiln_bigint_from_double(double value, void **out) { (void)value; (void)out; return -1; }
void kiln_bigint_free(void *handle) { (void)handle; }
double kiln_bigint_to_double(const void *handle) { (void)handle; return 0.0; }
int32_t kiln_bigint_to_string(const void *handle, uint32_t radix, unsigned char *buffer,
                              size_t capacity) {
    (void)handle; (void)radix; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_bigint_binary(uint32_t operation, const void *left, const void *right, void **out) {
    (void)operation; (void)left; (void)right; (void)out;
    return -1;
}
int32_t kiln_bigint_negate(const void *handle, void **out) { (void)handle; (void)out; return -1; }
int32_t kiln_bigint_compare(const void *left, const void *right) {
    (void)left; (void)right;
    return 0;
}
int32_t kiln_bigint_is_zero(const void *handle) { (void)handle; return 1; }
int32_t kiln_bigint_to_i128(const void *handle, int64_t *out_high, uint64_t *out_low) {
    (void)handle; (void)out_high; (void)out_low;
    return -1;
}
int32_t kiln_temporal_from_fields(uint32_t kind, const double *fields, size_t field_count,
                                  const unsigned char *text, const int64_t *text_lengths,
                                  size_t text_count, const unsigned char *option_text,
                                  const int64_t *option_text_lengths, size_t option_text_count,
                                  void **out_handle) {
    (void)kind; (void)fields; (void)field_count; (void)text; (void)text_lengths;
    (void)text_count; (void)option_text; (void)option_text_lengths; (void)option_text_count;
    (void)out_handle;
    return -1;
}
int32_t kiln_temporal_duration_from_fields(const double *fields, size_t field_count,
                                           void **out_handle) {
    (void)fields; (void)field_count; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_construct(uint32_t kind, const double *fields, size_t field_count,
                                const unsigned char *text, const int64_t *text_lengths,
                                size_t text_count, int64_t epoch_high, uint64_t epoch_low,
                                void **out_handle) {
    (void)kind; (void)fields; (void)field_count; (void)text; (void)text_lengths;
    (void)text_count; (void)epoch_high; (void)epoch_low; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_parse(uint32_t kind, const unsigned char *bytes, size_t len,
                            const unsigned char *option_text,
                            const int64_t *option_text_lengths, size_t option_text_count,
                            void **out_handle) {
    (void)kind; (void)bytes; (void)len; (void)option_text; (void)option_text_lengths;
    (void)option_text_count; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_now(uint32_t kind, int64_t high, uint64_t low, const unsigned char *zone,
                          size_t zone_len, void **out_handle) {
    (void)kind; (void)high; (void)low; (void)zone; (void)zone_len; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_instant_from_epoch_milliseconds(double milliseconds, void **out_handle) {
    (void)milliseconds; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_instant_from_epoch_nanoseconds(int64_t high, uint64_t low,
                                                     void **out_handle) {
    (void)high; (void)low; (void)out_handle;
    return -1;
}
void kiln_temporal_free(void *handle) { (void)handle; }
int32_t kiln_temporal_kind(const void *handle) { (void)handle; return -1; }
int32_t kiln_temporal_get(const void *handle, const unsigned char *name, size_t name_len,
                          double *out_number, unsigned char *buffer, size_t capacity,
                          int32_t *out_length) {
    (void)handle; (void)name; (void)name_len; (void)out_number; (void)buffer; (void)capacity;
    (void)out_length;
    return -1;
}
int32_t kiln_temporal_to_string(const void *handle, unsigned char *buffer, size_t capacity) {
    (void)handle; (void)buffer; (void)capacity;
    return -1;
}
int32_t kiln_temporal_compare(const void *left, const void *right, const void *relative_to,
                              const unsigned char *option_text,
                              const int64_t *option_text_lengths, size_t option_text_count,
                              int32_t *out_order) {
    (void)left; (void)right; (void)relative_to; (void)option_text; (void)option_text_lengths;
    (void)option_text_count; (void)out_order;
    return -1;
}
int32_t kiln_temporal_to_kind(const void *handle, uint32_t kind, void **out_handle) {
    (void)handle; (void)kind; (void)out_handle;
    return -1;
}
int32_t kiln_temporal_last_error(int32_t *out_kind, unsigned char *buffer, size_t capacity) {
    (void)out_kind; (void)buffer; (void)capacity;
    return -1;
}
/* `JSON.parse`'s reader lives in the Rust provider archive too, and this host does not link it.
 * Refusing every document is honest for the same reason the regexp stub is: no test here parses
 * one, and a test that did would see a SyntaxError rather than a quiet wrong answer. */
struct kiln_json_node;
int32_t kiln_json_parse(const unsigned char *bytes, size_t len,
                        const struct kiln_json_node **out_nodes, size_t *out_count,
                        const unsigned char **out_text, size_t *out_text_len) {
    (void)bytes; (void)len; (void)out_nodes; (void)out_count; (void)out_text; (void)out_text_len;
    return -1;
}
"#;

/// Link-only Realm provider for backend unit tests.
///
/// Scheduler behavior is covered through the real Rust archive in `kiln-native`; these tests
/// exercise generated scalar C directly. The fixture therefore owns and disposes any work it
/// is unexpectedly given without duplicating a second event loop in the backend test module.
const REALM_FIXTURE: &str = r#"
static uint64_t kiln_test_next_timer_id = 1;
static int kiln_test_realm_active = 0;
static uint64_t kiln_test_symbol_id = 1;
static char kiln_test_symbol_key[256];
static uint64_t kiln_test_symbol_key_len = 0;
static uint32_t kiln_test_callable_tags[256];
static uint64_t kiln_test_callable_codes[256];
static void *kiln_test_callable_stores[256];
static size_t kiln_test_callable_store_count = 0;
static kiln_value kiln_test_value_slots[5];
static uint64_t kiln_test_entropy_seed = 0;
static uint64_t kiln_test_entropy_state = 0;
static int kiln_test_entropy_initialized = 0;

int kiln_realm_create(uint64_t *out_realm) {
    if (out_realm == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_realm = 1;
    return KILN_C99_OK;
}
int kiln_realm_enter(uint64_t realm, kiln_realm_entry_fn entry, void *context) {
    int status;
    if (realm != 1 || entry == NULL || kiln_test_realm_active) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    kiln_test_realm_active = 1;
    status = entry(context);
    kiln_test_realm_active = 0;
    return status;
}
int kiln_realm_drive(uint64_t realm, void *host, void *thrown) {
    (void)host;
    (void)thrown;
    return realm == 1 ? KILN_C99_OK : KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_destroy(uint64_t realm) {
    if (realm != 1 || kiln_test_realm_active) return KILN_C99_INVALID_ARGUMENT;
    return KILN_C99_OK;
}
/* Here rather than beside the other provider stubs, because its argument is a struct the runtime
 * prelude declares and this fixture is the first text that follows the prelude. */
int32_t kiln_temporal_invoke(const kiln_temporal_call *call) { (void)call; return -1; }
int kiln_realm_is_active(void) { return kiln_test_realm_active; }
int kiln_realm_diagnostic_reset(void) { return KILN_C99_OK; }
int kiln_realm_diagnostic_note(const uint8_t *site, uint64_t site_len, uint64_t instruction,
                               const uint8_t *operation, uint64_t operation_len, uint64_t depth,
                               const uint8_t *detail, uint64_t detail_len,
                               const uint8_t *function, uint64_t function_len,
                               uint64_t method, uint64_t receiver_tag, int has_method) {
    (void)site; (void)site_len; (void)instruction; (void)operation; (void)operation_len;
    (void)depth; (void)detail; (void)detail_len; (void)function; (void)function_len;
    (void)method; (void)receiver_tag; (void)has_method;
    return KILN_C99_OK;
}
int kiln_realm_diagnostic_render(uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
/* `node:path`'s decompositions come from the Rust provider, which this test host does not link:
 * the backend's own tests compile generated C against a shim, not against the shipped archive.
 * Refusing here is the honest answer -- a second implementation living in the test host is exactly
 * the drift the provider exists to end. `kiln test conformance/*` and `scripts/native.sh` run
 * against the real archive and are where these are exercised. */
int kiln_node_path_part(uint32_t part, const uint8_t *path, uint64_t path_len,
                        const uint8_t *suffix, uint64_t suffix_len, uint8_t *out,
                        uint64_t capacity, uint64_t *out_len) {
    (void)part; (void)path; (void)path_len; (void)suffix; (void)suffix_len; (void)out;
    (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_node_path_relative(const uint8_t *from, uint64_t from_len, const uint8_t *to,
                            uint64_t to_len, uint8_t *out, uint64_t capacity, uint64_t *out_len) {
    (void)from; (void)from_len; (void)to; (void)to_len; (void)out; (void)capacity;
    if (out_len == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_len = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_open(const uint8_t *path, uint64_t path_len, uint64_t *out_library) {
    (void)path; (void)path_len; (void)out_library; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_close(uint64_t library) { (void)library; return KILN_C99_INVALID_ARGUMENT; }
int kiln_ffi_prepare(uint64_t library, const uint8_t *symbol, uint64_t symbol_len,
                     const uint32_t *argument_types, uint64_t argument_count,
                     uint32_t result_type, uint64_t *out_function) {
    (void)library; (void)symbol; (void)symbol_len; (void)argument_types;
    (void)argument_count; (void)result_type; (void)out_function;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_call(uint64_t function, const kiln_ffi_value *arguments,
                  uint64_t argument_count, kiln_ffi_value *out_result) {
    (void)function; (void)arguments; (void)argument_count; (void)out_result;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_register_callback(const uint32_t *argument_types, uint64_t argument_count,
                               uint32_t result_type, kiln_ffi_dispatch_fn dispatch,
                               void *context, uint64_t *out_pointer) {
    (void)argument_types; (void)argument_count; (void)result_type; (void)dispatch;
    (void)context; (void)out_pointer; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_unregister_callback(uint64_t pointer) {
    (void)pointer; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_ffi_last_error(const uint8_t **out_bytes, uint64_t *out_len) {
    if (out_bytes != NULL) *out_bytes = NULL;
    if (out_len != NULL) *out_len = 0;
    return KILN_C99_OK;
}
int kiln_realm_symbol_for(const uint8_t *key, uint64_t key_len, uint64_t *out_id) {
    if (!kiln_test_realm_active || out_id == NULL || key_len > sizeof kiln_test_symbol_key) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (kiln_test_symbol_key_len == 0) {
        memcpy(kiln_test_symbol_key, key, (size_t)key_len);
        kiln_test_symbol_key_len = key_len;
    } else if (key_len != kiln_test_symbol_key_len ||
               memcmp(kiln_test_symbol_key, key, (size_t)key_len) != 0) {
        ++kiln_test_symbol_id;
        memcpy(kiln_test_symbol_key, key, (size_t)key_len);
        kiln_test_symbol_key_len = key_len;
    }
    *out_id = kiln_test_symbol_id;
    return KILN_C99_OK;
}
int kiln_realm_symbol_key_for(uint64_t id, const uint8_t **out_key, uint64_t *out_key_len) {
    if (!kiln_test_realm_active || id != kiln_test_symbol_id || out_key == NULL ||
        out_key_len == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_key = (const uint8_t *)kiln_test_symbol_key;
    *out_key_len = kiln_test_symbol_key_len;
    return KILN_C99_OK;
}
int kiln_realm_callable_store(uint32_t tag, uint64_t code, void *candidate,
                              kiln_drop_opaque_fn drop_candidate, void **out_store) {
    size_t index;
    if (!kiln_test_realm_active || out_store == NULL ||
        ((candidate == NULL) != (drop_candidate == NULL))) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    if (candidate == NULL) {
        *out_store = NULL;
        return KILN_C99_OK;
    }
    for (index = 0; index < kiln_test_callable_store_count; ++index) {
        if (kiln_test_callable_tags[index] == tag && kiln_test_callable_codes[index] == code) {
            *out_store = kiln_test_callable_stores[index];
            return KILN_C99_OK;
        }
    }
    if (kiln_test_callable_store_count == 256) return KILN_C99_OUT_OF_MEMORY;
    index = kiln_test_callable_store_count++;
    kiln_test_callable_tags[index] = tag;
    kiln_test_callable_codes[index] = code;
    kiln_test_callable_stores[index] = candidate;
    *out_store = candidate;
    return KILN_C99_OK;
}
int kiln_realm_value_slot(uint32_t key, void **out_slot) {
    if (!kiln_test_realm_active || key == 0 || key > 5 || out_slot == NULL)
        return KILN_C99_INVALID_ARGUMENT;
    *out_slot = &kiln_test_value_slots[key - 1];
    return KILN_C99_OK;
}
int kiln_realm_entropy_word(uint64_t seed, uint64_t *out_word) {
    uint64_t mixed;
    if (!kiln_test_realm_active || out_word == NULL) return KILN_C99_INVALID_ARGUMENT;
    if (!kiln_test_entropy_initialized) {
        kiln_test_entropy_seed = seed;
        kiln_test_entropy_state = seed;
        kiln_test_entropy_initialized = 1;
    } else if (kiln_test_entropy_seed != seed) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    kiln_test_entropy_state += UINT64_C(0x9E3779B97F4A7C15);
    mixed = kiln_test_entropy_state;
    mixed = (mixed ^ (mixed >> 30)) * UINT64_C(0xBF58476D1CE4E5B9);
    mixed = (mixed ^ (mixed >> 27)) * UINT64_C(0x94D049BB133111EB);
    *out_word = mixed ^ (mixed >> 31);
    return KILN_C99_OK;
}
/* The fixture has no Tokio runtime and no HTTP engine, so it reports nothing. The dump this
 * feeds is unreachable here anyway: the fixture refuses to attach a heap, and the introspect
 * hook declines on a fallback heap. */
int kiln_realm_runtime_report(uint64_t *out_fields, uint64_t capacity, uint64_t *out_count) {
    (void)out_fields;
    (void)capacity;
    if (out_count == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_count = 0;
    return KILN_C99_INVALID_ARGUMENT;
}
void *kiln_realm_gc_current_heap(void) { return NULL; }
const uint64_t *kiln_realm_generation_slot(void) { static uint64_t generation = 1; return &generation; }
int kiln_realm_gc_attach(const kiln_gc_hooks *hooks, void *candidate, void **out_heap) {
    /* The fixture has no realm heap: every unit collects nothing and keeps its fallback heap. */
    (void)hooks;
    (void)candidate;
    if (out_heap != NULL) *out_heap = NULL;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_gc_register_roots(kiln_gc_roots_fn roots, void *context, uint32_t flags) {
    (void)roots;
    (void)context;
    (void)flags;
    return KILN_C99_OK;
}
int kiln_realm_gc_mark_roots(void *heap) {
    (void)heap;
    return KILN_C99_OK;
}
int kiln_realm_now(double *out_milliseconds) {
    if (!kiln_test_realm_active || out_milliseconds == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_milliseconds = 0.0;
    return KILN_C99_OK;
}
int kiln_realm_now_nanoseconds(int64_t *out_high, uint64_t *out_low) {
    if (!kiln_test_realm_active || out_high == NULL || out_low == NULL) {
        return KILN_C99_INVALID_ARGUMENT;
    }
    *out_high = 0;
    *out_low = 0;
    return KILN_C99_OK;
}
int kiln_realm_queue_microtask(kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work) {
    (void)run;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
/* The fixture drives nothing, so a registered lane is never drained: the prelude's ring holds
 * what a unit test queues until the process exits, exactly as this fixture's microtask queue
 * dropped it before. */
int kiln_realm_set_promise_lane(kiln_realm_lane_run_fn run,
                                kiln_realm_lane_pending_fn pending,
                                void *context) {
    (void)run;
    (void)pending;
    (void)context;
    return KILN_C99_OK;
}
int kiln_realm_queue_next_tick(kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work) {
    (void)run;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
/* No launcher ran, so there is no command line. The prelude reads zero here and leaves
 * `process.argv` empty, which is what a harness-hosted realm should report. */
int kiln_realm_argv_count(uint64_t *out_count) {
    if (!kiln_test_realm_active || out_count == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_count = 0;
    return KILN_C99_OK;
}
int kiln_realm_argv_at(uint64_t index, const uint8_t **out_bytes, uint64_t *out_len) {
    (void)index;
    (void)out_bytes;
    (void)out_len;
    return KILN_C99_INVALID_ARGUMENT;
}
static int kiln_test_set_timer(double delay_ms,
                               kiln_realm_work_fn run,
                               kiln_realm_drop_fn drop,
                               void *work,
                               uint64_t *out_id) {
    (void)delay_ms;
    (void)run;
    if (out_id == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_id = kiln_test_next_timer_id++;
    if (drop != NULL) drop(work);
    return KILN_C99_OK;
}
int kiln_realm_set_timeout(double delay_ms,
                           kiln_realm_work_fn run,
                           kiln_realm_drop_fn drop,
                           void *work,
                           uint64_t *out_id) {
    return kiln_test_set_timer(delay_ms, run, drop, work, out_id);
}
int kiln_realm_set_interval(double delay_ms,
                            kiln_realm_work_fn run,
                            kiln_realm_drop_fn drop,
                            void *work,
                            uint64_t *out_id) {
    return kiln_test_set_timer(delay_ms, run, drop, work, out_id);
}
int kiln_realm_clear_timer(uint64_t id) {
    (void)id;
    return KILN_C99_OK;
}
int kiln_realm_start_active_host_op(const kiln_async_host_op *operation,
                                    kiln_realm_completion_fn run,
                                    kiln_realm_drop_fn drop,
                                    void *work,
                                    uint64_t *out_request) {
    (void)operation;
    (void)run;
    if (out_request != NULL) *out_request = 1;
    if (drop != NULL) drop(work);
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_cancel_active_request(uint64_t request) {
    (void)request;
    return KILN_REALM_ASYNC_NOT_FOUND;
}

/* Scalar backend fixtures never exercise Node HTTP; production artifacts link these symbols
 * from the Rust realm provider. */
int kiln_realm_http_server_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work) {
    (void)resource; (void)run; if (drop != NULL) drop(work); return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_server_attach_with_options_active(
    uint64_t resource, int allow_half_close, kiln_realm_http_event_fn run,
    kiln_realm_drop_fn drop, void *work) {
    (void)allow_half_close;
    return kiln_realm_http_server_attach_active(resource, run, drop, work);
}
int kiln_realm_http_server_close_active(uint64_t resource, int mode) {
    (void)resource; (void)mode;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_attach_active(uint64_t resource, kiln_realm_http_event_fn run,
                                         kiln_realm_drop_fn drop, void *work) {
    (void)resource; (void)run; if (drop != NULL) drop(work); return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_request_id_active(uint64_t *out_id) {
    if (out_id == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_id = 1;
    return KILN_C99_OK;
}
int kiln_realm_http_agent_acquire_active(const uint8_t *host, uint64_t host_len,
                                         uint16_t port, uint64_t request,
                                         uint64_t *out_resource) {
    (void)host; (void)host_len; (void)port; (void)request;
    if (out_resource == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_resource = 0;
    return KILN_REALM_HTTP_AGENT_CONNECT;
}
int kiln_realm_http_client_owner_active(uint64_t resource, void **out_owner) {
    (void)resource;
    if (out_owner == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_owner = NULL;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_agent_connected_active(const uint8_t *host, uint64_t host_len,
                                           uint16_t port, uint64_t resource,
                                           uint64_t request) {
    (void)host; (void)host_len; (void)port; (void)resource; (void)request;
    return 1;
}
int kiln_realm_http_agent_release_active(uint64_t resource, int reusable,
                                         uint64_t *out_request) {
    (void)resource; (void)reusable;
    if (out_request == NULL) return KILN_C99_INVALID_ARGUMENT;
    *out_request = 0;
    return KILN_REALM_HTTP_AGENT_IDLE;
}
int kiln_realm_http_client_start_active(uint64_t resource,
                                        uint64_t id,
                                        const kiln_realm_http_request *request,
                                        uint64_t high_water_mark) {
    (void)resource; (void)id; (void)request; (void)high_water_mark;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_write_active(uint64_t resource,
                                        uint64_t id,
                                        const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_client_end_active(uint64_t resource,
                                      uint64_t id,
                                      const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_start_active(uint64_t resource, uint64_t id,
                                          const kiln_realm_http_response *response,
                                          uint64_t high_water_mark) {
    (void)resource; (void)id; (void)response; (void)high_water_mark;
    return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_write_active(uint64_t resource, uint64_t id,
                                          const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
int kiln_realm_http_response_end_active(uint64_t resource, uint64_t id,
                                        const uint8_t *data, uint64_t data_len) {
    (void)resource; (void)id; (void)data; (void)data_len; return KILN_C99_INVALID_ARGUMENT;
}
"#;

/// A real build compiles the vendored number runtime and each emitted unit as separate
/// translation units, so every unit starts with `HEADER` and asks for POSIX visibility before
/// its first `#include`. These tests concatenate them instead, which puts the number
/// runtime's includes first and makes `HEADER`'s request arrive too late to be honoured. The
/// prologue is therefore repeated here, ahead of everything, so the one TU the tests build
/// sees what the many TUs a build produces each see.
fn linked_source(module_source: &str, wrapper: &str) -> String {
    format!(
        "#if !defined(_WIN32) && !defined(__APPLE__) && !defined(_POSIX_C_SOURCE)\n\
             #define _POSIX_C_SOURCE 200809L\n\
             #endif\n{}\n{}\n{}\n{}\n{}",
        kiln_native_number::runtime_source(),
        REGEXP_FIXTURE,
        module_source,
        REALM_FIXTURE,
        wrapper
    )
}

fn function(code: Vec<Instruction>) -> FunctionProto {
    FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        parameter_count: 0,
        declared_length: 0,
        mapped_arguments: false,
        local_count: 0,
        code,
        upvalues: Vec::new(),
        type_annotations: None,
    }
}

#[test]
fn program_hash_covers_function_table_and_proto_contents() {
    let base = program(Vec::new(), vec![Instruction::Return], 0);
    let with_function = |functions| Program {
        functions,
        ..base.clone()
    };
    let empty_hash = hash_program(&with_function(Vec::new()));
    let one_hash = hash_program(&with_function(vec![function(vec![Instruction::Return])]));
    assert_ne!(
        empty_hash, one_hash,
        "empty and nonempty tables must differ"
    );

    let mut changed_body = function(vec![Instruction::Return]);
    changed_body.code = vec![Instruction::Pop, Instruction::Return];
    assert_ne!(one_hash, hash_program(&with_function(vec![changed_body])));

    for change in [
        FunctionProto {
            prologue_end: 0,
            asynchronous: true,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            name: Some("named".into()),
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            name: Some(String::new()),
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            arrow: true,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            generator: true,
            async_generator: false,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            is_method: true,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            parameter_count: 1,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            local_count: 1,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentLocal(0)],
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentUpvalue(0)],
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentLocal(1)],
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentUpvalue(1)],
            ..function(vec![Instruction::Return])
        },
    ] {
        assert_ne!(one_hash, hash_program(&with_function(vec![change])));
    }

    // Table order is semantic because MakeFunction stores stable table indices.  Referencing
    // each entry from the other function also exercises cyclic references without recursion.
    let first = function(vec![Instruction::MakeFunction(1), Instruction::Return]);
    let second = function(vec![Instruction::MakeFunction(0), Instruction::Return]);
    let cyclic = hash_program(&with_function(vec![first.clone(), second.clone()]));
    let reordered = hash_program(&with_function(vec![second, first]));
    assert_ne!(cyclic, reordered);
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
fn callable_module_materializes_its_private_refusal_before_returning_to_the_graph() {
    let generated = emit_module_with_logical_path(
        &module(
            Program {
                functions: vec![function(vec![Instruction::Return])],
                ..program(
                    Vec::new(),
                    vec![Instruction::MakeFunction(0), Instruction::Return],
                    0,
                )
            },
            Vec::new(),
            Vec::new(),
        ),
        "node_modules/provider/index.js",
    )
    .expect("callable module emits");

    assert!(generated.source.contains(
        "if (status == KILN_C99_CONTROL_ERROR)\n\
             \x20       *context->result = kiln_c99_control_diagnostic();"
    ));
    assert!(generated.source.contains(
        "if (status == KILN_C99_CONTROL_ERROR)\n\
             \x20           *result = kiln_c99_control_diagnostic();"
    ));
}

#[test]
fn closure_instruction_refusals_materialize_before_crossing_module_boundaries() {
    let generated = emit_module_with_logical_path(
        &module(
            Program {
                functions: vec![function(vec![
                    Instruction::LoadBuiltin(Builtin::Map),
                    Instruction::Call { argc: 0 },
                    Instruction::Return,
                ])],
                ..program(
                    Vec::new(),
                    vec![Instruction::MakeFunction(0), Instruction::Return],
                    0,
                )
            },
            Vec::new(),
            Vec::new(),
        ),
        "node_modules/provider/index.js",
    )
    .expect("refusing closure module emits");

    assert!(
        generated.source.contains(
            "kiln_c99_note_refusal(\"node_modules/provider/index.js\", (size_t)1, \"Call\""
        )
    );
    assert!(
        generated
            .source
            .contains("(size_t)kiln_c99_diagnostic_active_depth); goto kiln_refusal;")
    );
}

#[test]
fn logical_path_symbol_survives_body_edits_and_is_checkout_independent() {
    let first = module(
        program(
            vec![Constant::Number(1.0f64.to_bits())],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ),
        Vec::new(),
        Vec::new(),
    );
    let second = module(
        program(
            vec![Constant::Number(2.0f64.to_bits())],
            vec![Instruction::Const(0), Instruction::Return],
            0,
        ),
        Vec::new(),
        Vec::new(),
    );
    let one = emit_module_with_logical_path(&first, "src/dep.js").expect("module emits");
    let two = emit_module_with_logical_path(&second, "src/dep.js").expect("module emits");
    let other = emit_module_with_logical_path(&first, "src/other.js").expect("module emits");

    assert_eq!(one.symbol, two.symbol);
    assert_ne!(one.source, two.source);
    assert_ne!(one.symbol, other.symbol);
    assert!(!one.symbol.contains("/"));
    assert!(
        one.symbol
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
}

#[test]
fn logical_path_symbol_rejects_non_normalized_or_absolute_input() {
    let module = module(
        program(Vec::new(), vec![Instruction::Return], 0),
        Vec::new(),
        Vec::new(),
    );
    for path in [
        "",
        "/src/dep.js",
        "../dep.js",
        "src/../dep.js",
        "src\\dep.js",
        "C:/dep.js",
    ] {
        assert!(
            matches!(
                emit_module_with_logical_path(&module, path),
                Err(EmitError::InvalidProgram { message }) if message.contains("logical module path")
            ),
            "path should be rejected: {path:?}"
        );
    }
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
fn emitted_module_initializes_imports_and_publishes_exports() {
    let generated = emit_module(&module(
        program(
            vec![Constant::Number(2.0f64.to_bits())],
            vec![
                Instruction::LoadLocal(0),
                Instruction::Const(0),
                Instruction::Binary(BinaryOp::Add),
                Instruction::StoreLocal(1),
                Instruction::LoadLocal(1),
                Instruction::Return,
            ],
            2,
        ),
        vec![kiln_ir::ImportBinding {
            source: "dep".into(),
            imported: "value".into(),
            local: "input".into(),
            local_slot: 0,
        }],
        vec![kiln_ir::ExportBinding {
            exported: "answer".into(),
            local_slot: 1,
        }],
    ))
    .expect("module emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value input = kiln_c99_number(40.0); kiln_value output; kiln_value result; if ({}(NULL, &result, &input, 1, &output, 1) != 0) return 2; if (output.tag != KILN_VALUE_NUMBER || output.bits != kiln_c99_number(42.0).bits) return 3; if ({}(NULL, &result, NULL, 0, &output, 1) != KILN_C99_INVALID_ARGUMENT) return 4; return 0; }}\n",
        generated.symbol, generated.symbol
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
        .arg(std::env::temp_dir().join(format!("kiln-backend-c-module-{}", std::process::id())))
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
    let output_path =
        std::env::temp_dir().join(format!("kiln-backend-c-module-{}", std::process::id()));
    let compile = child.wait_with_output().expect("cc exits");
    assert!(
        compile.status.success(),
        "generated module C did not compile:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let run = Command::new(&output_path)
        .output()
        .expect("generated executable runs");
    let _ = std::fs::remove_file(output_path);
    assert!(
        run.status.success(),
        "generated module returned the wrong result"
    );
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
    assert!(generated.contains("\\x00\"\""));
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
    assert!(
        generated
            .source
            .contains("kiln_string_to_number(value.ptr, value.len)")
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
        if (arguments[0].tag != KILN_VALUE_STRING || arguments[0].ptr == NULL ||
            arguments[0].len != sizeof(expected) - 1 ||
            memcmp(arguments[0].ptr, expected, sizeof(expected) - 1) != 0)
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
            vec![Constant::String(HOST_CREATE_REALM.to_owned())],
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
            vec![Constant::String(HOST_EVAL_SCRIPT.to_owned())],
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
                vec![Constant::String(hook.to_owned())],
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
            vec![Constant::String(SHADOW_REALM.to_owned())],
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
            vec![Constant::String(SHADOW_REALM.to_owned())],
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
            vec![Constant::String("Array".to_owned())],
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
        read.tag != KILN_VALUE_STRING || read.len != 5 || memcmp(read.ptr, "first", 5) != 0)
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
        tag.tag != KILN_VALUE_STRING || tag.len != 9 ||
        memcmp(tag.ptr, "Int8Array", 9) != 0) return 4;
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
fn executable_array_from_maps_an_array_like_object() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("empty program emits runtime prelude");
    let wrapper = r#"
int main(void) {
    kiln_value source;
    kiln_value method = kiln_c99_method(KILN_C99_METHOD_ARRAY_FROM);
    kiln_value mapper = kiln_c99_method(KILN_C99_METHOD_NODE_UTIL_NOOP);
    kiln_value arguments[3];
    kiln_value result;
    kiln_value value;
    kiln_value thrown = kiln_c99_undefined();
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
    return 0;
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
        (Constant::String(String::new()), number(0.0), true),
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
               arguments[0].tag == KILN_VALUE_STRING && arguments[0].len == 1 &&
               arguments[0].ptr != NULL && arguments[0].ptr[0] == 'x') {{
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

/// Every binary operator emits now. The rule the two refused ones stood for still holds and
/// is worth a test of its own: an instruction this backend cannot do must say so rather than
/// emit a silent undefined, which is what `unreachable_unsupported_instructions` covers.
/// The explanation for the `const` gate above, which can only say *that* a site broke the
/// rule. This says which file, which line and which enclosing function -- and checks the half
/// the gate cannot see, that the internal-status sites really are where
/// `REFUSAL_INTERNAL_STATUS` says they are and in the numbers it claims.
#[test]
fn refusal_sites_name_themselves() {
    const SOURCES: [(&str, &str); 44] = [
        ("runtime/surface.c", include_str!("runtime/surface.c")),
        ("runtime/values.c", include_str!("runtime/values.c")),
        ("runtime/text.c", include_str!("runtime/text.c")),
        ("runtime/stores.c", include_str!("runtime/stores.c")),
        ("runtime/proxies.c", include_str!("runtime/proxies.c")),
        ("runtime/keys.c", include_str!("runtime/keys.c")),
        ("runtime/read.c", include_str!("runtime/read.c")),
        ("runtime/write.c", include_str!("runtime/write.c")),
        ("runtime/inspect.c", include_str!("runtime/inspect.c")),
        ("runtime/strings.c", include_str!("runtime/strings.c")),
        ("runtime/node.c", include_str!("runtime/node.c")),
        ("runtime/temporal.c", include_str!("runtime/temporal.c")),
        ("runtime/dispatch.c", include_str!("runtime/dispatch.c")),
        (
            "runtime/enumeration.c",
            include_str!("runtime/enumeration.c"),
        ),
        ("runtime/spread.c", include_str!("runtime/spread.c")),
        (
            "runtime/definitions.c",
            include_str!("runtime/definitions.c"),
        ),
        ("runtime/json.c", include_str!("runtime/json.c")),
        ("runtime/dates.c", include_str!("runtime/dates.c")),
        (
            "runtime/collections.c",
            include_str!("runtime/collections.c"),
        ),
        ("runtime/iterators.c", include_str!("runtime/iterators.c")),
        ("runtime/errors.c", include_str!("runtime/errors.c")),
        ("runtime/promises.c", include_str!("runtime/promises.c")),
        (
            "runtime/constructors.c",
            include_str!("runtime/constructors.c"),
        ),
        ("runtime/dynamic.c", include_str!("runtime/dynamic.c")),
        ("runtime/calls.c", include_str!("runtime/calls.c")),
        ("runtime/operators.c", include_str!("runtime/operators.c")),
        ("runtime/boxed.c", include_str!("runtime/boxed.c")),
        ("runtime/foundation.c", include_str!("runtime/foundation.c")),
        ("runtime/heap.c", include_str!("runtime/heap.c")),
        ("runtime/shapes.c", include_str!("runtime/shapes.c")),
        ("runtime/platform.c", include_str!("runtime/platform.c")),
        (
            "runtime/node_support.c",
            include_str!("runtime/node_support.c"),
        ),
        ("runtime/arrays.c", include_str!("runtime/arrays.c")),
        ("runtime/properties.c", include_str!("runtime/properties.c")),
        ("runtime/generators.c", include_str!("runtime/generators.c")),
        ("runtime/methods.c", include_str!("runtime/methods.c")),
        ("runtime/web.c", include_str!("runtime/web.c")),
        ("runtime/numbers.c", include_str!("runtime/numbers.c")),
        ("runtime/builtins.c", include_str!("runtime/builtins.c")),
        ("runtime/globals.c", include_str!("runtime/globals.c")),
        ("runtime/bigint.c", include_str!("runtime/bigint.c")),
        (
            "runtime/temporal_values.c",
            include_str!("runtime/temporal_values.c"),
        ),
        ("runtime/regexp.c", include_str!("runtime/regexp.c")),
        ("runtime/symbols.c", include_str!("runtime/symbols.c")),
    ];

    let mut unnamed = Vec::new();
    let mut internal_by_function: Vec<(String, usize)> = Vec::new();
    for (path, source) in SOURCES {
        // A top-level definition starts in column zero; everything inside one is indented.
        let mut function = String::from("<file scope>");
        for (index, line) in source.lines().enumerate() {
            if line.starts_with(|first: char| first.is_ascii_alphabetic())
                && let Some((head, _)) = line.split_once('(')
                && let Some(name) = head
                    .split(|byte: char| !(byte.is_alphanumeric() || byte == '_'))
                    .next_back()
            {
                function = name.to_owned();
            }
            if line.contains("KILN_C99_CONTROL_ERROR")
                && !line.contains("== KILN_C99_CONTROL_ERROR")
                && !line.contains("!= KILN_C99_CONTROL_ERROR")
            {
                unnamed.push(format!("{path}:{} in {function}", index + 1));
            }
            if line.contains("KILN_C99_REFUSE_INTERNAL(") {
                match internal_by_function
                    .iter_mut()
                    .find(|(name, _)| *name == function)
                {
                    Some((_, count)) => *count += 1,
                    None => internal_by_function.push((function.clone(), 1)),
                }
            }
        }
    }

    assert!(
        unnamed.is_empty(),
        "these prelude sites produce KILN_C99_CONTROL_ERROR by hand, so a refusal there would \
             name nothing and the enclosing instruction would invent a reason for it. Use \
             KILN_C99_REFUSE with what the site lacked, KILN_C99_REFUSE_NAMED when the subject is \
             a name only the site holds, or KILN_C99_REFUSE_INTERNAL when a caller converts the \
             status:\n  {}",
        unnamed.join("\n  ")
    );

    for row in REFUSAL_INTERNAL_STATUS {
        let found = internal_by_function
            .iter()
            .find(|(name, _)| name == row.function)
            .map(|(_, count)| *count)
            .unwrap_or(0);
        assert_eq!(
            found, row.sites,
            "REFUSAL_INTERNAL_STATUS says {} holds {} internal-status sites converted by {}, \
                 and the prelude holds {}",
            row.function, row.sites, row.converted_by, found
        );
    }
    for (function, count) in &internal_by_function {
        assert!(
            REFUSAL_INTERNAL_STATUS
                .iter()
                .any(|row| row.function == function.as_str()),
            "{function} holds {count} KILN_C99_REFUSE_INTERNAL sites and no \
                 REFUSAL_INTERNAL_STATUS row says which caller converts them"
        );
    }
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

/// Compile one emitted unit with a `main` of the caller's choosing and report its exit code.
fn run_with_main(generated: &CModule, wrapper: &str, label: &str) -> std::process::Output {
    let executable = std::env::temp_dir().join(format!(
        "kiln-backend-c-{label}-{}-{}",
        std::process::id(),
        generated.program_hash
    ));
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
        .write_all(linked_source(&generated.source, wrapper).as_bytes())
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
    run
}

#[test]
fn borrowed_array_map_and_filter_keep_generic_typed_array_semantics() {
    let callback = FunctionProto {
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

/// A closure over a module-level binding, which is the shape every counter has: the module
/// body and the function must write through one box, and the module must still read the
/// current value out of the slot afterwards.
#[test]
fn executable_closure_shares_a_cell_with_the_module_body() {
    let bump = FunctionProto {
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

/// An arrow has no call-time receiver. It snapshots the receiver of the frame that creates
/// it, even when a later plain call supplies `undefined` as `this`.
#[test]
fn executable_arrow_captures_its_defining_receiver() {
    let arrow = FunctionProto {
        prologue_end: 0,
        arrow: true,
        code: vec![Instruction::LoadThis, Instruction::Return],
        ..function(Vec::new())
    };
    let maker = FunctionProto {
        prologue_end: 0,
        code: vec![Instruction::MakeFunction(1), Instruction::Return],
        ..function(Vec::new())
    };
    let generated = emit(&Program {
        source: "".into(),
        version: BYTECODE_VERSION,
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
#[cfg(any())]
mod pre_typed_array_tests {

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
    if ({symbol}(NULL, &result) != KILN_C99_OK) return 2;
    return result.tag == KILN_VALUE_UNDEFINED ? 0 : 1;
}}
"#,
            symbol = generated.symbol
        );
        let run = run_with_main(&generated, &wrapper, "array-string-iterator");
        assert!(
            run.status.success(),
            "an absent ordinary @@iterator key must not expose Symbol.iterator"
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
        arguments[0].tag != KILN_VALUE_STRING || arguments[0].len != 8 ||
        memcmp(arguments[0].ptr, "{{ a: 2 }}", 8) != 0) return 1;
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
    if ({symbol}(NULL, &result) != KILN_C99_OK) return 2;
    return result.tag == KILN_VALUE_UNDEFINED ? 0 : 1;
}}
"#,
        symbol = generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "array-string-iterator");
    assert!(
        run.status.success(),
        "an absent ordinary @@iterator key must not expose Symbol.iterator"
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
    if (kiln_c99_invoke_method(NULL, &sub, KILN_C99_METHOD_TYPED_SUBARRAY, source,
                               (kiln_value[]){{kiln_c99_number(1.0), kiln_c99_number(3.0)}}, 2, 0,
                               &thrown) != KILN_C99_OK) return 8;
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
    kiln_c99_typed_own(bytes, "0", 1, &found, &enumerable);
    if (!found || !enumerable) return 19;
    kiln_c99_typed_own(bytes, "length", 6, &found, &enumerable);
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
    if (kiln_c99_get(NULL, prototype, "set", 3, &value, 0, &thrown) != KILN_C99_OK ||
        value.tag != KILN_VALUE_METHOD || value.bits != KILN_C99_METHOD_TYPED_SET) return 25;
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
        step.tag != KILN_VALUE_STRING || step.len != 10 ||
        memcmp(step.ptr, "Uint8Array", 10) != 0) return 28;
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
        arguments[0].tag != KILN_VALUE_STRING || arguments[0].len != 8 ||
        memcmp(arguments[0].ptr, "{{ a: 2 }}", 8) != 0) return 1;
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
fn executable_url_constructs_path_and_decodes_query_values() {
    let generated = emit(&program(
        vec![
            Constant::String("/echo?q=one%20two".into()),
            Constant::String("http://127.0.0.1".into()),
            Constant::String("pathname".into()),
            Constant::String("/echo".into()),
            Constant::String("searchParams".into()),
            Constant::String("get".into()),
            Constant::String("q".into()),
            Constant::String("one two".into()),
            Constant::Boolean(false),
        ],
        vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Url),
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::New { argc: 2 },
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(2),
            Instruction::Const(3),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::JumpIfFalse(20),
            Instruction::LoadLocal(0),
            Instruction::GetProperty(4),
            Instruction::Const(6),
            Instruction::CallMethod { name: 5, argc: 1 },
            Instruction::Const(7),
            Instruction::Binary(BinaryOp::StrictEqual),
            Instruction::Return,
            Instruction::Const(8),
            Instruction::Return,
            Instruction::Const(8),
            Instruction::Return,
        ],
        1,
    ))
    .expect("URL probe emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; if ({}(NULL, &result) != KILN_C99_OK) return 2; return result.tag == KILN_VALUE_BOOLEAN && result.bits ? 0 : 3; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "url-routing");
    assert!(
        run.status.success(),
        "native URL path/query behavior failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
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
fn executable_node_support_facades_are_deterministic_and_callback_driven() {
    let generated = emit(&program(
        vec![Constant::Undefined],
        vec![Instruction::Const(0), Instruction::Return],
        0,
    ))
    .expect("support facade probe emits");
    let wrapper = r#"
static int callback(kiln_call_context *ctx, kiln_val *result, kiln_val *upvalues,
                    const kiln_val *receiver_ptr, const kiln_val *args, size_t argc) {
    (void)ctx; (void)upvalues; (void)receiver_ptr;
    if (argc == 2 && KILN_VAL_IS_NULL(args[0]) && KILN_VAL_IS_OBJECT(args[1]) &&
        kiln_c99_array_of(kiln_val_to_boundary(args[1]))->count == 2) {
        *result = KILN_VAL_TRUE;
        return KILN_C99_OK;
    }
    if (argc == 1 && KILN_VAL_IS_STRING(args[0])) {
        kiln_value s = kiln_val_to_boundary(args[0]);
        if (s.len == 7 && memcmp(s.ptr, "forward", 7) == 0) {
            *result = KILN_VAL_TRUE;
            return KILN_C99_OK;
        }
    }
    *result = KILN_VAL_FALSE;
    return KILN_C99_OK;
}
int main(void) {
    uint64_t realm;
    kiln_value thrown = kiln_c99_undefined();
    kiln_value result;
    kiln_value args[4];
    kiln_value callable;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&callable, callback, "callback", 8, NULL, 0, "<test>", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK)
        return 1;
    args[0] = kiln_c99_copy_text("%s:%d", 5);
    args[1] = kiln_c99_copy_text("port", 4);
    args[2] = kiln_c99_number(7.0);
    if (kiln_c99_node_util(NULL, &result, KILN_C99_METHOD_NODE_UTIL_FORMAT,
                           args, 3, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_STRING || result.len != 6 ||
        memcmp(result.ptr, "port:7", 6) != 0) return 2;
    args[0] = kiln_c99_undefined();
    args[1] = callable;
    args[2] = kiln_c99_undefined();
    args[3] = kiln_c99_copy_text("forward", 7);
    if (kiln_c99_node_diagnostics(NULL, &result,
            KILN_C99_METHOD_NODE_DIAGNOSTICS_RUN_STORES, args, 4, 0, &thrown) != KILN_C99_OK ||
        result.tag != KILN_VALUE_BOOLEAN || !result.bits) return 3;
    args[0] = kiln_c99_copy_text("localhost", 9);
    if (kiln_c99_new_object(&args[1]) != KILN_C99_OK ||
        kiln_c99_put(NULL, args[1], "all", 3, kiln_c99_boolean(1), 0, NULL) != KILN_C99_OK)
        return 4;
    args[2] = callable;
    if (kiln_c99_node_dns_lookup(NULL, &result, args, 3, 0, &thrown) != KILN_C99_OK)
        return 5;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 6;
}
"#;
    let run = run_with_main(&generated, wrapper, "node-support-facades");
    assert!(
        run.status.success(),
        "native Node support facades failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
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
    const SOURCES: [(&str, &str); 44] = [
        ("runtime/surface.c", include_str!("runtime/surface.c")),
        ("runtime/values.c", include_str!("runtime/values.c")),
        ("runtime/text.c", include_str!("runtime/text.c")),
        ("runtime/stores.c", include_str!("runtime/stores.c")),
        ("runtime/proxies.c", include_str!("runtime/proxies.c")),
        ("runtime/keys.c", include_str!("runtime/keys.c")),
        ("runtime/read.c", include_str!("runtime/read.c")),
        ("runtime/write.c", include_str!("runtime/write.c")),
        ("runtime/inspect.c", include_str!("runtime/inspect.c")),
        ("runtime/strings.c", include_str!("runtime/strings.c")),
        ("runtime/node.c", include_str!("runtime/node.c")),
        ("runtime/temporal.c", include_str!("runtime/temporal.c")),
        ("runtime/dispatch.c", include_str!("runtime/dispatch.c")),
        (
            "runtime/enumeration.c",
            include_str!("runtime/enumeration.c"),
        ),
        ("runtime/spread.c", include_str!("runtime/spread.c")),
        (
            "runtime/definitions.c",
            include_str!("runtime/definitions.c"),
        ),
        ("runtime/json.c", include_str!("runtime/json.c")),
        ("runtime/dates.c", include_str!("runtime/dates.c")),
        (
            "runtime/collections.c",
            include_str!("runtime/collections.c"),
        ),
        ("runtime/iterators.c", include_str!("runtime/iterators.c")),
        ("runtime/errors.c", include_str!("runtime/errors.c")),
        ("runtime/promises.c", include_str!("runtime/promises.c")),
        (
            "runtime/constructors.c",
            include_str!("runtime/constructors.c"),
        ),
        ("runtime/dynamic.c", include_str!("runtime/dynamic.c")),
        ("runtime/calls.c", include_str!("runtime/calls.c")),
        ("runtime/operators.c", include_str!("runtime/operators.c")),
        ("runtime/boxed.c", include_str!("runtime/boxed.c")),
        ("runtime/foundation.c", include_str!("runtime/foundation.c")),
        ("runtime/heap.c", include_str!("runtime/heap.c")),
        ("runtime/shapes.c", include_str!("runtime/shapes.c")),
        ("runtime/platform.c", include_str!("runtime/platform.c")),
        (
            "runtime/node_support.c",
            include_str!("runtime/node_support.c"),
        ),
        ("runtime/arrays.c", include_str!("runtime/arrays.c")),
        ("runtime/properties.c", include_str!("runtime/properties.c")),
        ("runtime/generators.c", include_str!("runtime/generators.c")),
        ("runtime/methods.c", include_str!("runtime/methods.c")),
        ("runtime/web.c", include_str!("runtime/web.c")),
        ("runtime/numbers.c", include_str!("runtime/numbers.c")),
        ("runtime/builtins.c", include_str!("runtime/builtins.c")),
        ("runtime/globals.c", include_str!("runtime/globals.c")),
        ("runtime/bigint.c", include_str!("runtime/bigint.c")),
        (
            "runtime/temporal_values.c",
            include_str!("runtime/temporal_values.c"),
        ),
        ("runtime/regexp.c", include_str!("runtime/regexp.c")),
        ("runtime/symbols.c", include_str!("runtime/symbols.c")),
    ];

    let mut by_function: Vec<(String, usize)> = Vec::new();
    let mut by_hand = Vec::new();
    for (path, source) in SOURCES {
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
fn proven_numbers_reach_a_loop_carried_phi_and_stop_at_a_parameter() {
    // function (n) { let sum = 0; let i = 0; while (i < n) { sum += i; i++; } return sum; }
    //
    // `sum` and `i` are numbers by construction: literal starts, arithmetic on numbers, and
    // phis whose every input is one. `n` is a parameter and is never proven; the compare that
    // reads it therefore stays untyped, while `sum += i` and `i++` are typed.
    let proto = FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 1,
        name: Some("proven".into()),
        parameter_count: 1,
        local_count: 3,
        code: vec![
            Instruction::Const(0),
            Instruction::StoreLocal(1),
            Instruction::Const(0),
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(2), // 4: header
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::LessThan),
            Instruction::JumpIfFalse(17),
            Instruction::LoadLocal(1), // 8
            Instruction::LoadLocal(2),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(2),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(2),
            Instruction::Jump(4),
            Instruction::LoadLocal(1), // 17
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
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![Constant::Number(0), Constant::Number(1)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };
    let graph = build_function_graph(&proto, &program).unwrap();
    verify_function_graph(&graph).unwrap();
    let numbers = crate::graph::proven_numbers(&graph);

    let header = graph
        .blocks
        .iter()
        .find(|b| b.predecessors.len() == 2)
        .unwrap();
    let phis: Vec<ValueId> = header
        .operations
        .iter()
        .filter(|(_, op)| matches!(op, GraphOp::Phi { .. }))
        .map(|(v, _)| *v)
        .collect();
    // Slot 0 is the parameter: its phi is not proven. Slots 1 and 2 are.
    assert!(
        !numbers.contains(&phis[0]),
        "a parameter's phi is never proven"
    );
    assert!(numbers.contains(&phis[1]), "sum is proven through its phi");
    assert!(numbers.contains(&phis[2]), "i is proven through its phi");

    // Every arithmetic result in the body is proven; the compare against `n` is not.
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            if let GraphOp::Binary { op, left, .. } = op {
                match op {
                    BinaryOp::Add => assert!(numbers.contains(vid), "sum += i and i++ are typed"),
                    BinaryOp::LessThan => {
                        assert!(!numbers.contains(vid));
                        assert!(numbers.contains(left), "i is proven even where n is not");
                    }
                    _ => {}
                }
            }
        }
    }
    // The parameter itself is not a number.
    assert!(!numbers.contains(&graph.initial_locals[0]));
}

#[test]
fn versioning_emits_two_clones_with_entry_guard_and_loop_exit() {
    let proto = FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 1,
        name: Some("sum_to_n".into()),
        parameter_count: 1,
        local_count: 3,
        code: vec![
            Instruction::Const(0),
            Instruction::StoreLocal(1),
            Instruction::Const(0),
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(2), // 4: header
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::LessThan),
            Instruction::JumpIfFalse(17),
            Instruction::LoadLocal(1), // 8
            Instruction::LoadLocal(2),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(2),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(2),
            Instruction::Jump(4),
            Instruction::LoadLocal(1), // 17
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
        constants: vec![Constant::Number(0), Constant::Number(1)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };
    let graph = build_function_graph(&proto, &prog).unwrap();
    let candidates = crate::graph::candidate_number_parameters(&graph, &proto);
    assert_eq!(
        candidates,
        vec![0],
        "parameter slot 0 is candidate for numeric speculation"
    );

    let generated = emit(&prog).expect("program with candidate parameter emits");
    let src = &generated.source;

    // Must name the invariant in the comments/guard
    let param_inv = lookup_speculation_invariant("parameter_number");
    let loop_inv = lookup_speculation_invariant("loop_speculation_exit");
    assert!(src.contains(&format!("Speculation entry guard [{}]", param_inv.name)));
    assert!(src.contains(&format!("Speculation exit check [{}]", loop_inv.name)));

    // Must emit two clones
    assert!(src.contains("/* Clone A: guarded fast path */"));
    assert!(src.contains("kiln_fast_block_"));
    assert!(src.contains("/* Clone B: generic fallback path */"));
    assert!(src.contains("kiln_generic_block_"));
    assert!(src.contains("KILN_VAL_IS_DOUBLE(args[0])"));
}

#[test]
fn speculation_invariants_table_is_sound_and_complete() {
    assert_eq!(assert_speculation_invariants(SPECULATION_INVARIANTS), 7);
    for name in [
        "parameter_number",
        "loop_speculation_exit",
        "upvalue_array",
        "property_number",
        "spread_source_shaped",
        "spread_field_number",
        "method_call_number",
    ] {
        let row = lookup_speculation_invariant(name);
        assert!(!row.name.is_empty() && !row.language_rule.is_empty());
    }
}

#[test]
fn speculation_disabled_emits_generic_clone_alone() {
    let proto = FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 1,
        name: Some("sum_to_n_no_spec".into()),
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
            Instruction::JumpIfFalse(17),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(2),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(2),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Add),
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
        constants: vec![Constant::Number(0), Constant::Number(1)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };
    let orig = std::env::var("KILN_SPECULATE").ok();
    unsafe {
        std::env::set_var("KILN_SPECULATE", "0");
    }
    let generated = emit(&prog).expect("emits with speculation disabled");
    unsafe {
        if let Some(val) = orig {
            std::env::set_var("KILN_SPECULATE", val);
        } else {
            std::env::remove_var("KILN_SPECULATE");
        }
    }
    let src = &generated.source;
    assert!(!src.contains("/* Clone A: guarded fast path */"));
    assert!(!src.contains("kiln_fast_block_"));
    assert!(!src.contains("Speculation entry guard"));
    assert!(src.contains("kiln_block_"));
}

#[test]
fn value_graph_merges_locals_at_a_loop_header_with_phis() {
    // let sum = 0; let i = 0; while (i < 10) { sum += i; i++; } return sum;
    //
    // The header (pc 4) has two predecessors, the entry and the back edge. Each local it reads
    // must be a phi with one input per predecessor; without that the header compared the
    // pre-loop `i` forever (conformance/native/for-loop-in-arrow.js).
    let proto = FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: Some("phi_loop".into()),
        parameter_count: 0,
        local_count: 2,
        code: vec![
            Instruction::Const(0),
            Instruction::StoreLocal(0),
            Instruction::Const(0),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1), // 4: header
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::LessThan),
            Instruction::JumpIfFalse(17),
            Instruction::LoadLocal(0), // 8: body
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(1),
            Instruction::Const(2),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::Jump(4),
            Instruction::LoadLocal(0), // 17: exit
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
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::Number(0),
            Constant::Number(10),
            Constant::Number(1),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &program).unwrap();
    verify_function_graph(&graph).expect("a loop with phis verifies");

    let header = graph
        .blocks
        .iter()
        .find(|b| b.predecessors.len() == 2)
        .expect("the loop header has two predecessors");
    let phis: Vec<&Vec<(BlockId, ValueId)>> = header
        .operations
        .iter()
        .filter_map(|(_, op)| match op {
            GraphOp::Phi { inputs } => Some(inputs),
            _ => None,
        })
        .collect();
    assert_eq!(phis.len(), 2, "one phi per local at the header");
    for inputs in &phis {
        assert_eq!(inputs.len(), 2, "one input per predecessor");
        for pred in &header.predecessors {
            assert_eq!(inputs.iter().filter(|(b, _)| b == pred).count(), 1);
        }
    }
    // The condition reads the phi, not the pre-loop constant.
    let phi_ids: Vec<ValueId> = header
        .operations
        .iter()
        .filter(|(_, op)| matches!(op, GraphOp::Phi { .. }))
        .map(|(v, _)| *v)
        .collect();
    let compares_a_phi = header
        .operations
        .iter()
        .any(|(_, op)| matches!(op, GraphOp::Binary { left, .. } if phi_ids.contains(left)));
    assert!(compares_a_phi, "the header's compare must read a phi");

    // The entry block defines every slot the function starts with, so a parameter read or a
    // phi input has a definition to point at.
    let entry = graph.block(graph.entry_block).unwrap();
    let defined: Vec<ValueId> = entry
        .operations
        .iter()
        .filter(|(_, op)| matches!(op, GraphOp::LoadLocal(_)))
        .map(|(v, _)| *v)
        .collect();
    for slot_value in &graph.initial_locals {
        assert!(defined.contains(slot_value));
    }
}

#[test]
fn value_graph_builds_and_verifies_scalar_loop() {
    // for (let i = 0; i < 10; i++) sum += i;
    let proto = FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: Some("loop_test".into()),
        parameter_count: 0,
        local_count: 2,
        code: vec![
            Instruction::Const(0),      // 0
            Instruction::StoreLocal(0), // sum = 0
            Instruction::Const(0),      // 0
            Instruction::StoreLocal(1), // i = 0
            // loop header: pc = 4
            Instruction::LoadLocal(1),
            Instruction::Const(1), // 10
            Instruction::Binary(BinaryOp::LessThan),
            Instruction::JumpIfFalse(15),
            // loop body: pc = 8
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(0), // sum += i
            Instruction::LoadLocal(1),
            Instruction::Const(2), // 1
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1), // i++
            Instruction::Jump(4),
            // exit: pc = 17
            Instruction::LoadLocal(0),
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

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::Number(0),
            Constant::Number(10),
            Constant::Number(1),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &program).unwrap();
    assert!(!graph.blocks.is_empty());
    assert!(verify_function_graph(&graph).is_ok());
}

#[test]
fn value_graph_builds_and_verifies_try_catch() {
    let proto = FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: Some("try_catch".into()),
        parameter_count: 0,
        local_count: 2,
        code: vec![
            Instruction::PushHandler(5),
            Instruction::Const(0),
            Instruction::Throw,
            Instruction::PopHandler,
            Instruction::Jump(7),
            // catch handler at pc = 5
            Instruction::StoreLocal(1), // catch err
            Instruction::Const(1),
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

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![Constant::String("err".into()), Constant::Number(42)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &program).unwrap();
    assert!(verify_function_graph(&graph).is_ok());
}

#[test]
fn value_graph_lowers_to_c_without_stack_or_sp() {
    let proto = FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: Some("scalar_loop".into()),
        parameter_count: 0,
        local_count: 2,
        code: vec![
            Instruction::Const(0),      // 0
            Instruction::StoreLocal(0), // sum = 0
            Instruction::Const(0),      // 0
            Instruction::StoreLocal(1), // i = 0
            Instruction::LoadLocal(1),  // pc = 4
            Instruction::Const(1),      // 10
            Instruction::Binary(BinaryOp::LessThan),
            Instruction::JumpIfFalse(15),
            Instruction::LoadLocal(0), // pc = 8
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(1),
            Instruction::Const(2), // 1
            Instruction::Binary(BinaryOp::Add),
            Instruction::StoreLocal(1),
            Instruction::Jump(4),
            Instruction::LoadLocal(0), // pc = 17
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

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::Number(0),
            Constant::Number(10),
            Constant::Number(1),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &program).unwrap();
    assert!(verify_function_graph(&graph).is_ok());

    let mut output = String::new();
    let plan = crate::direct::CallPlan {
        numbers: crate::graph::proven_numbers(&graph),
        ..Default::default()
    };
    emit_function_from_graph(
        &mut output,
        &graph,
        &proto,
        &program,
        "test_mod",
        0,
        true,
        "scalar_loop",
        &[4],
        &[true],
        &plan,
        crate::emit_graph::EntryKind::Boxed,
    );

    // Invariants per Step 1.3:
    // 1. No stack[] array is allocated
    assert!(!output.contains("stack["));
    // 2. No sp stack pointer exists
    assert!(!output.contains("sp =") && !output.contains("sp;"));
    // 3. Values are C locals
    assert!(output.contains("kiln_val v0 = KILN_VAL_UNDEFINED"));
    // 4. Basic block labels exist
    assert!(output.contains("kiln_block_0:;"));
    assert!(output.contains("kiln_block_1:;"));
    // 5. Clean C99 return
    assert!(output.contains("return KILN_C99_OK;"));
}

#[test]
fn typed_dist_compiles_to_guarded_unboxed_clone_with_entry_guards_matching_untyped() {
    use kiln_ir::{TypeAnnotation, TypeAnnotations};

    // function dist(x, y) { return x * x + y * y; }
    let make_proto = |type_annotations: Option<TypeAnnotations>| FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 2,
        name: Some("dist".into()),
        parameter_count: 2,
        local_count: 2,
        code: vec![
            Instruction::LoadLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(1),
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ],
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations,
    };

    let untyped_proto = make_proto(None);
    let typed_proto = make_proto(Some(TypeAnnotations {
        parameters: vec![Some(TypeAnnotation::Number), Some(TypeAnnotation::Number)],
        return_type: Some(TypeAnnotation::Number),
    }));

    let untyped_prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![untyped_proto.clone()],
        template_sites: Vec::new(),
    };
    let typed_prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![typed_proto.clone()],
        template_sites: Vec::new(),
    };

    let untyped_graph = build_function_graph(&untyped_proto, &untyped_prog).unwrap();
    let typed_graph = build_function_graph(&typed_proto, &typed_prog).unwrap();

    let untyped_candidates =
        crate::graph::candidate_number_parameters(&untyped_graph, &untyped_proto);
    let typed_candidates = crate::graph::candidate_number_parameters(&typed_graph, &typed_proto);

    assert_eq!(untyped_candidates, vec![0, 1]);
    assert_eq!(typed_candidates, vec![0, 1]);

    let untyped_c = emit(&untyped_prog).unwrap();
    let typed_c = emit(&typed_prog).unwrap();

    // Both must emit Clone A (fast unboxed) and Clone B (generic fallback)
    assert!(typed_c.source.contains("/* Clone A: guarded fast path */"));
    assert!(
        typed_c
            .source
            .contains("/* Clone B: generic fallback path */")
    );

    // Both must emit entry guards for parameters 0 and 1
    assert!(typed_c.source.contains("KILN_VAL_IS_DOUBLE(args[0])"));
    assert!(typed_c.source.contains("KILN_VAL_IS_DOUBLE(args[1])"));

    // Verify the function bodies emitted for dist match between typed and untyped
    let extract_fn_body = |src: &str, sym: &str| -> String {
        let start = src
            .find("/* function 0: dist */")
            .expect("dist function definition");
        let end = src[start..]
            .find("/* entrypoint */")
            .map(|offset| start + offset)
            .unwrap_or(src.len());
        src[start..end].replace(sym, "kiln_module_SYM")
    };
    assert_eq!(
        extract_fn_body(&typed_c.source, &typed_c.symbol),
        extract_fn_body(&untyped_c.source, &untyped_c.symbol)
    );
}

#[test]
fn type_annotations_drive_candidate_speculation_without_prior_inferred_uses() {
    use kiln_ir::{TypeAnnotation, TypeAnnotations};

    // function passthrough(x) { return x; }
    let make_proto = |type_annotations: Option<TypeAnnotations>| FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 1,
        name: Some("passthrough".into()),
        parameter_count: 1,
        local_count: 1,
        code: vec![Instruction::LoadLocal(0), Instruction::Return],
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations,
    };

    let untyped_proto = make_proto(None);
    let typed_proto = make_proto(Some(TypeAnnotations {
        parameters: vec![Some(TypeAnnotation::Number)],
        return_type: Some(TypeAnnotation::Number),
    }));

    let untyped_prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![untyped_proto.clone()],
        template_sites: Vec::new(),
    };
    let typed_prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![typed_proto.clone()],
        template_sites: Vec::new(),
    };

    let untyped_graph = build_function_graph(&untyped_proto, &untyped_prog).unwrap();
    let typed_graph = build_function_graph(&typed_proto, &typed_prog).unwrap();

    let untyped_candidates =
        crate::graph::candidate_number_parameters(&untyped_graph, &untyped_proto);
    let typed_candidates = crate::graph::candidate_number_parameters(&typed_graph, &typed_proto);

    assert_eq!(untyped_candidates, Vec::<usize>::new());
    assert_eq!(typed_candidates, vec![0]);

    let typed_c = emit(&typed_prog).unwrap();
    assert!(typed_c.source.contains("/* Clone A: guarded fast path */"));
    assert!(
        typed_c
            .source
            .contains("/* Clone B: generic fallback path */")
    );
    assert!(typed_c.source.contains("KILN_VAL_IS_DOUBLE(args[0])"));
}

#[test]
fn type_profile_serialization_and_deserialization() {
    let profile = TypeProfile {
        version: 1,
        functions: vec![FunctionTypeProfile {
            index: 0,
            name: Some("fib".into()),
            parameters: vec!["int".into()],
            return_type: Some("number".into()),
        }],
        sites: vec![SiteTypeProfile {
            id: 42,
            tag: "number".into(),
        }],
    };

    let json = profile.to_json().unwrap();
    let parsed = TypeProfile::parse_json(&json).unwrap();
    assert_eq!(profile, parsed);
    assert_eq!(parsed.function(0).unwrap().parameters, vec!["int"]);
    assert_eq!(parsed.site_tag(42), Some("number"));
}

#[test]
fn type_profile_drives_candidate_speculation_and_recording() {
    let make_proto = || FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 1,
        name: Some("passthrough".into()),
        parameter_count: 1,
        local_count: 1,
        code: vec![Instruction::LoadLocal(0), Instruction::Return],
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };

    let proto = make_proto();
    let prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &prog).unwrap();

    // Without profile: no speculation
    let no_prof_candidates =
        crate::graph::candidate_number_parameters_with_profile(&graph, &proto, None);
    assert_eq!(no_prof_candidates, Vec::<usize>::new());

    // With profile indicating parameter 0 is a number:
    let func_prof = FunctionTypeProfile {
        index: 0,
        name: Some("passthrough".into()),
        parameters: vec!["number".into()],
        return_type: Some("number".into()),
    };
    let prof_candidates =
        crate::graph::candidate_number_parameters_with_profile(&graph, &proto, Some(&func_prof));
    assert_eq!(prof_candidates, vec![0]);

    // Emit with policy.type_profile:
    let profile_policy = EmissionPolicy {
        type_profile: Some(std::sync::Arc::new(TypeProfile {
            version: 1,
            functions: vec![func_prof],
            sites: Vec::new(),
        })),
        ..Default::default()
    };

    let c_with_profile = emit_with_policy(&prog, profile_policy.clone()).unwrap();
    assert!(
        c_with_profile
            .source
            .contains("/* Clone A: guarded fast path */")
    );
    assert!(
        c_with_profile
            .source
            .contains("KILN_VAL_IS_DOUBLE(args[0])")
    );

    let compact = emit_with_policy(
        &prog,
        EmissionPolicy {
            compact: true,
            ..profile_policy
        },
    )
    .unwrap();
    assert!(!compact.source.contains("Clone A: guarded fast path"));
    assert!(compact.source.len() < c_with_profile.source.len());

    // Emit with policy.type_profile_record:
    let record_policy = EmissionPolicy {
        type_profile_record: true,
        ..Default::default()
    };

    let c_with_record = emit_with_policy(&prog, record_policy).unwrap();
    assert!(c_with_record.source.contains("kiln_profile_tag_of"));
    assert!(c_with_record.source.contains("_dump_type_profile"));
    assert!(c_with_record.source.contains("_fn_0_param_tags[1]"));
    assert!(c_with_record.source.contains("atexit("));
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
fn pure_closure_classification_and_parallel_map_emission() {
    use crate::graph::build_function_graph;

    // A pure callback: (x) => x * 2
    let callback = FunctionProto {
        prologue_end: 0,
        local_count: 1,
        parameter_count: 1,
        upvalues: Vec::new(),
        code: vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0), // 2.0
            Instruction::Binary(BinaryOp::Multiply),
            Instruction::Return,
        ],
        arrow: true,
        ..function(Vec::new())
    };

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::Number(2f64.to_bits()),
            Constant::String("parallelMap".into()),
        ],
        code: Vec::new(),
        local_count: 0,
        functions: vec![callback.clone()],
        template_sites: Vec::new(),
    };

    let graph = build_function_graph(&callback, &program).expect("callback graph builds");
    // `x => x * 2` is a function of its double parameter: the pooled lowering may run it on a
    // provider pool thread (ADR 0058, application 2).
    assert!(crate::parallel::parallel_safe(&graph, &callback, &program));
    // Code calling array.parallelMap(callback)
    let module_code = vec![
        Instruction::NewArray { count: 0 },
        Instruction::MakeFunction(0),
        Instruction::CallMethod {
            name: 1, // "parallelMap"
            argc: 1,
        },
        Instruction::Return,
    ];

    let full_program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::Number(2f64.to_bits()),
            Constant::String("parallelMap".into()),
        ],
        code: module_code,
        local_count: 0,
        functions: vec![callback],
        template_sites: Vec::new(),
    };

    let generated = emit(&full_program).expect("parallelMap emission succeeds");
    assert!(
        generated.source.contains("built_arr"),
        "emitted C should contain native array lowering for parallelMap"
    );
}
