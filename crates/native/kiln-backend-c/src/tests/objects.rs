use super::*;

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
