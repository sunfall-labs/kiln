use super::*;

fn proto(
    name: &str,
    code: Vec<Instruction>,
    parameter_count: u32,
    local_count: u32,
    arrow: bool,
    mapped_arguments: bool,
) -> kiln_ir::FunctionProto {
    kiln_ir::FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: Some(name.to_owned()),
        arrow,
        generator: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        parameter_count,
        declared_length: parameter_count,
        mapped_arguments,
        local_count,
        code,
        upvalues: Vec::new(),
        type_annotations: None,
    }
}

fn program_with_functions(
    constants: Vec<Constant>,
    code: Vec<Instruction>,
    local_count: u32,
    functions: Vec<kiln_ir::FunctionProto>,
) -> Program {
    Program {
        source: String::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants,
        code,
        local_count,
        functions,
    }
}

fn host() -> TestHost {
    TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: Default::default(),
        timezone: None,
        locale: None,
    }
}

fn length_reader(mapped_arguments: bool) -> kiln_ir::FunctionProto {
    proto(
        "lengthReader",
        vec![
            Instruction::LoadArguments,
            Instruction::GetProperty(0),
            Instruction::Return,
        ],
        0,
        0,
        false,
        mapped_arguments,
    )
}

fn root_function<'a>(program: &'a Program, host: &'a mut TestHost) -> (Vm<'a, TestHost>, Value) {
    let mut vm = test_vm(program, host);
    let value = vm.run().expect("module returns the function").value;
    (vm, value)
}

#[test]
fn length_only_function_uses_the_actual_argument_count_including_surplus_arguments() {
    let program = program_with_functions(
        vec![Constant::String("length".into())],
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![length_reader(false)],
    );
    let mut host = host();
    let (mut vm, function) = root_function(&program, &mut host);
    let Value::Function(closure) = &function else {
        panic!("MakeFunction returns a closure");
    };
    assert!(closure.arguments_length_only);

    for (arguments, expected) in [
        (Vec::new(), 0.0),
        (vec![Value::Number(10.0)], 1.0),
        (
            vec![
                Value::Number(10.0),
                Value::Number(20.0),
                Value::Number(30.0),
            ],
            3.0,
        ),
    ] {
        assert_eq!(
            vm.call_value(&function, Value::Undefined, arguments, 0),
            Ok(Value::Number(expected)),
        );
    }
}

#[test]
fn arguments_object_observes_indices_and_mutation_when_the_body_needs_the_object() {
    let function = proto(
        "objectReader",
        vec![Instruction::LoadArguments, Instruction::Return],
        1,
        1,
        false,
        true,
    );
    let program = program_with_functions(
        Vec::new(),
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![function],
    );
    let mut host = host();
    let (mut vm, function) = root_function(&program, &mut host);
    let Value::Function(closure) = &function else {
        panic!("MakeFunction returns a closure");
    };
    assert!(!closure.arguments_length_only);

    let arguments = vm
        .call_value(
            &function,
            Value::Undefined,
            vec![Value::Number(11.0), Value::Number(22.0)],
            0,
        )
        .expect("ordinary arguments-object read succeeds");
    let Value::Object(arguments) = arguments else {
        panic!("arguments is an ordinary object");
    };
    assert_eq!(
        arguments.borrow().get_own("length"),
        Some(&Value::Number(2.0))
    );
    assert_eq!(arguments.borrow().get_own("0"), Some(&Value::Number(11.0)));
    arguments.borrow_mut().set("0", Value::Number(99.0));
    assert_eq!(arguments.borrow().get_own("0"), Some(&Value::Number(99.0)));
    assert_eq!(arguments.borrow().get_own("callee"), Some(&function));
}

#[test]
fn a_local_alias_of_sloppy_arguments_remains_the_same_mutable_object() {
    let function = proto(
        "aliasArguments",
        vec![
            Instruction::LoadArguments,
            Instruction::StoreLocal(0),
            Instruction::LoadLocal(0),
            Instruction::Const(2),
            Instruction::SetProperty(1),
            Instruction::Pop,
            Instruction::LoadArguments,
            Instruction::GetProperty(1),
            Instruction::Return,
        ],
        1,
        1,
        false,
        true,
    );
    let program = program_with_functions(
        vec![
            Constant::String("length".into()),
            Constant::String("0".into()),
            number(99.0),
        ],
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![function],
    );
    let mut host = host();
    let (mut vm, function) = root_function(&program, &mut host);
    let Value::Function(closure) = &function else {
        panic!("MakeFunction returns a closure");
    };
    assert!(!closure.arguments_length_only);
    assert_eq!(
        vm.call_value(&function, Value::Undefined, vec![Value::Number(11.0)], 0,),
        Ok(Value::Number(99.0)),
        "writes through a local alias are visible through the arguments binding",
    );
}

#[test]
fn unused_arrow_does_not_capture_arguments_but_a_descendant_reader_does() {
    let unused = program_with_functions(
        vec![number(7.0)],
        vec![
            Instruction::MakeFunction(0),
            Instruction::Const(0),
            Instruction::Call { argc: 1 },
            Instruction::Return,
        ],
        0,
        vec![
            proto(
                "parent",
                vec![Instruction::MakeFunction(1), Instruction::Return],
                0,
                0,
                false,
                false,
            ),
            proto(
                "unusedArrow",
                vec![Instruction::Const(0), Instruction::Return],
                0,
                0,
                true,
                false,
            ),
        ],
    );
    let mut host = host();
    let unused = run_vm(&unused, &mut host);
    let Value::Function(unused) = unused else {
        panic!("outer call returns the unused arrow");
    };
    assert!(unused.captured_arguments.is_none());

    let length_arrow_program = program_with_functions(
        vec![Constant::String("length".into())],
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![
            proto(
                "parent",
                vec![Instruction::MakeFunction(1), Instruction::Return],
                0,
                0,
                false,
                false,
            ),
            proto(
                "lengthArrow",
                vec![
                    Instruction::LoadArguments,
                    Instruction::GetProperty(0),
                    Instruction::Return,
                ],
                0,
                0,
                true,
                false,
            ),
        ],
    );
    let mut host = host();
    let mut vm = test_vm(&length_arrow_program, &mut host);
    let parent = vm.run().expect("module creates parent").value;
    let arrow = vm
        .call_value(
            &parent,
            Value::Undefined,
            vec![Value::Number(1.0), Value::Number(2.0), Value::Number(3.0)],
            0,
        )
        .expect("parent creates length-reading arrow");
    let Value::Function(length_arrow) = &arrow else {
        panic!("returned value is an arrow");
    };
    assert!(length_arrow.captured_arguments.is_some());
    assert_eq!(
        vm.call_value(&arrow, Value::Undefined, Vec::new(), 0),
        Ok(Value::Number(3.0)),
        "a length-only arrow reads the enclosing call's actual count",
    );

    let nested = program_with_functions(
        vec![
            Constant::String("length".into()),
            Constant::String("0".into()),
        ],
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![
            proto(
                "parent",
                vec![
                    Instruction::LoadArguments,
                    Instruction::GetProperty(0),
                    Instruction::Pop,
                    Instruction::MakeFunction(1),
                    Instruction::Return,
                ],
                0,
                0,
                false,
                false,
            ),
            proto(
                "middleArrow",
                vec![Instruction::MakeFunction(2), Instruction::Return],
                0,
                0,
                true,
                false,
            ),
            proto(
                "nestedReader",
                vec![
                    Instruction::LoadArguments,
                    Instruction::GetProperty(1),
                    Instruction::Return,
                ],
                0,
                0,
                true,
                false,
            ),
        ],
    );
    let mut host = host();
    let mut vm = test_vm(&nested, &mut host);
    let parent = vm.run().expect("module creates parent").value;
    let Value::Function(parent_closure) = &parent else {
        panic!("module creates parent closure");
    };
    let middle = vm
        .call_value(
            &parent,
            Value::Undefined,
            vec![Value::Number(1.0), Value::Number(2.0), Value::Number(3.0)],
            0,
        )
        .expect("parent creates middle arrow");
    assert!(
        !parent_closure.arguments_length_only,
        "a child arrow that observes an index prevents eliding the parent's length proof"
    );
    let Value::Function(middle_closure) = &middle else {
        panic!("middle value is an arrow");
    };
    assert!(middle_closure.captured_arguments.is_some());
    let leaf = vm
        .call_value(&middle, Value::Undefined, Vec::new(), 0)
        .expect("middle arrow creates nested arrow");
    let Value::Function(leaf_closure) = &leaf else {
        panic!("leaf value is an arrow");
    };
    assert!(leaf_closure.captured_arguments.is_some());
    assert_eq!(
        vm.call_value(&leaf, Value::Undefined, Vec::new(), 0),
        Ok(Value::Number(1.0)),
        "the descendant reads index zero from the lexical parent's arguments object",
    );
}

#[test]
fn arrow_this_and_new_target_captures_do_not_require_an_arguments_capture() {
    let this_program = program_with_functions(
        Vec::new(),
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![
            proto(
                "parent",
                vec![Instruction::MakeFunction(1), Instruction::Return],
                0,
                0,
                false,
                false,
            ),
            proto(
                "thisArrow",
                vec![Instruction::LoadThis, Instruction::Return],
                0,
                0,
                true,
                false,
            ),
        ],
    );
    let mut host = host();
    let mut vm = test_vm(&this_program, &mut host);
    let parent = vm.run().expect("module creates parent").value;
    let receiver = object(&[("marker", Value::Number(5.0))]);
    let arrow = vm
        .call_value(&parent, receiver.clone(), Vec::new(), 0)
        .expect("parent creates arrow");
    let Value::Function(arrow_closure) = &arrow else {
        panic!("returned value is an arrow");
    };
    assert!(arrow_closure.captured_arguments.is_none());
    assert_eq!(arrow_closure.captured_this.as_ref(), Some(&receiver));
    assert_eq!(
        vm.call_value(&arrow, Value::Undefined, Vec::new(), 0),
        Ok(receiver),
    );

    let new_target_program = program_with_functions(
        Vec::new(),
        vec![
            Instruction::MakeFunction(0),
            Instruction::New { argc: 0 },
            Instruction::Return,
        ],
        0,
        vec![
            proto(
                "constructor",
                vec![Instruction::MakeFunction(1), Instruction::Return],
                0,
                0,
                false,
                false,
            ),
            proto(
                "newTargetArrow",
                vec![Instruction::LoadNewTarget, Instruction::Return],
                0,
                0,
                true,
                false,
            ),
        ],
    );
    let value = run(&new_target_program).expect("constructor returns its arrow");
    let Value::Function(arrow) = value else {
        panic!("constructor returns the arrow object");
    };
    assert!(arrow.captured_arguments.is_none());
    assert!(matches!(
        arrow.captured_new_target,
        Some(Value::Function(_))
    ));
}

#[test]
fn ordinary_child_arguments_are_its_call_arguments_not_the_parent_lexical_arguments() {
    let program = program_with_functions(
        vec![Constant::String("length".into())],
        vec![
            Instruction::MakeFunction(0),
            Instruction::Const(1),
            Instruction::Const(2),
            Instruction::Const(3),
            Instruction::Call { argc: 3 },
            Instruction::Return,
        ],
        0,
        vec![
            proto(
                "parent",
                vec![
                    Instruction::MakeFunction(1),
                    Instruction::Const(4),
                    Instruction::Call { argc: 1 },
                    Instruction::Return,
                ],
                0,
                0,
                false,
                false,
            ),
            length_reader(false),
        ],
    );
    // The outer call has three actuals, while the parent passes exactly one to its ordinary
    // child. The child's own arguments object must win over the enclosing lexical call.
    let program = with_constants(
        program,
        vec![
            Constant::String("length".into()),
            number(1.0),
            number(2.0),
            number(3.0),
            number(4.0),
        ],
    );
    assert_eq!(run(&program), Ok(Value::Number(1.0)));
}

#[test]
fn argument_facts_are_scoped_to_the_exact_program_in_a_reused_realm() {
    let length_program = program_with_functions(
        vec![Constant::String("length".into())],
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![length_reader(false)],
    );
    let object_program = program_with_functions(
        Vec::new(),
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![proto(
            "objectReader",
            vec![Instruction::LoadArguments, Instruction::Return],
            0,
            0,
            false,
            false,
        )],
    );
    let mut runtime = realm_runtime().expect("realm runtime");
    let mut host = host();
    let length_closure = execute_in_runtime_with_strictness(
        Rc::new(length_program.clone()),
        Rc::new(Strictness::sloppy(&length_program)),
        &mut host,
        ExecutionLimits::default(),
        &mut runtime,
        ExecutionCapabilities::default(),
    )
    .expect("first module creates closure")
    .value;
    let object_closure = execute_in_runtime_with_strictness(
        Rc::new(object_program.clone()),
        Rc::new(Strictness::sloppy(&object_program)),
        &mut host,
        ExecutionLimits::default(),
        &mut runtime,
        ExecutionCapabilities::default(),
    )
    .expect("second module creates closure")
    .value;

    let (Value::Function(length_closure), Value::Function(object_closure)) =
        (&length_closure, &object_closure)
    else {
        panic!("both modules return closures");
    };
    assert!(!Rc::ptr_eq(
        &length_closure.program,
        &object_closure.program
    ));
    assert!(length_closure.arguments_length_only);
    assert!(!object_closure.arguments_length_only);
}

#[test]
fn direct_eval_keeps_arguments_visible_after_a_length_read() {
    fn eval_arguments_length(
        _request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        Ok((
            program_with_functions(
                vec![Constant::String("length".into())],
                vec![
                    Instruction::LoadArguments,
                    Instruction::GetProperty(0),
                    Instruction::Return,
                ],
                0,
                Vec::new(),
            ),
            Strictness::default(),
        ))
    }

    let site = EvalSite {
        module_id: "arguments-proof".to_owned(),
        source_offset: 0,
        bindings: Vec::new(),
        with_objects: Vec::new(),
        strict: false,
        global_var_scope: false,
        argc: Some(1),
        home: None,
        new_target: EvalNewTarget::Caller,
        private_scopes: Vec::new(),
        blocked_var_names: Vec::new(),
    };
    let function = proto(
        "evalReader",
        vec![
            Instruction::LoadArguments,
            Instruction::GetProperty(0),
            Instruction::Pop,
            Instruction::LoadBuiltin(Builtin::Eval),
            Instruction::Const(1),
            Instruction::DirectEval(Box::new(site)),
            Instruction::Return,
        ],
        0,
        0,
        false,
        false,
    );
    let program = program_with_functions(
        vec![
            Constant::String("length".into()),
            Constant::String("arguments.length".into()),
        ],
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![function],
    );
    let mut host = host();
    let mut vm = Vm::with_entropy_seed(
        Rc::new(program.clone()),
        Rc::new(Strictness::sloppy(&program)),
        &mut host,
        Vec::new(),
        ExecutionLimits::default(),
        &RealmCapabilities::default(),
        ExecutionCapabilities::default().with_source_compiler(eval_arguments_length),
    )
    .expect("realm capabilities resolve");
    let function = vm.run().expect("module creates function").value;
    let Value::Function(closure) = &function else {
        panic!("MakeFunction returns a closure");
    };
    assert!(
        !closure.arguments_length_only,
        "direct eval keeps the complete arguments object available to dynamic code",
    );
    assert_eq!(
        vm.call_value(
            &function,
            Value::Undefined,
            vec![Value::Number(1.0), Value::Number(2.0), Value::Number(3.0)],
            0,
        ),
        Ok(Value::Number(3.0)),
        "direct eval sees the same call arguments as the surrounding function",
    );
}

#[test]
fn paired_length_read_obeys_step_limit_for_both_instructions() {
    let program = program_with_functions(
        vec![Constant::String("length".into())],
        vec![
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        0,
        vec![length_reader(false)],
    );
    let mut host = host();
    let error = execute(
        Rc::new(program),
        &mut host,
        ExecutionLimits::default().with_max_steps(3),
        ExecutionCapabilities::default(),
    )
    .expect_err("the limit expires between LoadArguments and GetProperty");
    assert_eq!(
        error,
        ExecutionError::StepLimitExceeded { steps: 3, limit: 3 }
    );
}

#[test]
fn pending_length_read_survives_a_bounded_run_between_the_pair() {
    let program = program_with_functions(
        vec![Constant::String("length".into()), number(4.0)],
        vec![
            Instruction::MakeFunction(0),
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Call { argc: 2 },
            Instruction::Return,
        ],
        0,
        vec![length_reader(false)],
    );
    let mut host = host();
    let mut vm = test_vm(&program, &mut host);
    // Stop at callee PC 1, after LoadArguments ran and before its adjacent GetProperty.
    let partial = vm
        .run_bounded(usize::MAX, 1, 1)
        .expect("bounded run pauses between the two instructions");
    assert_eq!(partial.steps, 5);
    let result = vm.run().expect("resumption completes the call");
    assert_eq!(result.value, Value::Number(2.0));
    assert_eq!(result.steps, 8);
}

#[test]
fn get_property_is_not_rewritten_when_a_branch_enters_from_another_predecessor() {
    let function = proto(
        "branchedRead",
        vec![
            Instruction::LoadLocal(0),
            Instruction::JumpIfFalse(10),
            Instruction::LoadArguments,
            Instruction::GetProperty(0),
            Instruction::Pop,
            Instruction::LoadLocal(1),
            Instruction::Jump(8),
            Instruction::LoadArguments,
            Instruction::GetProperty(0),
            Instruction::Return,
            Instruction::LoadLocal(2),
            Instruction::Jump(8),
        ],
        3,
        3,
        false,
        false,
    );
    let program = program_with_functions(
        vec![Constant::String("length".into())],
        vec![Instruction::MakeFunction(0), Instruction::Return],
        0,
        vec![function],
    );
    let mut host = host();
    let (mut vm, function) = root_function(&program, &mut host);
    let supplied = object(&[("length", Value::Number(77.0))]);
    assert_eq!(
        vm.call_value(
            &function,
            Value::Undefined,
            vec![
                Value::Boolean(false),
                object(&[("length", Value::Number(55.0))]),
                supplied.clone(),
            ],
            0,
        ),
        Ok(Value::Number(77.0)),
        "the branch reaches GetProperty with an ordinary object, not arguments",
    );
    assert_eq!(
        vm.call_value(
            &function,
            Value::Undefined,
            vec![
                Value::Boolean(true),
                object(&[("length", Value::Number(55.0))]),
                supplied,
            ],
            0,
        ),
        Ok(Value::Number(55.0)),
        "the shared property-read instruction still reads the supplied object after the pair",
    );
}

fn run_vm(program: &Program, host: &mut TestHost) -> Value {
    test_vm(program, host)
        .run()
        .expect("program completes")
        .value
}

fn with_constants(mut program: Program, constants: Vec<Constant>) -> Program {
    program.constants = constants;
    program
}
