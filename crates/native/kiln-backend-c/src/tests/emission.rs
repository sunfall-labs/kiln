use super::*;

#[test]
fn generated_closures_retain_their_host_context_without_rebinding_plain_closures() {
    let mut closure_program = program(
        Vec::new(),
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
    );
    closure_program
        .functions
        .push(function(vec![Instruction::Return]));
    let generated = emit(&closure_program).expect("closure module emits");
    assert!(
        generated
            .source
            .contains("kiln_c99_closure_of(built)->module_host = kiln_c99_module_host(host);")
    );

    let wrapper = r#"
int main(void) {
    kiln_host old_host = { NULL, (void *)(uintptr_t)1, NULL };
    kiln_host current_host = { NULL, (void *)(uintptr_t)2, NULL };
    kiln_host unused_storage;
    kiln_closure module_closure = {0};
    kiln_closure plain_closure = {0};
    module_closure.module_host = &old_host;
    if (kiln_c99_closure_host(&current_host, &module_closure, &unused_storage) != &old_host) return 1;
    if (old_host.call != current_host.call) return 2;
    plain_closure.module_host = NULL;
    if (kiln_c99_closure_host(&current_host, &plain_closure, &unused_storage) != &current_host) return 3;
    if (kiln_c99_module_host(&current_host) != NULL) return 4;
    old_host.module_context = &old_host;
    if (kiln_c99_module_host(&old_host) != &old_host) return 5;
    return 0;
}
"#;
    let run = run_with_main(&generated, wrapper, "closure-module-host-context");
    assert!(
        run.status.success(),
        "closure host binding failed ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn generated_entry_refuses_a_failed_root_registration_before_running() {
    let generated = emit(&program(
        vec![Constant::Number(42.0f64.to_bits())],
        vec![Instruction::Const(0), Instruction::Return],
        0,
    ))
    .expect("scalar entry emits");
    let wrapper = r#"
static int root_registration_entry(void *context) {
    kiln_value result = kiln_c99_number(91.0);
    kiln_value before = result;
    int status;
    (void)context;
    kiln_test_fail_root_registration = 1;
    status = __MODULE_ENTRY__(NULL, &result);
    if (status != KILN_C99_HOST_ERROR || memcmp(&result, &before, sizeof result) != 0) return 1;
    kiln_test_fail_root_registration = 0;
    status = __MODULE_ENTRY__(NULL, &result);
    if (status != KILN_C99_OK || result.tag != KILN_VALUE_NUMBER ||
        kiln_c99_as_number(result) != 42.0) return 2;
    return 0;
}
int main(void) {
    uint64_t realm;
    int status;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 3;
    status = kiln_realm_enter(realm, root_registration_entry, NULL);
    if (status != KILN_C99_OK) return status;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 4;
}
"#
    .replace("__MODULE_ENTRY__", &generated.symbol);
    let run = run_with_main(&generated, &wrapper, "failed-module-root-registration");
    assert!(
        run.status.success(),
        "module ran after failed root registration ({:?}):\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn default_emission_policy_is_inert_and_host_clock_changes_identity() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let legacy = emit(&program).expect("default program emits");
    let explicit_default =
        emit_with_policy(&program, EmissionPolicy::default()).expect("default policy emits");
    let explicit_enabled = emit_with_policy(
        &program,
        EmissionPolicy {
            speculate: Some(true),
            ..EmissionPolicy::default()
        },
    )
    .expect("explicitly enabled policy emits");
    let explicit_disabled = emit_with_policy(
        &program,
        EmissionPolicy {
            speculate: Some(false),
            ..EmissionPolicy::default()
        },
    )
    .expect("explicitly disabled policy emits");
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
    assert_eq!(legacy.symbol, explicit_enabled.symbol);
    assert_eq!(legacy.program_hash, explicit_enabled.program_hash);
    assert_ne!(legacy.symbol, explicit_disabled.symbol);
    assert_ne!(legacy.program_hash, explicit_disabled.program_hash);
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
fn decoupled_modules_include_one_external_runtime_header() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
    assert!(
        runtime_prelude_header().contains("extern const kiln_string kiln_c99_ascii_strings[256]"),
        "modules must refer to the program-wide ASCII string table"
    );
    assert!(
        !runtime_prelude_header().contains("static const kiln_string kiln_c99_ascii_strings[256]"),
        "the declarations header must not instantiate the table in every module"
    );
    assert_eq!(
        runtime_prelude_source()
            .matches("const kiln_string kiln_c99_ascii_strings[256]")
            .count(),
        1,
        "the prelude must own exactly one ASCII string table"
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
fn structural_instruction_refusals_always_name_their_owner() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("structural refusal probe emits");
    assert!(
        generated.source.contains("kiln_c99_note_refusal"),
        "runtime must include the refusal provenance boundary"
    );
    let emitter = include_str!("../tail.rs");
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::String("f16round".into())],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("read_unimplemented_math_property".into()),
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            is_class: false,
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
        debug_call_sites: Vec::new(),
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
/* The closure keeps the name pointer, so it is a headed static over ASCII bytes. */
static const uint8_t init_name_data[] = "initialize";
static const kiln_string init_name = { 10, KILN_STRING_LATIN1, init_name_data };
int main(void) {
    uint64_t realm;
    kiln_value function;
    kiln_value result = kiln_c99_undefined();
    kiln_value thrown = kiln_c99_undefined();
    kiln_value diagnostic;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&function, refuses_before_instruction, &init_name,
                              NULL, 0, "node_modules/example/index.js", 0, 0, 0, 0, 0, 0, 0,
                              1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED, 0, 1, 0, 0,
                              NULL, 0) != KILN_C99_OK) return 1;
    if (kiln_c99_call(NULL, &result, function, kiln_c99_undefined(), NULL, 0, 0,
                      &thrown) != KILN_C99_CONTROL_ERROR) return 2;
    diagnostic = kiln_c99_control_diagnostic();
    if (diagnostic.tag != KILN_VALUE_STRING) return 3;
    if (!kiln_test_units_contain(diagnostic, "module=node_modules/example/index.js")) return 4;
    if (!kiln_test_units_contain(diagnostic, "operation=FunctionBody")) return 5;
    if (!kiln_test_units_contain(diagnostic, "function=initialize")) return 6;
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
fn an_unowned_runtime_failure_does_not_publish_a_null_diagnostic_site() {
    let generated = emit(&program(vec![], vec![Instruction::Return], 0))
        .expect("diagnostic ownership probe emits");
    let wrapper = r#"
int main(void) {
    uint64_t realm;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 1;
    kiln_test_realm_active = 1;
    kiln_c99_diagnostic_detail = "nested helper threw";
    kiln_c99_note_refusal(NULL, 0, NULL, 1);
    if (kiln_c99_diagnostic_site != NULL || kiln_c99_diagnostic_operation != NULL) return 2;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 3;
}
"#;
    let run = run_with_main(&generated, wrapper, "unowned-runtime-diagnostic");
    assert!(
        run.status.success(),
        "unowned runtime diagnostic failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn compact_stack_tail_operations_use_shared_operation_helpers() {
    use crate::tail::{TailContext, emit_tail_instruction};

    let instructions = [
        Instruction::DefineAccessor {
            setter: false,
            enumerable: true,
        },
        Instruction::DefineMethod,
        Instruction::Apply,
    ];
    let render = |compact| {
        let context = TailContext {
            constants: &[],
            symbol: "ku",
            diagnostic_site: if compact {
                super::COMPACT_DIAGNOSTIC_SITE
            } else {
                "fixture.js"
            },
            callables: true,
            compact,
            compile_regex: !compact,
        };
        let mut output = String::new();
        for (pc, instruction) in instructions.iter().enumerate() {
            emit_tail_instruction(
                &mut output,
                instruction,
                pc as u32,
                &context,
                "goto kiln_unwind;",
            );
        }
        output
    };

    let compact = render(true);
    assert!(compact.contains("kiln_c99_tail_define_accessor"));
    assert!(compact.contains("kiln_c99_tail_define_method"));
    assert!(compact.contains("kiln_c99_tail_apply"));
    assert!(!compact.contains("kiln_value method ="));
    assert!(!compact.contains("kiln_value list ="));

    let ordinary = render(false);
    assert!(!ordinary.contains("kiln_c99_tail_"));
    assert!(ordinary.contains("kiln_value method ="));
    assert!(ordinary.contains("kiln_value list ="));
    assert!(runtime_prelude_source().contains("kiln_c99_tail_define_method"));
    assert!(runtime_prelude_header().contains("kiln_c99_tail_define_method"));
}

#[test]
fn compact_literals_use_shared_nan_boxed_builders() {
    let policy = EmissionPolicy {
        compact: true,
        decoupled_prelude: true,
        ..EmissionPolicy::default()
    };
    let object = emit_with_policy(
        &program(
            vec![
                Constant::String("answer".to_owned().into()),
                Constant::Number(42_f64.to_bits()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::NewObject { count: 1 },
                Instruction::Return,
            ],
            0,
        ),
        policy.clone(),
    )
    .expect("compact object literal emits");
    let array = emit_with_policy(
        &program(
            vec![Constant::Number(42_f64.to_bits())],
            vec![
                Instruction::Const(0),
                Instruction::NewArray { count: 1 },
                Instruction::Return,
            ],
            0,
        ),
        policy,
    )
    .expect("compact array literal emits");

    assert!(object.source.contains("kiln_c99_val_new_object_literal"));
    assert!(array.source.contains("kiln_c99_val_new_array_literal"));
    assert!(!array.source.contains("kiln_array *arr ="));
    assert!(runtime_prelude_source().contains("kiln_c99_val_new_object_literal"));
    assert!(runtime_prelude_header().contains("kiln_c99_val_new_object_literal"));
}

#[test]
fn compact_closure_source_unions_live_ranges_and_remaps_nested_spans() {
    let mut program = program(vec![], vec![Instruction::Return], 0);
    program.source = "xxabcYYdefZZ".into();
    program.functions = vec![
        function(vec![Instruction::Return]),
        function(vec![Instruction::Return]),
        function(vec![Instruction::Return]),
    ];
    program.functions[0].span = kiln_ir::Span { start: 2, end: 5 };
    program.functions[1].span = kiln_ir::Span { start: 7, end: 10 };
    program.functions[2].span = kiln_ir::Span { start: 3, end: 4 };

    compact_closure_source(&mut program, None, false);

    assert_eq!(&*program.source, "abcdef");
    assert_eq!(
        program.functions[0].span,
        kiln_ir::Span { start: 0, end: 3 }
    );
    assert_eq!(
        program.functions[1].span,
        kiln_ir::Span { start: 3, end: 6 }
    );
    assert_eq!(
        program.functions[2].span,
        kiln_ir::Span { start: 1, end: 2 }
    );

    compact_closure_source(&mut program, None, true);
    assert!(program.source.is_empty());
    assert!(program.functions[0].span.start > program.functions[0].span.end);
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
            inferred_name: None,
            prologue_end: 0,
            asynchronous: true,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            name: Some("named".into()),
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            name: Some(String::new()),
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            arrow: true,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            generator: true,
            async_generator: false,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            is_method: true,
            is_class: false,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            parameter_count: 1,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            local_count: 1,
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentLocal(0)],
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentUpvalue(0)],
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentLocal(1)],
            ..function(vec![Instruction::Return])
        },
        FunctionProto {
            inferred_name: None,
            prologue_end: 0,
            upvalues: vec![Upvalue::ParentUpvalue(1)],
            ..function(vec![Instruction::Return])
        },
    ] {
        assert_ne!(one_hash, hash_program(&with_function(vec![change])));
    }

    let mut inferred_high = function(vec![Instruction::Return]);
    inferred_high.inferred_name = Some(kiln_ir::StringUnits::from_units(&[0xd800]));
    let mut inferred_replacement = inferred_high.clone();
    inferred_replacement.inferred_name = Some(kiln_ir::StringUnits::from_units(&[0xfffd]));
    assert_ne!(
        hash_program(&with_function(vec![inferred_high.clone()])),
        hash_program(&with_function(vec![inferred_replacement.clone()])),
        "inferred function names retain distinct UTF-16 units in the cache identity"
    );

    inferred_high.name = Some("written".into());
    inferred_replacement.name = Some("written".into());
    assert_eq!(
        hash_program(&with_function(vec![inferred_high])),
        hash_program(&with_function(vec![inferred_replacement])),
        "a written name takes precedence over inferred display metadata"
    );

    // Table order is semantic because MakeFunction stores stable table indices.  Referencing
    // each entry from the other function also exercises cyclic references without recursion.
    let first = function(vec![Instruction::MakeFunction(1), Instruction::Return]);
    let second = function(vec![Instruction::MakeFunction(0), Instruction::Return]);
    let cyclic = hash_program(&with_function(vec![first.clone(), second.clone()]));
    let reordered = hash_program(&with_function(vec![second, first]));
    assert_ne!(cyclic, reordered);
}

#[test]
fn program_hash_covers_lossless_source_origin() {
    let base = program(Vec::new(), vec![Instruction::Return], 0);
    let with_origin = |unit| {
        let mut program = base.clone();
        program.source_origin = Some(std::sync::Arc::new(kiln_ir::SourceOrigin {
            units: kiln_ir::StringUnits::from_units(&[unit]),
            byte_to_unit: vec![0, 1].into(),
        }));
        program
    };
    assert_ne!(
        hash_program(&with_origin(0xd800)),
        hash_program(&with_origin(0xfffd)),
        "distinct original UTF-16 source units must not share an emitted-artifact identity"
    );
}

#[test]
fn program_hash_covers_static_transform_identity() {
    let base = program(Vec::new(), vec![Instruction::Return], 0);
    let with_identity = |identity: &str| {
        let mut program = base.clone();
        program.static_source_identity = Some(std::sync::Arc::from(identity));
        program
    };
    assert_ne!(
        hash_program(&with_identity("transform-a")),
        hash_program(&with_identity("transform-b")),
        "transform policy and source-map identity must affect native cache identity"
    );

    let source = "export const add = (left: number, right: number) => left + right;";
    let compile_at = |path: &str| {
        let hir = kiln_frontend_oxc::parse(path, source);
        kiln_ir::compile_module(&hir)
            .expect("static TypeScript compiles")
            .program
    };
    let first = compile_at("/checkout-one/src/entry.ts");
    let second = compile_at("/checkout-two/src/entry.ts");
    assert_eq!(
        first.static_source_identity, second.static_source_identity,
        "absolute diagnostic paths do not participate in content identity"
    );
    assert_eq!(
        hash_program(&first),
        hash_program(&second),
        "identical transformed programs and maps should share native artifacts across roots"
    );
}

#[test]
fn program_hash_covers_debug_call_site_coordinates() {
    let base = program(Vec::new(), vec![Instruction::Return], 0);
    let with_site = |site: kiln_ir::DebugCallSite| {
        let mut program = base.clone();
        program.debug_call_sites = vec![site];
        program
    };
    let base_site = kiln_ir::DebugCallSite {
        function_span: None,
        instruction: 0,
        line: 3,
        column: 7,
    };
    let variants = [
        kiln_ir::DebugCallSite {
            line: 4,
            ..base_site
        },
        kiln_ir::DebugCallSite {
            column: 8,
            ..base_site
        },
        kiln_ir::DebugCallSite {
            instruction: 1,
            ..base_site
        },
        kiln_ir::DebugCallSite {
            function_span: Some(kiln_ir::Span { start: 0, end: 0 }),
            ..base_site
        },
        kiln_ir::DebugCallSite {
            line: 3,
            ..base_site
        },
    ];
    let baseline = hash_program(&with_site(base_site));
    for variant in variants.into_iter().take(4) {
        assert_ne!(
            baseline,
            hash_program(&with_site(variant)),
            "each debug call-site field affects emitted frame metadata"
        );
    }
    let mut reordered = base.clone();
    reordered.debug_call_sites = vec![base_site, variants[0]];
    let mut reversed = reordered.clone();
    reversed.debug_call_sites.reverse();
    assert_ne!(
        hash_program(&reordered),
        hash_program(&reversed),
        "ordered call-site tables must retain their instruction mapping"
    );
}

#[test]
fn module_hash_covers_link_facts_used_by_emission() {
    let base = ModuleProgram {
        program: program(Vec::new(), vec![Instruction::Return], 0),
        ..ModuleProgram::default()
    };
    let baseline = hash_module_with_seed(&base, 0xcbf29ce484222325);

    let mut direct_eval = base.clone();
    direct_eval.link_facts.direct_eval = true;
    assert_ne!(
        baseline,
        hash_module_with_seed(&direct_eval, 0xcbf29ce484222325),
        "direct eval changes module evaluation emission"
    );

    let mut removable = base.clone();
    removable.link_facts.removable_declarations = vec![kiln_ir::RemovableDeclaration {
        local_slots: vec![1, 3],
        pc_start: 2,
        pc_end: 5,
    }];
    assert_ne!(
        baseline,
        hash_module_with_seed(&removable, 0xcbf29ce484222325),
        "closed-world declaration elision changes module emission"
    );
    let mut changed = removable.clone();
    changed.link_facts.removable_declarations[0]
        .local_slots
        .reverse();
    assert_ne!(
        hash_module_with_seed(&removable, 0xcbf29ce484222325),
        hash_module_with_seed(&changed, 0xcbf29ce484222325),
        "declaration slot ordering is retained in module identity"
    );
    changed = removable.clone();
    changed.link_facts.removable_declarations[0].pc_end += 1;
    assert_ne!(
        hash_module_with_seed(&removable, 0xcbf29ce484222325),
        hash_module_with_seed(&changed, 0xcbf29ce484222325),
        "declaration boundaries are retained in module identity"
    );
}

#[test]
fn emitted_closure_names_use_lossless_effective_function_metadata() {
    let mut high = function(vec![Instruction::Return]);
    high.inferred_name = Some(kiln_ir::StringUnits::from_units(&[0xd800]));
    let mut high_program = program(
        Vec::new(),
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
    );
    high_program.functions = vec![high];
    let high_emitted = emit(&high_program).expect("inferred name program emits");
    assert!(high_emitted.source.contains(&format!(
        "static const kiln_string {}_function_name_0 = {{ 1, KILN_STRING_UTF16",
        high_emitted.symbol
    )));
    assert!(
        high_emitted
            .source
            .contains(&format!("&{}_function_name_0", high_emitted.symbol))
    );

    let mut written = function(vec![Instruction::Return]);
    written.name = Some("written".into());
    written.inferred_name = Some(kiln_ir::StringUnits::from_units(&[0xd800]));
    let mut written_program = high_program;
    written_program.functions = vec![written];
    let written_emitted = emit(&written_program).expect("written name program emits");
    assert!(written_emitted.source.contains(&format!(
        "static const kiln_string {}_function_name_0 = {{ 7, KILN_STRING_LATIN1",
        written_emitted.symbol
    )));
}

#[test]
fn compact_mode_deduplicates_function_names_across_functions() {
    let mut f0 = function(vec![Instruction::Return]);
    f0.name = Some("shared".into());
    let mut f1 = function(vec![Instruction::Return]);
    f1.name = Some("shared".into());
    let mut f2 = function(vec![Instruction::Return]);
    f2.name = Some("distinct".into());
    let mut prog = program(
        Vec::new(),
        vec![
            Instruction::MakeFunction(0),
            Instruction::MakeFunction(1),
            Instruction::MakeFunction(2),
            Instruction::Return,
        ],
        0,
    );
    prog.functions = vec![f0, f1, f2];
    let emitted = emit_with_policy(
        &prog,
        EmissionPolicy {
            compact: true,
            generic_only: true,
            ..Default::default()
        },
    )
    .expect("program emits");
    assert!(emitted.source.contains(&format!(
        "static const kiln_string {}_fn_name_0 =",
        emitted.symbol
    )));
    assert!(emitted.source.contains(&format!(
        "static const kiln_string {}_fn_name_1 =",
        emitted.symbol
    )));
    assert!(!emitted.source.contains(&format!(
        "static const kiln_string {}_fn_name_2 =",
        emitted.symbol
    )));
    let fn0_target = emitted
        .source
        .lines()
        .find(|l| l.starts_with(&format!("#define {}_function_name_0", emitted.symbol)))
        .unwrap();
    let fn1_target = emitted
        .source
        .lines()
        .find(|l| l.starts_with(&format!("#define {}_function_name_1", emitted.symbol)))
        .unwrap();
    assert_eq!(
        fn0_target.split_whitespace().last(),
        fn1_target.split_whitespace().last()
    );
}

#[test]
fn generic_mixed_tier_keeps_unsupported_bodies_at_o0_and_boxed() {
    let selected = function(vec![Instruction::Return]);
    let mut unsupported = function(vec![Instruction::Return]);
    unsupported.generator = true;
    let mut prog = program(
        Vec::new(),
        vec![
            Instruction::MakeFunction(0),
            Instruction::MakeFunction(1),
            Instruction::Return,
        ],
        0,
    );
    prog.functions = vec![selected, unsupported];

    let emitted = emit_with_policy(
        &prog,
        EmissionPolicy {
            generic_only: true,
            ..Default::default()
        },
    )
    .expect("generic module emits");
    assert!(emitted.generic_only);
    assert!(emitted.generic_mixed_optimization);
    assert_eq!(emitted.chunk_generic_mixed_optimization, Vec::<bool>::new());
    assert_eq!(
        emitted
            .source
            .matches("defined(KILN_GENERIC_MIXED_TIER)")
            .count(),
        1,
        "only the unselected generic body receives an O0 attribute"
    );
    assert!(emitted.source.contains("_fn_0("));
    assert!(emitted.source.contains("_fn_1("));
    assert!(
        !emitted.source.contains("_fn_0_t("),
        "the optimized generic tier retains the boxed ABI instead of a typed clone"
    );

    let mut compiled = emitted.clone();
    compiled.source = format!("#define KILN_GENERIC_MIXED_TIER 1\n{}", compiled.source);
    let wrapper = format!(
        "int main(void) {{ kiln_value result; return {0}(NULL, &result) == KILN_C99_OK ? 0 : 1; }}\n",
        compiled.symbol
    );
    let run = run_with_main(&compiled, &wrapper, "generic-mixed-tier-attributes");
    assert!(
        run.status.success(),
        "mixed-tier C compiles and runs: {}",
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn generic_mixed_tier_keeps_looping_generator_boxed_while_selecting_its_body() {
    let mut generator = function(vec![
        Instruction::NewObject { count: 0 },
        Instruction::Yield,
        Instruction::Pop,
        Instruction::Jump(0),
    ]);
    generator.generator = true;
    let mut prog = program(
        Vec::new(),
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
    );
    prog.functions.push(generator);

    let emitted = emit_with_policy(
        &prog,
        EmissionPolicy {
            generic_only: true,
            ..Default::default()
        },
    )
    .expect("generic generator module emits");
    assert!(emitted.generic_mixed_optimization);
    assert!(
        !emitted.source.contains("_fn_0_t("),
        "the O1 tier keeps the existing boxed generator entry and ABI"
    );
    assert_eq!(
        emitted
            .source
            .matches("defined(KILN_GENERIC_MIXED_TIER)")
            .count(),
        0,
        "the selected generator body is emitted without the O0 optnone attribute"
    );

    let mut compiled = emitted.clone();
    compiled.source = format!("#define KILN_GENERIC_MIXED_TIER 1\n{}", compiled.source);
    let wrapper = format!(
        r#"
int main(void) {{
    uint64_t realm;
    kiln_value function, generator, result, value, done;
    kiln_value thrown = kiln_c99_undefined();
    if (setenv("KILN_GC_STRESS", "1", 1) != 0) return 1;
    if (kiln_realm_create(&realm) != KILN_C99_OK) return 2;
    kiln_test_realm_active = 1;
    if (kiln_c99_make_closure(&function, {symbol}_fn_0, NULL, NULL, 0, "<tier-test>", 0, 0,
                              1, 0, 0, 0, 0, 1, KILN_VAL_UNDEFINED, KILN_VAL_UNDEFINED,
                              0, 4, 0, 0, NULL, 0) != KILN_C99_OK) return 3;
    {{ int status = kiln_c99_call(NULL, &generator, function, kiln_c99_undefined(), NULL, 0, 0,
                                  &thrown);
       if (status != KILN_C99_OK) {{
           size_t len = 0; const char *message = kiln_c99_string_ascii_bytes(thrown, &len);
           fprintf(stderr, "generator call status=%d thrown-tag=%d %.*s\\n", status,
                   thrown.tag, (int)len, message ? message : "");
           return 40 + status;
       }} }}
    if (generator.tag != KILN_VALUE_GENERATOR) return 30 + generator.tag;
    for (int i = 0; i < 5; ++i) {{
        if (kiln_c99_invoke_method(NULL, &result, KILN_C99_METHOD_GENERATOR_NEXT, generator,
                                   NULL, 0, 0, &thrown) != KILN_C99_OK) return 5;
    }}
    if (result.tag != KILN_VALUE_OBJECT) return 20 + result.tag;
    if (kiln_c99_get(NULL, result, "value", 5, &value, 0, &thrown) != KILN_C99_OK ||
        value.tag != KILN_VALUE_OBJECT) return 3;
    if (kiln_c99_get(NULL, result, "done", 4, &done, 0, &thrown) != KILN_C99_OK ||
        kiln_c99_truthy(done)) return 4;
    kiln_test_realm_active = 0;
    return kiln_realm_destroy(realm) == KILN_C99_OK ? 0 : 5;
}}
"#,
        symbol = compiled.symbol
    );
    let run = run_with_main(&compiled, &wrapper, "generic-looping-generator-tier");
    assert!(
        run.status.success(),
        "boxed generator C compiles and runs (status {:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn generic_mixed_tier_budget_is_spent_after_module_liveness() {
    let mut prog = program(
        (0..65)
            .map(|index| Constant::Number((index as f64).to_bits()))
            .collect(),
        vec![
            Instruction::MakeFunction(64),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        0,
    );
    prog.functions = (0..65)
        .map(|index| function(vec![Instruction::Const(index), Instruction::Return]))
        .collect();
    let emitted = emit_module_with_logical_path_and_policy(
        &module(prog, Vec::new(), Vec::new()),
        "retained-tier.js",
        EmissionPolicy {
            compact: true,
            generic_only: true,
            live_module_slots: Some(std::sync::Arc::new(Default::default())),
            unit_chunk_bytes: Some(1),
            ..Default::default()
        },
    )
    .expect("module emits");
    assert!(emitted.generic_mixed_optimization);
    assert!(
        emitted.chunks.is_empty(),
        "one retained body needs only one unit"
    );
    assert!(emitted.source.contains("_fn_64"));
    assert!(
        !emitted.source.contains("defined(KILN_GENERIC_MIXED_TIER)"),
        "the only retained function receives the optimized tier"
    );
}

#[test]
fn generic_mixed_tier_metadata_tracks_each_emitted_chunk() {
    let selected = function(vec![Instruction::Return]);
    let mut unsupported = function(vec![Instruction::Return]);
    unsupported.generator = true;
    let mut prog = program(
        Vec::new(),
        vec![
            Instruction::MakeFunction(0),
            Instruction::MakeFunction(1),
            Instruction::Return,
        ],
        0,
    );
    prog.functions = vec![selected, unsupported];
    let emitted = emit_module_with_logical_path_and_policy(
        &module(prog, Vec::new(), Vec::new()),
        "tier-test.js",
        EmissionPolicy {
            compact: true,
            generic_only: true,
            unit_chunk_bytes: Some(1),
            ..Default::default()
        },
    )
    .expect("chunked generic module emits");

    assert!(emitted.generic_mixed_optimization);
    assert!(emitted.chunks.len() >= 2);
    assert_eq!(
        emitted.chunk_generic_mixed_optimization.len(),
        emitted.chunks.len()
    );
    assert!(emitted.chunk_generic_mixed_optimization[0]);
    assert!(
        emitted
            .chunk_generic_mixed_optimization
            .iter()
            .any(|mixed| !mixed)
    );
}

#[test]
fn compact_mode_emits_closure_metadata_table_and_compact_call_sites() {
    let mut f0 = function(vec![Instruction::Return]);
    f0.name = Some("target".into());
    let mut prog = program(
        Vec::new(),
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
    );
    prog.functions = vec![f0];
    let emitted = emit_with_policy(
        &prog,
        EmissionPolicy {
            compact: true,
            generic_only: true,
            ..Default::default()
        },
    )
    .expect("program emits");
    assert!(
        emitted
            .source
            .contains("static const ku_proto_meta ku_protos[1] = {")
    );
    assert!(
        emitted
            .source
            .contains("static __attribute__((unused)) int ku_make_closure(")
    );
    assert!(
        emitted
            .source
            .contains("#define KU_CLOSURE(DST, PROTO, RECV, CAP, CAP_CNT)")
    );
    assert!(
        emitted
            .source
            .contains("KU_CLOSURE(v0, 0, KILN_VAL_UNDEFINED, NULL, 0);")
    );
}

#[test]
fn executable_compact_closure_executes_correctly() {
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
    let generated = emit_with_policy(
        &Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
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
                Instruction::LoadCell(0),
                Instruction::Return,
            ],
            local_count: 2,
            functions: vec![bump],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        EmissionPolicy {
            compact: true,
            generic_only: true,
            ..Default::default()
        },
    )
    .expect("program emits");
    let wrapper = format!(
        "int main(void) {{ kiln_value result; int status = {}(NULL, &result); if (status != 0) return 2; if (result.tag != KILN_VALUE_NUMBER) return 3; return kiln_c99_as_number(result) == 2.0 ? 0 : 4; }}\n",
        generated.symbol
    );
    let run = run_with_main(&generated, &wrapper, "compact-closure-cell");
    assert!(
        run.status.success(),
        "run stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
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
fn emitted_async_module_emits_publish_exports() {
    let generated = emit_module(&module(
        program(
            vec![Constant::Number(42.0f64.to_bits())],
            vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Await,
                Instruction::Pop,
                Instruction::Return,
            ],
            1,
        ),
        vec![],
        vec![kiln_ir::ExportBinding {
            exported: "answer".into(),
            local_slot: 0,
        }],
    ))
    .expect("async module emits");
    assert!(generated.source.contains(&format!(
        "void {}_publish_exports(kiln_host *host, kiln_value *exports)",
        generated.symbol
    )));
    assert!(
        generated
            .source
            .contains("if (roots_status != KILN_C99_OK) return roots_status;")
    );
}

#[test]
fn emitted_module_publishes_exported_object_literal() {
    let generated = emit_module(&module(
        program(
            vec![
                Constant::String("layer".into()),
                Constant::Number(42.0f64.to_bits()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::Const(1),
                Instruction::NewObject { count: 1 },
                Instruction::StoreLocal(0),
                Instruction::Const(1),
                Instruction::Return,
            ],
            1,
        ),
        vec![],
        vec![kiln_ir::ExportBinding {
            exported: "service".into(),
            local_slot: 0,
        }],
    ))
    .expect("module emits");
    assert!(
        generated
            .source
            .contains("kiln_c99_new_object_with_capacity"),
        "exported object literal must not be scalar-replaced: emitted source was:\n{}",
        generated.source
    );
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
    let sources = crate::prelude_source_parts();

    let mut unnamed = Vec::new();
    let mut internal_by_function: Vec<(String, usize)> = Vec::new();
    for (path, source) in sources {
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
fn emitted_static_object_literal_uses_cached_shape_and_inline_slots() {
    // 1. All numbers: no dynamic mask
    let prog_numbers = program(
        vec![
            Constant::String("x".into()),
            Constant::Number(10.0f64.to_bits()),
            Constant::String("y".into()),
            Constant::Number(20.0f64.to_bits()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Const(3),
            Instruction::NewObject { count: 2 },
            Instruction::Return,
        ],
        0,
    );
    let gen_numbers = emit(&prog_numbers).expect("program emits").source;
    assert!(
        gen_numbers.contains("static const kiln_shape *lit_shape = NULL;"),
        "expected cached shape in emitted source:\n{gen_numbers}"
    );
    assert!(
        gen_numbers.contains("kiln_c99_heap_store_val(obj, &obj->inline_slots[0],"),
        "expected inline slot 0 store in emitted source:\n{gen_numbers}"
    );
    assert!(
        gen_numbers.contains("kiln_c99_heap_store_val(obj, &obj->inline_slots[1],"),
        "expected inline slot 1 store in emitted source:\n{gen_numbers}"
    );
    assert!(
        gen_numbers.contains("obj->shape = (kiln_shape *)lit;"),
        "expected shape assignment in emitted source:\n{gen_numbers}"
    );
    assert!(
        !gen_numbers.contains("uint32_t mask ="),
        "all-number object literal should not generate dynamic mask:\n{gen_numbers}"
    );

    // 2. Mixed types (string value): dynamic mask
    let prog_mixed = program(
        vec![
            Constant::String("x".into()),
            Constant::Number(10.0f64.to_bits()),
            Constant::String("name".into()),
            Constant::String("test".into()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Const(3),
            Instruction::NewObject { count: 2 },
            Instruction::Return,
        ],
        0,
    );
    let gen_mixed = emit(&prog_mixed).expect("program emits").source;
    assert!(
        gen_mixed.contains("static const kiln_shape *lit_shape = NULL;"),
        "expected cached shape in emitted source:\n{gen_mixed}"
    );
    assert!(
        gen_mixed.contains("uint32_t mask ="),
        "mixed-type object literal should generate dynamic mask:\n{gen_mixed}"
    );
    assert!(
        gen_mixed.contains("lit->double_slots_mask == mask"),
        "mixed-type object literal should guard on mask:\n{gen_mixed}"
    );

    // 3. Dynamic key or >8 keys or __proto__: fallback to new_object_with_capacity
    let prog_proto = program(
        vec![
            Constant::String("__proto__".into()),
            Constant::Number(10.0f64.to_bits()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::NewObject { count: 1 },
            Instruction::Return,
        ],
        0,
    );
    let gen_proto = emit(&prog_proto).expect("program emits").source;
    assert!(
        !gen_proto.contains("static const kiln_shape *lit_shape = NULL;"),
        "__proto__ key must not use cached shape:\n{gen_proto}"
    );
    assert!(
        gen_proto.contains("kiln_c99_new_object_with_capacity"),
        "fallback must use new_object_with_capacity:\n{gen_proto}"
    );
}

#[test]
fn compact_module_pools_inline_cache_sites_and_interned_keys_into_dense_tables() {
    let function = |local_count, code| FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: None,
        parameter_count: 0,
        local_count,
        code,
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![
                Constant::String("foo".to_owned().into()),
                Constant::String("bar".to_owned().into()),
                Constant::Undefined,
            ],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::LoadLocal(0),
                Instruction::GetProperty(1),
                Instruction::Return,
            ],
            local_count: 1,
            functions: vec![function(
                1,
                vec![
                    Instruction::Const(2),
                    Instruction::GetProperty(0),
                    Instruction::Return,
                ],
            )],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "getFoo".into(),
            local_slot: 0,
        }],
    );
    let emitted = emit_module_with_logical_path_and_policy(
        &module,
        "dense_sites.js",
        EmissionPolicy {
            compact: true,
            ..EmissionPolicy::default()
        },
    )
    .expect("compact module with sites emits");

    // 1. Dense site table at file scope
    assert!(
        emitted
            .source
            .contains("static kiln_c99_site ku_sites[2] = {0};")
    );
    // 2. Function site macro pointing into ku_sites
    assert!(
        emitted
            .source
            .contains("#define ku_fn_0_site (&ku_sites[0])")
    );
    // 3. Entry site macro pointing into ku_sites
    assert!(
        emitted
            .source
            .contains("#define ku_entry_site (&ku_sites[1])")
    );
    // 4. Dense key table array and macros
    assert!(
        emitted
            .source
            .contains("static const kiln_string *ku_keys[2];")
    );
    assert!(emitted.source.contains("#define ku_key_0 (ku_keys[0])"));
    assert!(emitted.source.contains("#define ku_key_1 (ku_keys[1])"));
    // 5. No individual scattered site arrays or static key pointers
    assert!(!emitted.source.contains("static kiln_c99_site ku_fn_0_site"));
    assert!(
        !emitted
            .source
            .contains("static const kiln_string *ku_key_0;")
    );

    // 6. Valid C99 compilation and execution
    let wrapper = r#"
int main(void) {
    kiln_host host;
    memset(&host, 0, sizeof(host));
    kiln_value result = kiln_c99_undefined();
    kiln_value exports[1] = { kiln_c99_undefined() };
    int status = ku_body(&host, &result, NULL, 0, exports, 1);
    if (status != KILN_C99_OK) return 1;
    if (ku_keys_shared != 1) return 2;
    if (ku_keys[0] == NULL || ku_keys[1] == NULL) return 3;
    return 0;
}
"#;
    let run = run_with_main(&emitted, wrapper, "dense-sites");
    assert!(
        run.status.success(),
        "dense sites executable failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn compact_module_with_many_keys_uses_dense_static_key_array_and_loop() {
    let module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![
                Constant::String("k0".to_owned().into()),
                Constant::String("k1".to_owned().into()),
                Constant::String("k2".to_owned().into()),
                Constant::String("k3".to_owned().into()),
                Constant::String("k4".to_owned().into()),
                Constant::String("k5".to_owned().into()),
                Constant::Number(0.0f64.to_bits()),
            ],
            code: vec![
                Instruction::Const(6),
                Instruction::GetProperty(0),
                Instruction::GetProperty(1),
                Instruction::GetProperty(2),
                Instruction::GetProperty(3),
                Instruction::GetProperty(4),
                Instruction::GetProperty(5),
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        Vec::new(),
    );
    let emitted = emit_module_with_logical_path_and_policy(
        &module,
        "dense_keys.js",
        EmissionPolicy {
            compact: true,
            ..EmissionPolicy::default()
        },
    )
    .expect("compact module with 6 keys emits");

    // Dense key table with >4 keys emits static array and loop
    assert!(
        emitted
            .source
            .contains("static const kiln_string *const ku_key_sources[6] = {")
    );
    assert!(
        emitted
            .source
            .contains("for (size_t i = 0; i < (size_t)6; ++i) {")
    );
    assert!(
        emitted
            .source
            .contains("if ((ku_keys[i] = kiln_c99_intern_static_key(ku_key_sources[i])) == NULL) return KILN_C99_OUT_OF_MEMORY;")
    );
    assert!(emitted.source.contains(
        "if (!kiln_c99_register_static_keys(ku_key_sources, ku_keys, (size_t)6)) return;"
    ));

    let wrapper = r#"
int main(void) {
    kiln_host host;
    memset(&host, 0, sizeof(host));
    kiln_value result = kiln_c99_undefined();
    int status;
    if (ku_keys_shared != 1) return 4;
    for (size_t i = 0; i < 6; ++i) {
        if (ku_keys[i] == NULL || !kiln_c99_key_is_shared(ku_keys[i])) return 5;
    }
    status = ku_body(&host, &result, NULL, 0, NULL, 0);
    (void)status;
    if (ku_keys_shared != 1) return 2;
    for (size_t i = 0; i < 6; ++i) {
        if (ku_keys[i] == NULL) return 3;
    }
    return 0;
}
"#;
    let run = run_with_main(&emitted, wrapper, "dense-keys");
    assert!(
        run.status.success(),
        "dense keys executable failed ({:?}): {}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
}

#[test]
fn split_instantiation_call_sites_use_declared_shared_receiver_helper() {
    let mut module = super::module(
        super::program(
            vec![
                Constant::Number(1.0f64.to_bits()),
                Constant::String("toString".into()),
            ],
            vec![
                Instruction::Const(0),
                Instruction::CallMethod { name: 1, argc: 0 },
                Instruction::Return,
            ],
            0,
        ),
        Vec::new(),
        Vec::new(),
    );
    module.link_facts.module_instantiation_end = 2;

    let emitted = emit_module_with_logical_path_and_policy_and_instantiation(
        &module,
        "fixture/instantiate-method.js",
        EmissionPolicy::default(),
        true,
    )
    .expect("split module emits");
    let instantiation = emitted.instantiation.expect("instantiation entry emitted");

    assert!(
        instantiation
            .source
            .contains("kiln_c99_normalize_closure_receiver")
    );
    assert!(runtime_prelude_header().contains(
        "extern int kiln_c99_normalize_closure_receiver(kiln_val *receiver, const kiln_closure *closure);"
    ));
}
