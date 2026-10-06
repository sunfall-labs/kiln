use super::*;

/// Every native the machine must dispatch has a route to it.
///
/// `call_native` is a free function with no VM, so a built-in that can run program code --
/// a comparator, a getter, an options bag, a set-like's `has` -- gets an `unreachable!` arm
/// there and a route in `Vm::dispatch_native`. The `unreachable!` is written to make a lost
/// route loud, and it does the opposite: nothing routes to the arm, so nothing reaches the
/// assertion, and the built-in aborts the process the first time a program calls it.
///
/// That has now happened to `Set.prototype.union`, the copying array methods, every
/// `RegExp.prototype` accessor, the date methods, the regexp symbol methods, `Iterator.from`,
/// `Object.groupBy`, `Map.groupBy` and `Promise.try` -- each found by a panicking test rather
/// than by the guard that exists for it. The pairing is checked here instead of trusted.
#[test]
fn every_machine_dispatched_native_has_a_route() {
    const MARKER: &str = "dispatched by the interpreter";
    let source = include_str!("../intrinsics/dispatch.rs");
    let routes = include_str!("../vm/intrinsics.rs");

    let mut missing = Vec::new();
    let mut cursor = 0;
    while let Some(found) = source[cursor..].find(MARKER) {
        let at = cursor + found;
        cursor = at + MARKER.len();
        // The arm's patterns sit between the previous arm and this one's `=>`.
        let head = &source[..at];
        let Some(arrow) = head.rfind("=>") else {
            continue;
        };
        let arm_start = head[..arrow]
            .rfind("\n        ")
            .map_or(0, |index| index + 1);
        for pattern in head[arm_start..arrow].split("Native::").skip(1) {
            let name: String = pattern
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            if !routes.contains(&format!("Native::{name}")) {
                missing.push(name);
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "these natives assert that the machine dispatches them, but `dispatch_native` has no \
             route to them, so calling one aborts the process: {missing:?}"
    );
}

/// A direct eval writes the caller's binding, not a copy of it.
///
/// This is the whole reason the site hands over cells: the eval'd program is compiled against
/// slot numbers of its own, and only a shared cell can make `eval("x = 2")` visible to the
/// caller afterwards. A copy would leave the caller reading 1 and look entirely plausible.
#[test]
fn a_direct_eval_writes_through_to_the_callers_binding() {
    let mut program = program(
        vec![
            Constant::Number(1.0f64.to_bits()),
            Constant::String("x = 2; x".to_owned().into()),
        ],
        vec![
            Instruction::NewCell(0),
            Instruction::Const(0),
            Instruction::StoreCell(0),
            Instruction::LoadBuiltin(kiln_ir::Builtin::Eval),
            Instruction::Const(1),
            Instruction::DirectEval(Box::new(EvalSite {
                module_id: "<language-test>".to_owned(),
                source_offset: 0,
                bindings: vec![kiln_ir::EvalBinding {
                    name: "x".to_owned(),
                    access: EvalAccess::Cell(0),
                    declaration: kiln_ir::EvalDeclaration::Var,
                }],
                with_objects: Vec::new(),
                strict: false,
                global_var_scope: false,
                argc: Some(1),
                home: None,
                new_target: kiln_ir::EvalNewTarget::Forbidden,
                private_scopes: Vec::new(),
                blocked_var_names: Vec::new(),
            })),
            // The eval's completion value is discarded; the caller's binding is the answer.
            Instruction::Pop,
            Instruction::LoadCell(0),
            Instruction::Return,
        ],
    );
    program.local_count = 1;
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut runtime = realm_runtime().expect("realm runtime");
    let result = execute_in_runtime_with_strictness(
        Rc::new(program.clone()),
        Rc::new(Strictness::sloppy(&program)),
        &mut host,
        ExecutionLimits::default(),
        &mut runtime,
        ExecutionCapabilities::default().with_source_compiler(stub_eval_compiler),
    )
    .map(|execution| execution.value);
    assert_eq!(result, Ok(Value::Number(2.0)));
}

#[test]
fn source_compiler_authority_is_scoped_to_one_execution() {
    fn constant_compiler(
        _request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        Ok((
            program(
                vec![Constant::Number(42.0f64.to_bits())],
                vec![Instruction::Const(0), Instruction::Return],
            ),
            Strictness::default(),
        ))
    }

    let invoking_eval = program(
        vec![Constant::String("ignored".to_owned().into())],
        vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Eval),
            Instruction::Const(0),
            Instruction::Call { argc: 1 },
            Instruction::Return,
        ],
    );
    let run_with = |capabilities| {
        let mut host = TestHost {
            calls: Vec::new(),
            clock_now: None,
            entropy: std::collections::VecDeque::new(),
            timezone: None,
            locale: None,
        };
        execute(
            Rc::new(invoking_eval.clone()),
            &mut host,
            ExecutionLimits::default(),
            capabilities,
        )
        .map(|result| result.value)
    };

    assert_eq!(
        run_with(ExecutionCapabilities::default().with_source_compiler(constant_compiler)),
        Ok(Value::Number(42.0))
    );
    assert!(matches!(
        run_with(ExecutionCapabilities::default()),
        Err(ExecutionError::NoSourceCompiler)
    ));
}

#[test]
fn dynamic_source_boundary_preserves_raw_units_and_keeps_parser_errors_distinct() {
    fn constant_compiler(
        request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        Ok((
            Program {
                source: "".into(),
                template_sites: Vec::new(),
                debug_call_sites: Vec::new(),
                version: kiln_ir::BYTECODE_VERSION,
                source_origin: None,
                static_source_identity: None,
                static_source_provenance: None,
                constants: vec![Constant::Number(7.0f64.to_bits())],
                code: vec![Instruction::Const(0), Instruction::Return],
                local_count: request.outer.len() as u32,
                functions: Vec::new(),
            },
            Strictness::default(),
        ))
    }

    fn syntax_compiler(
        _request: &EvalSource<'_>,
    ) -> Result<(Program, Strictness), EvalCompileError> {
        Err(EvalCompileError::Syntax("ordinary parser error".to_owned()))
    }

    let run_eval = |source: StringUnits, compiler: SourceCompiler| {
        let program = Program {
            source: "".into(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::String(source)],
            code: vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::Eval),
                Instruction::Const(0),
                Instruction::Call { argc: 1 },
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
        };
        let mut host = TestHost {
            calls: Vec::new(),
            clock_now: None,
            entropy: std::collections::VecDeque::new(),
            timezone: None,
            locale: None,
        };
        execute(
            Rc::new(program),
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default().with_source_compiler(compiler),
        )
        .map(|result| result.value)
    };

    let escaped = StringUnits::from(r#""\ud800""#);
    assert_eq!(run_eval(escaped, constant_compiler), Ok(Value::Number(7.0)));

    let raw = StringUnits::from_units(&[b'"' as u16, 0xd800, b'"' as u16]);
    assert_eq!(run_eval(raw, constant_compiler), Ok(Value::Number(7.0)));
    let program = no_program();
    let mut syntax_host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut syntax_vm = test_vm(&program, &mut syntax_host);
    syntax_vm.source_compiler = Some(syntax_compiler);
    let syntax = syntax_vm.enter_eval_units(&StringUnits::from(")"), None, false);
    assert!(matches!(
        syntax,
        Err(ExecutionError::SyntaxErrorMessage { message })
            if message == "ordinary parser error"
    ));

    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let program = no_program();
    let mut vm = test_vm(&program, &mut host);
    vm.source_compiler = Some(constant_compiler);
    let raw_body = Value::String(StringUnits::from_units(&[0xd800]));
    assert_eq!(
        vm.dispatch_native(
            Native::FunctionConstructor,
            &Value::Undefined,
            &[raw_body],
            0
        ),
        Ok(Value::Number(7.0))
    );
}

/// A directive prologue decides an eval's strictness, and an escaped spelling is not one.
#[test]
fn a_use_strict_directive_is_recognised_only_when_it_is_written_literally() {
    assert!(source_is_strict("\"use strict\"; x"));
    assert!(source_is_strict("  // a comment\n /* b */ 'use strict'"));
    assert!(source_is_strict("\"other\"; \"use strict\";"));
    assert!(!source_is_strict("x; \"use strict\";"));
    // `"use\u0020strict"` spells the same characters and is deliberately not the directive.
    assert!(!source_is_strict("\"use\\u0020strict\";"));
    assert!(!source_is_strict(""));
}

#[test]
fn a_captured_dead_zone_read_names_its_source_binding() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::String("later".to_owned().into())],
        code: vec![
            Instruction::NewCell(0),
            Instruction::MarkUninitialized(0),
            Instruction::MakeFunction(0),
            Instruction::Call { argc: 0 },
            Instruction::Return,
        ],
        local_count: 1,
        functions: vec![kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("readTooEarly".to_owned()),
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
            upvalues: vec![Upvalue::NamedParentLocal { slot: 0, name: 0 }],
            type_annotations: None,
        }],
    };

    assert!(matches!(
        run(&program),
        Err(ExecutionError::Uncaught { message, .. })
            if message == "ReferenceError: cannot access binding `later` before its declaration"
    ));
}

#[test]
fn loose_equality_is_not_strict_equality_with_a_conversion() {
    // The three rules that are not conversions at all.
    assert!(loose_equal(&Value::Null, &Value::Undefined));
    assert!(!loose_equal(&Value::Null, &Value::Number(0.0)));
    assert!(!loose_equal(
        &Value::Number(f64::NAN),
        &Value::Number(f64::NAN)
    ));
    // A boolean becomes a number first, so `[] == false` goes by way of "" and 0.
    assert!(loose_equal(&array(&[]), &Value::Boolean(false)));
    assert!(loose_equal(
        &Value::Number(1.0),
        &Value::String("1".to_owned().into())
    ));
    assert!(loose_equal(
        &array(&[Value::Number(1.0)]),
        &Value::Number(1.0)
    ));
    // Two objects compare by identity even when their contents match.
    assert!(!loose_equal(&object(&[]), &object(&[])));
    let shared = object(&[]);
    assert!(loose_equal(&shared, &shared.clone()));
}

#[test]
fn return_signal_rejected_promise_does_not_surface_as_unhandled_rejection() {
    let program = Program {
        source: "".into(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
        version: kiln_ir::BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: Vec::new(),
        local_count: 0,
        functions: Vec::new(),
    };
    let mut host = TestHost {
        calls: Vec::new(),
        clock_now: None,
        entropy: std::collections::VecDeque::new(),
        timezone: None,
        locale: None,
    };
    let mut vm = test_vm(&program, &mut host);
    let promise = PromiseState::pending();
    let settler = Value::Settler(Rc::clone(&promise), true);
    assert!(
        vm.call_value(
            &settler,
            Value::Undefined,
            vec![Value::Native(Native::ReturnSignal, RealmId::PRIMARY)],
            0
        )
        .is_ok()
    );
    assert!(matches!(vm.process_rejections(), Ok(false)));
}

/// Every `"@@..."` literal in this crate is a key `is_reserved` recognises.
///
/// `is_reserved` used to be the prefix test `key.starts_with("@@")`, which is a guess about what a
/// program would never write -- and it is wrong. `symbol-observable`, which redux and rxjs depend
/// on, stores its hook under the literal string `"@@observable"`, and that property vanished from
/// `Object.keys` and `getOwnPropertyNames` while staying readable by name.
///
/// The replacement is an exact list, and an exact list drifts. This reads the crate's own source
/// and fails if a slot was added without being listed, because the failure it prevents is the
/// opposite one: an internal slot that starts appearing in a program's `Object.keys`.
#[test]
fn every_internal_slot_is_reserved() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut unlisted = Vec::new();
    let mut checked = 0usize;
    let mut sources = Vec::new();
    let mut directories = vec![root];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(&directory).expect("read a source directory") {
            let path = entry.expect("read a source entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "tests") {
                    directories.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && path.file_name().is_none_or(|name| name != "tests.rs")
            {
                sources.push(path);
            }
        }
    }
    for path in sources {
        let text = std::fs::read_to_string(&path).expect("read a source file");
        for line in text.lines() {
            // Prose, not code. The documentation for this very rule quotes `"@@observable"` as
            // the key that must *not* be reserved, so a scan that reads comments would fail on
            // the explanation of why it exists.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for (offset, _) in line.match_indices("\"@@") {
                let rest = &line[offset + 1..];
                let Some(end) = rest.find('"') else { continue };
                let key = &rest[..end];
                // `RESERVED_KEY_PREFIX` itself: the prefix, not a key written under it.
                if key == "@@" {
                    continue;
                }
                // A key built by `format!` appears here as its prefix -- `@@symbol:` and
                // `@@private:` are both in `INTERNAL_PREFIXES`, so they answer true as written.
                checked += 1;
                if !kiln_interpreter_is_reserved(key) {
                    unlisted.push(format!("{} in {}", key, path.display()));
                }
            }
        }
    }
    assert!(
        checked > 40,
        "expected to find the crate's internal keys, found {checked}"
    );
    unlisted.sort();
    unlisted.dedup();
    assert!(
        unlisted.is_empty(),
        "these internal keys are not recognised by `symbol::is_reserved`, so they would appear \
         in a program's `Object.keys`: {unlisted:#?}"
    );
}

/// A built-in never reports flags for a member it does not have.
///
/// The failure this guards against does not look like a missing property; it looks like an extra
/// one. Attributes are asked before the value is, so a rule that answers from the *name* invents
/// the property outright: generalising one such rule from `Date` to every prototype gave every
/// prototype in the realm a phantom own `@@toPrimitive`, and the tests that noticed were in
/// `Date`, `Object` and `language/expressions` rather than anywhere near the change.
///
/// Swept across every table and every well-known symbol, plus names no built-in publishes.
#[test]
fn attributes_are_never_reported_for_an_absent_member() {
    let absent = [
        "definitelyNotAMember",
        "constructor2",
        "__nope__",
        "species",
        "iterator",
    ];
    for table in every_prototype_table() {
        let target = Value::Native(Native::PrototypeOf(table), RealmId::PRIMARY);
        let symbols = crate::symbol::WellKnown::ALL
            .into_iter()
            .map(|symbol| symbol.property_key());
        for key in symbols.chain(absent.iter().copied()) {
            let published = !matches!(
                Native::PrototypeOf(table).property(key),
                None | Some(Value::Undefined)
            ) || intrinsic_accessor(&target, key).is_some()
                || machine_accessor(&target, key).is_some();
            let described = intrinsic_own_attributes(&target, key).is_some();
            assert_eq!(
                described, published,
                "{table:?} describes {key} as {described} but publishes it as {published}"
            );
        }
    }
}

#[test]
fn internal_key_classification_skips_ordinary_names_without_widening_reserved_keys() {
    use crate::symbol::is_internal_key;

    for key in [
        "map",
        "length",
        "value",
        "@@observable",
        "@@symbol-observable",
    ] {
        assert!(!is_internal_key(key), "{key} is a program-visible key");
    }

    for key in [
        "@@symbol:1",
        "@@private:scope:1",
        "@@kiln:host-slot",
        "@@node:timer-handle",
    ] {
        assert!(is_internal_key(key), "{key} belongs to an internal family");
    }

    for key in ["@symbol:1", "@@symbol", "@@private", "@@kiln", "@@node"] {
        assert!(!is_internal_key(key), "{key} is not a complete family key");
    }
}
