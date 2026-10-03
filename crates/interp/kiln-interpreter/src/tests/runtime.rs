use super::*;

/// `in` asks about presence, which is not the same question as reading and comparing to
/// undefined: a property explicitly set to undefined is present.
#[test]
fn in_tests_presence_rather_than_value() {
    let object = object(&[("set_to_undefined", Value::Undefined)]);
    assert!(has_property(
        &Intrinsics::default(),
        &object,
        &Value::String("set_to_undefined".to_owned().into())
    ));
    assert!(!has_property(
        &Intrinsics::default(),
        &object,
        &Value::String("absent".to_owned().into())
    ));
}

/// `join` treats null and undefined as empty rather than stringifying them, which is the one
/// place its output differs from mapping `String` over the elements.
#[test]
fn join_renders_nullish_elements_as_empty() {
    let items = [
        Value::Number(1.0),
        Value::Undefined,
        Value::Null,
        Value::String("x".to_owned().into()),
    ];
    assert_eq!(join(&items, ",").unwrap(), "1,,,x");
}

#[test]
fn error_cause_is_installed_by_presence_not_by_value() {
    let with_cause = build_error(
        ErrorKind::Error,
        &[
            Value::String("m".to_owned().into()),
            object(&[("cause", Value::Undefined)]),
        ],
    );
    let Value::Object(data) = &with_cause else {
        panic!("construct_error returns an object");
    };
    assert_eq!(data.borrow().get("cause"), Some(Value::Undefined));

    let without = build_error(ErrorKind::Error, &[Value::String("m".to_owned().into())]);
    let Value::Object(data) = &without else {
        panic!("construct_error returns an object");
    };
    assert_eq!(data.borrow().get("cause"), None, "no options, no cause");
}

/// A cause is carried by identity: the error holds the same object the caller passed.
#[test]
fn error_cause_keeps_the_identity_of_the_value_given() {
    let inner = object(&[("marker", Value::Number(1.0))]);
    let outer = build_error(
        ErrorKind::Error,
        &[
            Value::String("outer".to_owned().into()),
            object(&[("cause", inner.clone())]),
        ],
    );
    let Value::Object(data) = &outer else {
        panic!("construct_error returns an object");
    };
    let cause = data.borrow().get("cause").expect("cause present");
    assert!(strict_equal(&cause, &inner), "cause is the same object");
}

#[test]
fn error_is_error_inherits_through_native_error_constructors() {
    let intrinsics = Intrinsics::default();
    let type_error = Value::Native(Native::Error(ErrorKind::Type), RealmId::PRIMARY);
    assert!(matches!(
        native_inherited_property(&intrinsics, &type_error, "isError"),
        Value::Native(Native::ErrorIsError, _)
    ));
    assert!(matches!(
        prototype_of(&type_error),
        Value::Native(Native::Error(ErrorKind::Error), _)
    ));
}

/// The low-level constructor preserves an absent message; the public lazy stack formatter is
/// installed by the shared TypeScript Error builtin, outside this constructor unit fixture.
#[test]
fn an_empty_message_stays_absent_before_stack_formatter_registration() {
    for arguments in [Vec::new(), vec![Value::Undefined]] {
        let Value::Object(data) = build_error(ErrorKind::Error, &arguments) else {
            panic!("construct_error returns an object");
        };
        assert_eq!(data.borrow().get_own("stack"), None);
        assert_eq!(data.borrow().get_own("message"), None);
    }
}

/// `name` is the prototype's, never the instance's.
///
/// An own one would shadow a subclass's `name` class field, which is installed on the
/// instance while the constructor runs -- so `e.name` answered "Error" for an error the
/// program had explicitly named, and `if (e.name === "TimeoutError")` took the wrong branch.
#[test]
fn an_error_instance_has_no_own_name() {
    let Value::Object(data) = build_error(ErrorKind::Type, &[Value::String("m".to_owned().into())])
    else {
        panic!("construct_error returns an object");
    };
    let data = data.borrow();
    assert_eq!(data.get_own("name"), None);
    assert_eq!(
        data.get_own("message"),
        Some(&Value::String("m".to_owned().into())),
        "a message that was passed is still the instance's own"
    );
}

/// A `DataView` reads *big*-endian unless told otherwise -- the opposite of the platform and
/// of every typed array. Getting it backwards round-trips cleanly through the same view, so
/// nothing inside a program notices; it only shows when the bytes meet something outside
/// one, which is exactly what a `DataView` is for.
#[test]
fn a_data_view_defaults_to_the_opposite_order_from_the_platform() {
    let mut written = [0_u8; 2];
    TypedKind::Int16.encode(&Value::Number(258.0), &mut written);
    order_bytes(&mut written, false);
    assert_eq!(written, [0x01, 0x02], "the high byte leads");

    let mut same_order = written;
    order_bytes(&mut same_order, false);
    assert_eq!(TypedKind::Int16.decode(&same_order), Value::Number(258.0));

    let mut other_order = written;
    order_bytes(&mut other_order, true);
    assert_eq!(
        TypedKind::Int16.decode(&other_order),
        Value::Number(513.0),
        "the same bytes read the other way round"
    );
}

/// The two numeric towers do not cross, in either direction.
#[test]
fn an_element_refuses_the_other_numeric_tower() {
    let one = Value::Number(1.0);
    let big = Value::BigInt(Rc::new(num_bigint::BigInt::from(1)));
    assert!(coerce_element(TypedKind::BigInt64, &one).is_err());
    assert!(coerce_element(TypedKind::Uint8, &big).is_err());
    assert_eq!(coerce_element(TypedKind::BigInt64, &big).unwrap(), big);
    assert_eq!(coerce_element(TypedKind::Uint8, &one).unwrap(), one);
}

/// `typeof null` is "object" -- a specification bug old enough to be load-bearing, and
/// distinct from the diagnostic name, which says "null" because that is more useful there.
#[test]
fn typeof_reports_the_language_tags_including_the_null_one() {
    assert_eq!(Value::Null.type_of(), "object");
    assert_eq!(Value::Null.type_name(), "null");
    assert_eq!(Value::Undefined.type_of(), "undefined");
    assert_eq!(Value::Number(1.0).type_of(), "number");
    assert_eq!(
        Value::Native(Native::Error(ErrorKind::Error), RealmId::PRIMARY).type_of(),
        "function"
    );
    assert_eq!(object(&[]).type_of(), "object");
}

#[test]
fn new_on_a_non_constructor_is_rejected() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: vec![number(1.0)],
        code: vec![
            Instruction::Const(0),
            Instruction::New { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "TypeError: a number is not a constructor"
    ));
}

/// Deleting during iteration must not skip the next element. Compacting the backing store
/// shifts everything past the cursor by one, which is the bug this design exists to avoid.
#[test]
fn deleting_leaves_a_hole_rather_than_compacting() {
    let entries = vec![Value::Number(1.0), Value::Number(2.0), Value::Number(3.0)];
    assert_eq!(find_entry(&entries, &Value::Number(2.0), false), Some(1));

    let with_hole = vec![
        Value::Number(1.0),
        Value::Native(Native::Tombstone, RealmId::PRIMARY),
        Value::Number(3.0),
    ];
    assert_eq!(
        find_entry(&with_hole, &Value::Number(3.0), false),
        Some(2),
        "a hole must not shift the indices after it"
    );
    assert_eq!(
        find_entry(&with_hole, &Value::Number(2.0), false),
        None,
        "a deleted entry is not found"
    );
}

/// An exception with no handler is an execution error naming what was thrown, rather than a
/// silent stop or a panic.
#[test]
fn an_uncaught_throw_names_what_was_thrown() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: vec![Constant::String("boom".to_owned().into())],
        code: vec![Instruction::Const(0), Instruction::Throw],
        local_count: 0,
        functions: Vec::new(),
    };
    let Err(ExecutionError::Uncaught { message, .. }) = run(&program) else {
        panic!("an unhandled throw must not succeed");
    };
    assert_eq!(message, "boom");
}

/// A handler restores the stack to the height it had when the handler was registered, so
/// values left by the abandoned code cannot leak into the catch block.
#[test]
fn unwinding_restores_the_stack_the_handler_expected() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: vec![
            number(1.0),
            number(2.0),
            Constant::String("x".to_owned().into()),
        ],
        code: vec![
            Instruction::Const(0), // a value the handler must still see
            Instruction::PushHandler(6),
            Instruction::Const(1), // debris left by the abandoned try block
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Throw,
            // handler: the thrown value is on top, and the sentinel is beneath it
            Instruction::Pop,
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    assert_eq!(
        run(&program),
        Ok(Value::Number(1.0)),
        "the sentinel from before the handler survives, the debris does not"
    );
}

/// An error renders through its inherited `toString`; any other value through its ordinary
/// string form, because JavaScript allows throwing anything.
#[test]
fn a_thrown_value_is_described_by_what_it_is() {
    let error = object(&[
        ("name", Value::String("E".to_owned().into())),
        ("message", Value::String("boom".to_owned().into())),
        (
            "toString",
            Value::Native(Native::ErrorToString, RealmId::PRIMARY),
        ),
    ]);
    assert_eq!(describe_thrown(&error), "E: boom");
    assert_eq!(describe_thrown(&Value::Number(42.0)), "42");
    assert_eq!(describe_thrown(&Value::Undefined), "undefined");
}

/// A closure keeps working after the call that created it has returned, which is the whole
/// reason captures are cells rather than pointers into a frame.
#[test]
fn a_captured_cell_outlives_the_frame_that_created_it() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: vec![number(5.0)],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("outer".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                is_class: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 1,
                code: vec![
                    Instruction::NewCell(0),
                    Instruction::Const(0),
                    Instruction::StoreCell(0),
                    Instruction::MakeFunction(1),
                    Instruction::Return,
                ],
                upvalues: Vec::new(),
                type_annotations: None,
            },
            kiln_ir::FunctionProto {
                inferred_name: None,
                span: kiln_ir::Span::default(),
                prologue_end: 0,
                asynchronous: false,
                name: Some("read".to_owned()),
                arrow: false,
                generator: false,
                async_generator: false,
                is_method: false,
                is_class: false,
                parameter_count: 0,
                declared_length: 0,
                mapped_arguments: false,
                local_count: 0,
                code: vec![Instruction::LoadUpvalue(0), Instruction::Return],
                upvalues: vec![Upvalue::ParentLocal(0)],
                type_annotations: None,
            },
        ],
    };
    assert_eq!(run(&program), Ok(Value::Number(5.0)));
}

#[test]
fn a_call_binds_arguments_to_parameters_in_order() {
    let program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
    assert_eq!(run(&program), Ok(Value::Number(2.0)));
}

/// Wrong arity is not an error in JavaScript, so it is not one here either.
#[test]
fn missing_arguments_are_undefined_and_extra_ones_are_discarded() {
    let short = calling_program(1, vec![Instruction::Const(0)]);
    assert_eq!(
        run(&short),
        Ok(Value::Undefined),
        "second parameter unbound"
    );

    let long = calling_program(
        3,
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Const(2),
        ],
    );
    assert_eq!(run(&long), Ok(Value::Number(2.0)), "third argument dropped");
}

/// The caller's operand stack must survive the call untouched.
#[test]
fn a_call_leaves_only_its_result_on_the_caller_stack() {
    let mut program = calling_program(2, vec![Instruction::Const(0), Instruction::Const(1)]);
    // Push a sentinel first; it must still be underneath the result afterwards.
    program.code.insert(0, Instruction::Const(2));
    program.code.pop();
    program.code.push(Instruction::Pop);
    program.code.push(Instruction::Return);
    assert_eq!(run(&program), Ok(Value::Number(3.0)), "sentinel survived");
}

#[test]
fn calling_a_non_function_is_an_error_rather_than_a_coercion() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: vec![number(1.0)],
        code: vec![
            Instruction::Const(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
    };
    // A TypeError the program could have caught, not a fatal stop: reaching the top with
    // no handler is what makes it an `Uncaught`, and the message names the kind.
    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "TypeError: a number is not a function"
    ));
}

/// Unbounded recursion has to be reported, not abort the process by exhausting the host
/// stack. The step limit alone does not do it: frames are consumed far faster than steps.
#[test]
fn runaway_recursion_is_bounded_by_call_depth() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: Vec::new(),
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("forever".to_owned()),
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
                Instruction::MakeFunction(0),
                Instruction::Call { argc: 0 },
                Instruction::Return,
            ],
            upvalues: Vec::new(),
            type_annotations: None,
        }],
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let limits = ExecutionLimits {
        max_call_depth: 8,
        ..ExecutionLimits::default()
    };
    // Reported as a thrown `RangeError`, which nothing here catches, rather than as a
    // fatal status: the bound is still enforced, and a program that wants to survive its own
    // recursion can now write the `try` that does it.
    let outcome = execute(
        Rc::new(program.clone()),
        &mut host,
        limits,
        ExecutionCapabilities::default(),
    );
    assert!(
        matches!(&outcome, Err(ExecutionError::Uncaught { message, .. })
                if message.contains("Maximum call stack size exceeded")),
        "expected an uncaught RangeError, got {outcome:?}"
    );
}

#[test]
fn an_unknown_function_index_is_rejected_rather_than_indexed() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: Vec::new(),
        code: vec![Instruction::MakeFunction(3), Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::UnknownFunction { function: 3 })
    ));
}

#[test]
fn executes_arithmetic_and_reports_instruction_count() {
    let program = program(
        vec![number(2.0), number(3.0)],
        vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ],
    );
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let result = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .unwrap();
    assert_eq!(result.value, Value::Number(5.0));
    assert_eq!(result.steps, 4);
}

#[test]
fn a_marked_local_read_throws_a_reference_error() {
    // Lexical slots are marked on scope entry, so a direct read must inspect the marker just
    // like a captured cell does. This is deliberately hand-written bytecode: it catches a
    // regression where `LoadLocal` silently returned the marker as `undefined`.
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: Vec::new(),
        code: vec![
            Instruction::MarkUninitialized(0),
            Instruction::LoadLocal(0),
            Instruction::Return,
        ],
        local_count: 1,
        functions: Vec::new(),
    };
    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "ReferenceError: cannot access a binding before its declaration"
    ));
}

#[test]
fn conditional_jumps_consume_conditions() {
    let program = program(
        vec![Constant::Boolean(false), number(1.0), number(2.0)],
        vec![
            Instruction::Const(0),
            Instruction::JumpIfFalse(4),
            Instruction::Const(1),
            Instruction::Jump(5),
            Instruction::Const(2),
            Instruction::Return,
        ],
    );
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let result = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .unwrap();
    assert_eq!(result.value, Value::Number(2.0));
}

#[test]
fn nullish_jump_keeps_non_nullish_left_operand() {
    // This is the shape emitted for `left ?? right`: duplicate the left value, consume the
    // duplicate for the branch, and leave the selected operand on the stack.
    let program = program(
        vec![
            Constant::String("left".into()),
            Constant::String("right".into()),
        ],
        vec![
            Instruction::Const(0),
            Instruction::Dup,
            Instruction::JumpIfNotNullish(6),
            Instruction::Pop,
            Instruction::Const(1),
            Instruction::Jump(6),
            Instruction::Return,
        ],
    );
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let result = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits::default(),
        ExecutionCapabilities::default(),
    )
    .unwrap();
    assert_eq!(result.value, Value::String("left".into()));
}

#[test]
fn step_limit_stops_an_infinite_program() {
    let program = program(Vec::new(), vec![Instruction::Jump(0)]);
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let error = execute(
        Rc::new(program.clone()),
        &mut host,
        ExecutionLimits {
            max_steps: 3,
            ..ExecutionLimits::default()
        },
        ExecutionCapabilities::default(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        ExecutionError::StepLimitExceeded { steps: 3, limit: 3 }
    );
}

#[test]
fn only_a_canonical_index_counts_as_one() {
    // `"01"` and `"1.0"` name the same property as themselves and not as 1, so they are
    // ordinary string keys and enumerate in insertion order.
    assert_eq!(index_key("0"), Some(0));
    assert_eq!(index_key("10"), Some(10));
    assert_eq!(index_key("01"), None);
    assert_eq!(index_key("1.0"), None);
    assert_eq!(index_key("-1"), None);
    assert_eq!(index_key(""), None);
    // 2^32 - 1 is the length, never an index.
    assert_eq!(index_key("4294967295"), None);
    assert_eq!(index_key("4294967294"), Some(4_294_967_294));
}

#[test]
fn a_negative_slice_bound_counts_from_the_end() {
    // The reason this is not a cast: `-2` means two from the end, not clamp-to-zero.
    assert_eq!(relative_index(Some(&Value::Number(-2.0)), 5, 0), 3);
    assert_eq!(relative_index(Some(&Value::Number(-99.0)), 5, 0), 0);
    assert_eq!(relative_index(Some(&Value::Number(2.0)), 5, 0), 2);
    assert_eq!(relative_index(Some(&Value::Number(99.0)), 5, 0), 5);
    // An absent or explicitly undefined bound takes the default, not zero.
    assert_eq!(relative_index(None, 5, 5), 5);
    assert_eq!(relative_index(Some(&Value::Undefined), 5, 5), 5);
}

#[test]
fn a_cycle_prints_once_rather_than_forever() {
    let items = ArrayData::new(Vec::new());
    items.borrow_mut().push(Value::Array(Rc::clone(&items)));
    assert_eq!(inspect(&Value::Array(items)), "[ [Circular *1] ]");
}

#[test]
fn nesting_past_the_depth_limit_becomes_a_placeholder() {
    // node stops descending at depth 2 and prints the kind instead.
    let deep = array(&[array(&[array(&[array(&[Value::Number(1.0)])])])]);
    assert_eq!(inspect(&deep), "[ [ [ [Array] ] ] ]");
}

#[test]
fn bitwise_operands_truncate_to_thirty_two_bits() {
    // Not a cast: the value wraps modulo 2^32 rather than saturating, which is why
    // `1 << 31` is negative and `2 ** 32 | 0` is zero.
    assert_eq!(to_int32(1.0), 1);
    assert_eq!(to_int32(2147483648.0), i32::MIN);
    assert_eq!(to_int32(4294967296.0), 0);
    assert_eq!(to_int32(-1.5), -1);
    assert_eq!(to_int32(f64::NAN), 0);
    assert_eq!(to_int32(f64::INFINITY), 0);
    assert_eq!(to_uint32(-1.0), u32::MAX);
}

#[test]
fn parse_int_reads_as_far_as_it_can() {
    // The whole difference from `Number(text)`, which is NaN for every one of these.
    let parsed = |text: &str, radix: Option<f64>| {
        let mut arguments = vec![Value::String(text.to_owned().into())];
        if let Some(radix) = radix {
            arguments.push(Value::Number(radix));
        }
        let Value::Number(result) = parse_int(&arguments) else {
            panic!("parseInt returns a number");
        };
        result
    };
    assert_eq!(parsed("12px", None), 12.0);
    assert_eq!(parsed("  -42  ", None), -42.0);
    assert_eq!(parsed("0x1f", None), 31.0);
    assert_eq!(parsed("ff", Some(16.0)), 255.0);
    assert!(parsed("px", None).is_nan());
    // A radix outside 2..=36 has no meaning, and neither does an empty prefix.
    assert!(parsed("12", Some(1.0)).is_nan());
    assert!(parsed("", None).is_nan());
}

#[test]
fn a_program_is_not_finished_while_it_still_has_work_scheduled() {
    // Returning at the end of the module body would make every timer unobservable.
    let program = timer_program(&[("scheduled", 5.0)]);
    assert_eq!(stdout_of(&program), "scheduled");
}

#[test]
fn an_async_arrow_captures_its_defining_receiver() {
    let maker = kiln_ir::FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        parameter_count: 0,
        declared_length: 0,
        mapped_arguments: false,
        local_count: 0,
        code: vec![Instruction::MakeFunction(1), Instruction::Return],
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let arrow = kiln_ir::FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: true,
        name: None,
        arrow: true,
        generator: true,
        async_generator: false,
        is_method: false,
        is_class: false,
        parameter_count: 0,
        declared_length: 0,
        mapped_arguments: false,
        local_count: 0,
        code: vec![
            Instruction::LoadThis,
            Instruction::GetProperty(1),
            Instruction::Return,
        ],
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let report = kiln_ir::FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        parameter_count: 1,
        declared_length: 1,
        mapped_arguments: false,
        local_count: 1,
        code: vec![
            Instruction::LoadLocal(0),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        constants: vec![
            Constant::String("make".into()),
            Constant::String("tag".into()),
            Constant::String("owner".into()),
            Constant::String("wrong".into()),
            Constant::String("call".into()),
            Constant::String("then".into()),
        ],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::NewObject { count: 1 },
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(0),
            Instruction::SetProperty(0),
            Instruction::Pop,
            Instruction::LoadLocal(1),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::StoreLocal(2),
            Instruction::Const(1),
            Instruction::Const(3),
            Instruction::NewObject { count: 1 },
            Instruction::StoreLocal(3),
            Instruction::LoadLocal(2),
            Instruction::LoadLocal(3),
            Instruction::CallMethod { name: 4, argc: 1 },
            Instruction::MakeFunction(2),
            Instruction::CallMethod { name: 5, argc: 1 },
            Instruction::Pop,
            Instruction::Return,
        ],
        local_count: 4,
        functions: vec![maker, arrow, report],
    };

    assert_eq!(stdout_of(&program), "owner");
}

/// What a table lists, it also describes -- and the other way round.
///
/// The raw name lists are deliberately supersets: `Array.prototype` and `%TypedArray%.prototype`
/// share one, so the array table names `set`, which only the view has.
/// `intrinsic_prototype_entries` is what resolves that, by dropping any name the value tables do
/// not answer -- so the *listing* is correct even though the list is not.
///
/// What must not drift is the agreement between that filtered listing and
/// `intrinsic_own_attributes`. `hasOwnProperty`, `getOwnPropertyNames` and
/// `getOwnPropertyDescriptor` are answered by different halves of this pair, and a name in one
/// and not the other is a property that exists depending on which question you ask -- which has
/// shipped more than once.
#[test]
fn listing_and_describing_agree() {
    // Collected rather than asserted one at a time: the whole point is to see the shape of a
    // drift, and failing on the first row hides how wide it is.
    let mut disagreements = Vec::new();
    for table in every_prototype_table() {
        let target = Value::Native(Native::PrototypeOf(table), RealmId::PRIMARY);
        let listed: Vec<String> = intrinsic_prototype_entries(table)
            .into_iter()
            .map(|(key, _)| key)
            .collect();
        // Swept over the union of this table's names *and* every other table's, so that the
        // check runs in both directions. Only sweeping this table's own list missed the
        // direction that actually bit: `Object.prototype`'s eleven members were described as own
        // by every prototype in the realm and listed by none, because the ownership test was
        // asking the *read* path, which carries a table its last step up to `Object.prototype`.
        let mut sweep: Vec<&str> = every_prototype_table()
            .into_iter()
            .flat_map(|other| intrinsic_prototype_method_names(other).iter().copied())
            .collect();
        sweep.sort_unstable();
        sweep.dedup();
        for name in sweep {
            let is_listed = listed.iter().any(|key| key == name);
            let is_described = intrinsic_own_attributes(&target, name).is_some();
            if is_listed != is_described {
                disagreements.push(format!(
                    "{table:?}.{name}: listed={is_listed} described={is_described}"
                ));
            }
        }
        // Everything the filtered listing holds must describe, including `constructor` and the
        // accessors, which the name list does not carry.
        for key in &listed {
            if intrinsic_own_attributes(&target, key).is_none() {
                disagreements.push(format!("{table:?}.{key}: listed=true described=false"));
            }
        }
    }
    assert!(
        disagreements.is_empty(),
        "listing and describing disagree on {} members:\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

/// Every callable a built-in prototype publishes has a `name` and a `length`.
///
/// The third axis of the same drift. A member's value comes from the per-table value functions,
/// but its `name` and `length` come from `intrinsic_identity`, which is keyed on the *`Native`
/// variant* rather than on the table and key. Nothing joins the two: publishing a method whose
/// variant has no arity tabulated gives a function with no own `length` and no own `name`, which
/// every `Function.prototype.bind` wrapper and every argument-count check in the ecosystem reads.
#[test]
fn every_published_callable_is_named_and_measured() {
    let mut missing = Vec::new();
    for table in every_prototype_table() {
        for (key, property) in intrinsic_prototype_entries(table) {
            let Property::Value(value) = property else {
                continue;
            };
            if value.type_of() != "function" {
                continue;
            }
            if intrinsic_identity(&value).is_none() {
                missing.push(format!("{table:?}.{key}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "published callables with no name or length: {}",
        missing.join(", ")
    );
}
