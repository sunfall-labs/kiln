use super::*;
use kiln_hir::*;

fn module(body: Vec<Statement>) -> Module {
    Module {
        // These modules are built as HIR rather than parsed, so there is no text their spans
        // point into. A function compiled from one answers `toString` with the empty string,
        // which is what "no source" means here.
        source: "".into(),
        strict: false,
        source_kind: kiln_hir::SourceKind::JavaScript,
        // Script, because these fixtures are bodies rather than ESM: the goal now decides
        // strictness, and calling them modules would compile every one of them strict and
        // quietly stop the sloppy cases below from being sloppy cases.
        source_goal: kiln_hir::SourceGoal::Script,
        items: Vec::new(),
        body,
        requested_modules: Vec::new(),
        imports: Vec::new(),
        exports: Vec::new(),
        diagnostics: Vec::new(),
        panicked: false,
    }
}

fn number(value: f64) -> Expression {
    Expression {
        kind: ExpressionKind::Literal(Literal::Number(value.to_bits())),
        span: Span::default(),
    }
}

fn identifier(name: &str) -> Expression {
    Expression {
        kind: ExpressionKind::Identifier(name.into()),
        span: Span::default(),
    }
}

fn statement(kind: StatementKind) -> Statement {
    Statement {
        kind,
        span: Span::default(),
    }
}

fn function(name: &str, parameters: &[&str], body: Vec<Statement>) -> Statement {
    statement(StatementKind::Function(Box::new(kiln_hir::Function {
        strict: false,
        binds_own_name: false,
        asynchronous: false,
        name: Some(name.to_owned()),
        arrow: false,
        generator: false,
        parameters: parameters
            .iter()
            .map(|name| kiln_hir::Parameter {
                pattern: kiln_hir::Pattern::Identifier((*name).to_owned()),
                type_annotation: None,
                span: Span::default(),
            })
            .collect(),
        rest: None,
        return_type: None,
        body,
        span: Span::default(),
    })))
}

fn call(callee: Expression, arguments: Vec<Expression>) -> Expression {
    Expression {
        kind: ExpressionKind::Call {
            callee: Box::new(callee),
            arguments: arguments.into_iter().map(ListElement::Item).collect(),
            optional: false,
        },
        span: Span::default(),
    }
}

fn declaration(kind: VariableKind, name: &str, initializer: Option<Expression>) -> Statement {
    statement(StatementKind::Variable {
        kind,
        declarations: vec![kiln_hir::VariableDeclarator {
            pattern: Pattern::Identifier(name.to_owned()),
            initializer,
            span: Span::default(),
        }],
    })
}

fn string(text: &str) -> Expression {
    Expression {
        kind: ExpressionKind::Literal(Literal::String(text.to_owned())),
        span: Span::default(),
    }
}

/// A direct `eval` hands its caller's bindings over as cells, not as slot numbers.
///
/// The eval'd source is compiled separately, so a slot number would mean nothing to it. A
/// cell is the one storage two independently compiled programs can share -- and sharing is
/// the requirement, because `eval("x = 2")` has to be visible to the caller afterwards.
#[test]
fn a_direct_eval_carries_the_bindings_around_it() {
    // function f() { var x = 1; eval("x"); }
    let program = compile(&module(vec![function(
        "f",
        &[],
        vec![
            declaration(VariableKind::Var, "x", Some(number(1.0))),
            statement(StatementKind::Expression(call(
                identifier("eval"),
                vec![string("x")],
            ))),
        ],
    )]))
    .expect("the module compiles");
    let body = &program.functions[0];
    let site = body
        .code
        .iter()
        .find_map(|instruction| match instruction {
            Instruction::DirectEval(site) => Some(site),
            _ => None,
        })
        .expect("the call lowered to a direct eval");
    let binding = site
        .bindings
        .iter()
        .find(|binding| binding.name == "x")
        .expect("`x` is in scope at the call");
    assert!(
        matches!(binding.access, EvalAccess::Cell(_)),
        "the caller's binding travels as a cell: {:?}",
        binding.access
    );
    assert!(
        body.code.contains(&Instruction::NewCell(0)),
        "and the function boxed it, which is what makes the cell exist"
    );
}

/// The caller's names occupy the eval program's first slots, in the order they were handed
/// over. That ordering *is* the contract between the compiler and the machine that installs
/// the cells; nothing else records which slot a name landed in.
#[test]
fn eval_code_binds_the_callers_names_to_its_first_slots() {
    let program = compile_eval(
        &module(vec![statement(StatementKind::Expression(identifier("x")))]),
        &EvalOptions {
            outer: vec![EvalOuter {
                name: "x".to_owned(),
                declaration: EvalDeclaration::Var,
            }],
            strict: false,
            global_var_scope: false,
            script: false,
            ..EvalOptions::default()
        },
    )
    .expect("the eval source compiles");
    assert!(
        program.code.contains(&Instruction::LoadCell(0)),
        "`x` reads the cell installed in slot 0: {:?}",
        program.code
    );
    assert!(
        matches!(program.code.last(), Some(Instruction::Return)),
        "and the program ends by returning its completion value"
    );
}

/// Strictness is inherited into every nested body, and a class body raises it.
///
/// The store instructions carry no strictness of their own, so the interpreter reads it off
/// the body a write was written in. That table is only right if the compiler propagates the
/// flag the way the language does: outward-in for a `"use strict"` prologue, and
/// unconditionally through a class -- whose methods are strict in the sloppiest of files.
#[test]
fn strictness_reaches_every_body_written_inside_a_strict_one() {
    let mut inner = kiln_hir::Function {
        name: Some("inner".to_owned()),
        binds_own_name: true,
        arrow: false,
        generator: false,
        asynchronous: false,
        parameters: Vec::new(),
        rest: None,
        return_type: None,
        body: Vec::new(),
        strict: false,
        span: Span::default(),
    };
    let mut outer = inner.clone();
    outer.name = Some("outer".to_owned());
    outer.strict = true;
    outer.body = vec![statement(StatementKind::Function(Box::new(inner.clone())))];

    let (_, strictness) = compile_with_strictness(
        &module(vec![statement(StatementKind::Function(Box::new(outer)))]),
        CompileOptions::default(),
    )
    .expect("the program compiles");
    assert_eq!(
        strictness.functions,
        vec![true, true],
        "the nested body is strict because the one around it said so"
    );
    assert!(!strictness.body, "and the file around them both is not");

    inner.name = Some("m".to_owned());
    let class = kiln_hir::Class {
        inferred_name: None,
        name: Some("C".to_owned()),
        superclass: None,
        constructor: None,
        methods: vec![kiln_hir::ClassMethod {
            key: ObjectKey::Static("m".to_owned()),
            function: inner,
            is_static: false,
            kind: kiln_hir::MethodKind::Normal,
        }],
        initialisers: Vec::new(),
        span: Span::default(),
    };
    let (_, strictness) = compile_with_strictness(
        &module(vec![statement(StatementKind::Class(Box::new(class)))]),
        CompileOptions::default(),
    )
    .expect("the class compiles");
    assert!(
        strictness.functions.iter().all(|strict| *strict),
        "a class body is strict code whatever encloses it: {:?}",
        strictness.functions
    );
}

#[test]
fn property_mutations_carry_their_local_strictness() {
    let assignment = || {
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::MemberAssignment {
                operator: AssignmentOperator::Assign,
                object: Box::new(Expression {
                    kind: ExpressionKind::Object {
                        members: Vec::new(),
                    },
                    span: Span::default(),
                }),
                property: "answer".into(),
                value: Box::new(number(42.0)),
            },
            span: Span::default(),
        }))
    };
    let mut strict_module = module(vec![assignment()]);
    strict_module.strict = true;
    let strict = compile(&strict_module).expect("strict module compiles");
    assert!(strict.code.iter().any(|instruction| matches!(
        instruction,
        Instruction::SetPropertyStrict { strict: true, .. }
    )));

    let sloppy = compile_commonjs(&module(vec![assignment()])).expect("CommonJS compiles");
    assert!(sloppy.program.code.iter().any(|instruction| matches!(
        instruction,
        Instruction::SetPropertyStrict { strict: false, .. }
    )));
}

#[test]
fn commonjs_var_may_redeclare_a_wrapper_parameter() {
    let program = compile_commonjs(&module(vec![declaration(
        VariableKind::Var,
        "module",
        Some(number(42.0)),
    )]))
    .expect("CommonJS wrapper parameters have function-parameter var semantics");

    assert!(program.program.code.contains(&Instruction::StoreCell(2)));
}

/// Strictness decides whose binding a `var` in eval code is.
///
/// A sloppy direct eval declares into the caller's variable environment, so `var x` over a
/// name the caller already has *is* that binding. Strict eval code gets a scope of its own,
/// so the same declaration shadows it -- and writing one must not reach the caller's.
#[test]
fn a_sloppy_eval_var_takes_the_callers_binding_and_a_strict_one_shadows_it() {
    let source = || module(vec![declaration(VariableKind::Var, "x", Some(number(2.0)))]);
    let outer = || {
        vec![EvalOuter {
            name: "x".to_owned(),
            declaration: EvalDeclaration::Var,
        }]
    };
    let sloppy = compile_eval(
        &source(),
        &EvalOptions {
            outer: outer(),
            strict: false,
            global_var_scope: false,
            script: false,
            ..EvalOptions::default()
        },
    )
    .expect("the sloppy eval compiles");
    assert!(
        sloppy.code.contains(&Instruction::StoreCell(0)),
        "the sloppy `var` writes the caller's cell: {:?}",
        sloppy.code
    );
    let strict = compile_eval(
        &source(),
        &EvalOptions {
            outer: outer(),
            strict: true,
            global_var_scope: false,
            script: false,
            ..EvalOptions::default()
        },
    )
    .expect("the strict eval compiles");
    assert!(
        !strict.code.contains(&Instruction::StoreCell(0)),
        "the strict `var` gets a slot of its own instead: {:?}",
        strict.code
    );
}

/// `EvalDeclarationInstantiation` refuses a `var` naming a binding in a record between the call's
/// lexical environment and its variable one. The only such record this compiler cannot see from
/// inside the eval program is a *parameter* environment -- parameters and body share one scope
/// here -- so the names travel on the call site, and this is the check that consumes them.
#[test]
fn a_blocked_name_refuses_a_sloppy_eval_var_and_a_strict_one_is_untouched() {
    let source = || module(vec![declaration(VariableKind::Var, "arguments", None)]);
    let blocked = || vec!["arguments".to_owned(), "p".to_owned()];
    let refused = compile_eval(
        &source(),
        &EvalOptions {
            blocked_var_names: blocked(),
            ..EvalOptions::default()
        },
    )
    .expect_err("a blocked `var` is an early error");
    assert!(
        refused[0].message.contains("arguments"),
        "the refusal names the binding: {refused:?}"
    );
    // Strict eval code has a variable environment of its own, so there is nothing between it and
    // its declarations and the rule does not apply -- which is why the blocked names are only
    // consulted for a sloppy source.
    compile_eval(
        &source(),
        &EvalOptions {
            blocked_var_names: blocked(),
            strict: true,
            ..EvalOptions::default()
        },
    )
    .expect("a strict eval declares into its own scope");
    // A name the record does not hold is an ordinary declaration.
    compile_eval(
        &module(vec![declaration(VariableKind::Var, "other", None)]),
        &EvalOptions {
            blocked_var_names: blocked(),
            ..EvalOptions::default()
        },
    )
    .expect("an unrelated `var` is unaffected");
}

fn errors(body: Vec<Statement>) -> Vec<String> {
    compile(&module(body))
        .expect_err("expected the module to be rejected")
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

/// An arrow function taking the given parameters.
fn arrow(parameters: &[&str], body: Vec<Statement>) -> Expression {
    Expression {
        kind: ExpressionKind::Function(Box::new(kiln_hir::Function {
            strict: false,
            binds_own_name: false,
            asynchronous: false,
            name: None,
            arrow: true,
            generator: false,
            parameters: parameters
                .iter()
                .map(|name| kiln_hir::Parameter {
                    pattern: kiln_hir::Pattern::Identifier((*name).to_owned()),
                    type_annotation: None,
                    span: Span::default(),
                })
                .collect(),
            rest: None,
            return_type: None,
            body,
            span: Span::default(),
        })),
        span: Span::default(),
    }
}

/// The capture analysis has to descend into a function *expression* nested in a `return`.
/// It did not at first -- it stopped as soon as it was not already collecting -- so
/// `function f(n) { return (m) => n + m; }` compiled with `n` unboxed and the arrow could
/// not resolve it. This is that program.
#[test]
fn an_arrow_returned_from_a_function_captures_its_parameter() {
    let program = compile(&module(vec![function(
        "make_adder",
        &["n"],
        vec![statement(StatementKind::Return(Some(arrow(
            &["m"],
            vec![statement(StatementKind::Return(Some(Expression {
                kind: ExpressionKind::Binary {
                    operator: kiln_hir::BinaryOperator::Add,
                    left: Box::new(identifier("n")),
                    right: Box::new(identifier("m")),
                },
                span: Span::default(),
            })))],
        ))))],
    )]))
    .expect("compiles");

    let outer = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("make_adder"))
        .expect("outer proto");
    assert!(
        outer
            .code
            .iter()
            .any(|i| matches!(i, Instruction::NewCell(_))),
        "the captured parameter must be boxed"
    );
    let inner = program
        .functions
        .iter()
        .find(|proto| proto.name.is_none())
        .expect("the arrow has no name");
    assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
}

/// A computed key is an arbitrary expression evaluated in source order, so it must be
/// compiled rather than folded into a constant.
#[test]
fn a_computed_object_key_is_evaluated_not_interned() {
    let program = compile(&module(vec![statement(StatementKind::Expression(
        Expression {
            kind: ExpressionKind::Object {
                members: vec![
                    kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                        key: kiln_hir::ObjectKey::Static("fixed".into()),
                        value: number(1.0),
                        form: kiln_hir::PropertyForm::KeyValue,
                        span: Span::default(),
                    }),
                    kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                        key: kiln_hir::ObjectKey::Computed(Expression {
                            kind: ExpressionKind::Binary {
                                operator: kiln_hir::BinaryOperator::Add,
                                left: Box::new(number(1.0)),
                                right: Box::new(number(2.0)),
                            },
                            span: Span::default(),
                        }),
                        value: number(3.0),
                        form: kiln_hir::PropertyForm::KeyValue,
                        span: Span::default(),
                    }),
                ],
            },
            span: Span::default(),
        },
    ))]))
    .expect("compiles");
    assert!(
        program
            .code
            .iter()
            .any(|i| matches!(i, Instruction::Binary(_))),
        "the computed key is evaluated, not folded to a constant"
    );
}

#[test]
fn a_compound_member_assignment_evaluates_its_receiver_once() {
    let program = compile(&module(vec![
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("o".into()),
                initializer: Some(Expression {
                    kind: ExpressionKind::Object {
                        members: Vec::new(),
                    },
                    span: Span::default(),
                }),
                span: Span::default(),
            }],
        }),
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::MemberAssignment {
                operator: AssignmentOperator::Add,
                object: Box::new(identifier("o")),
                property: "count".into(),
                value: Box::new(number(1.0)),
            },
            span: Span::default(),
        })),
    ]))
    .expect("compiles");

    let loads = program
        .code
        .iter()
        .filter(|i| matches!(i, Instruction::LoadLocal(_)))
        .count();
    assert_eq!(loads, 1, "the receiver is loaded once and duplicated");
    assert!(program.code.contains(&Instruction::Dup));
}

/// Property names repeat across a program, so they are interned rather than carried inline.
#[test]
fn repeated_property_names_share_one_constant() {
    let read = |name: &str| {
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::StaticMember {
                optional: false,
                object: Box::new(identifier("o")),
                property: name.into(),
            },
            span: Span::default(),
        }))
    };
    let program = compile(&module(vec![
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("o".into()),
                initializer: Some(Expression {
                    kind: ExpressionKind::Object {
                        members: Vec::new(),
                    },
                    span: Span::default(),
                }),
                span: Span::default(),
            }],
        }),
        read("length"),
        read("length"),
        read("length"),
    ]))
    .expect("compiles");

    let names = program
        .constants
        .iter()
        .filter(|constant| **constant == Constant::String("length".to_owned()))
        .count();
    assert_eq!(names, 1, "three reads of .length share one constant");
}

#[test]
fn a_captured_local_is_boxed_and_the_capture_points_at_its_slot() {
    let program = compile(&module(vec![function(
        "outer",
        &[],
        vec![
            statement(StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("captured".into()),
                    initializer: Some(number(0.0)),
                    span: Span::default(),
                }],
            }),
            function(
                "inner",
                &[],
                vec![statement(StatementKind::Return(Some(identifier(
                    "captured",
                ))))],
            ),
        ],
    )]))
    .expect("compiles");

    let outer = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("outer"))
        .expect("outer proto");
    assert!(
        outer
            .code
            .iter()
            .any(|i| matches!(i, Instruction::NewCell(_))),
        "the captured local must be boxed"
    );

    let inner = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("inner"))
        .expect("inner proto");
    assert_eq!(inner.upvalues.len(), 1);
    assert!(matches!(inner.upvalues[0], Upvalue::ParentLocal(_)));
    assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
}

/// A local nothing captures must stay an ordinary slot: the indirection is paid where the
/// semantics need it and nowhere else.
#[test]
fn an_uncaptured_local_is_not_boxed() {
    let program = compile(&module(vec![function(
        "plain",
        &[],
        vec![statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("local".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        })],
    )]))
    .expect("compiles");
    assert!(
        !program.functions[0]
            .code
            .iter()
            .any(|i| matches!(i, Instruction::NewCell(_))),
        "nothing captures this local, so it must not be boxed"
    );
}

/// Two levels of nesting is two hops: the middle function captures from its parent, and the
/// innermost captures from the middle. A single long hop would reach a frame that is gone.
#[test]
fn a_two_level_capture_threads_through_the_function_between() {
    let program = compile(&module(vec![function(
        "outer",
        &["x"],
        vec![function(
            "middle",
            &[],
            vec![function(
                "inner",
                &[],
                vec![statement(StatementKind::Return(Some(identifier("x"))))],
            )],
        )],
    )]))
    .expect("compiles");

    let middle = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("middle"))
        .expect("middle proto");
    let inner = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("inner"))
        .expect("inner proto");

    assert!(
        matches!(middle.upvalues.as_slice(), [Upvalue::ParentLocal(_)]),
        "middle captures outer's boxed parameter directly"
    );
    assert!(
        matches!(inner.upvalues.as_slice(), [Upvalue::ParentUpvalue(0)]),
        "inner captures what middle had already captured"
    );
}

/// Writing through a capture has to be a write to the shared cell, not to a frame slot.
#[test]
fn assigning_to_a_captured_name_writes_through_the_cell() {
    let program = compile(&module(vec![function(
        "outer",
        &["seed"],
        vec![function(
            "bump",
            &[],
            vec![statement(StatementKind::Expression(Expression {
                kind: ExpressionKind::Assignment {
                    operator: AssignmentOperator::Assign,
                    target: "seed".into(),
                    value: Box::new(number(1.0)),
                },
                span: Span::default(),
            }))],
        )],
    )]))
    .expect("compiles");
    let bump = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("bump"))
        .expect("bump proto");
    assert!(bump.code.contains(&Instruction::StoreUpvalue(0)));
}

#[test]
fn a_function_body_compiles_into_its_own_proto() {
    let program = compile(&module(vec![function(
        "second",
        &["a", "b"],
        vec![statement(StatementKind::Return(Some(identifier("b"))))],
    )]))
    .expect("compiles");

    assert_eq!(program.functions.len(), 1);
    let proto = &program.functions[0];
    assert_eq!(proto.parameter_count, 2);
    assert_eq!(proto.local_count, 2, "parameters occupy the first slots");
    assert_eq!(proto.code.first(), Some(&Instruction::LoadLocal(1)));
    // Every body ends in a Return, so a caller can never fall off the end of one.
    assert_eq!(proto.code.last(), Some(&Instruction::Return));
}

#[test]
fn a_captured_defaulted_identifier_parameter_uses_its_positional_cell() {
    let default = Expression {
        kind: ExpressionKind::StaticMember {
            object: Box::new(identifier("factory")),
            property: "fallback".to_owned(),
            optional: false,
        },
        span: Span::default(),
    };
    let function = statement(StatementKind::Function(Box::new(kiln_hir::Function {
        strict: false,
        binds_own_name: false,
        asynchronous: false,
        name: Some("factory".to_owned()),
        arrow: false,
        generator: false,
        parameters: vec![kiln_hir::Parameter {
            pattern: Pattern::WithDefault {
                inner: Box::new(Pattern::Identifier("value".to_owned())),
                default: Box::new(default),
            },
            type_annotation: None,
            span: Span::default(),
        }],
        rest: None,
        return_type: None,
        body: vec![statement(StatementKind::Return(Some(arrow(
            &[],
            vec![statement(StatementKind::Return(Some(identifier("value"))))],
        ))))],
        span: Span::default(),
    })));
    let program = compile(&module(vec![function])).expect("defaulted parameter compiles");
    let factory = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("factory"))
        .expect("factory proto");
    assert!(factory.code.contains(&Instruction::LoadCell(0)));
    assert!(factory.code.contains(&Instruction::StoreCell(0)));
    assert_eq!(
        factory
            .code
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::LoadGlobal(_)))
            .count(),
        0,
        "the function's module binding is captured, never read from the caller's module"
    );
    assert!(factory.code.contains(&Instruction::LoadUpvalue(0)));
}

/// The binding has to exist before the body is compiled, or a function cannot call itself.
#[test]
fn a_function_can_refer_to_itself() {
    let program = compile(&module(vec![function(
        "loop_forever",
        &[],
        vec![statement(StatementKind::Return(Some(Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(identifier("loop_forever")),
                arguments: Vec::new(),
                optional: false,
            },
            span: Span::default(),
        })))],
    )]))
    .expect("compiles");
    assert!(
        program.functions[0]
            .code
            .contains(&Instruction::Call { argc: 0 })
    );
}

/// A function reads a module binding through a captured cell, never through its own slots.
///
/// It used to read one through `LoadGlobal`, against the running module's frame. That is
/// right while the function stays home and wrong the moment it is exported: the frame found
/// belongs to the importer. A cell belongs to the binding, so it travels with the closure.
/// What must never appear is `LoadLocal`, which would read the callee frame's own slot.
#[test]
fn a_function_reads_module_scope_through_a_captured_cell() {
    let program = compile(&module(vec![
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("shared".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        }),
        function(
            "read",
            &[],
            vec![statement(StatementKind::Return(Some(identifier("shared"))))],
        ),
    ]))
    .expect("compiles");
    assert!(
        program.functions[0]
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadUpvalue(_))),
        "a module binding a function reads has to reach it through a cell"
    );
    assert!(
        !program.functions[0]
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadLocal(_))),
        "module reads must not compile to LoadLocal, which would read the frame's own slot"
    );
    // The cell is made at module scope, before anything can capture it.
    assert!(program.code.contains(&Instruction::NewCell(0)));
}

/// A function expression assigned to a prototype is still nested in the surrounding module.
///
/// This is the shape used by Express' router (`Layer.prototype.match = function match (...)`)
/// and it once lost the collector's `inside_function` state while walking the assignment.
/// The method consequently emitted `LoadGlobal`, which indexed the importer's module frame
/// when the function crossed the CommonJS boundary.
#[test]
fn a_prototype_assigned_function_captures_its_module_binding() {
    let assigned = Expression {
        kind: ExpressionKind::MemberAssignment {
            operator: AssignmentOperator::Assign,
            object: Box::new(identifier("prototype")),
            property: "match".to_owned(),
            value: Box::new(Expression {
                kind: ExpressionKind::Function(Box::new(kiln_hir::Function {
                    strict: false,
                    binds_own_name: true,
                    asynchronous: false,
                    name: Some("match".to_owned()),
                    arrow: false,
                    generator: false,
                    parameters: Vec::new(),
                    rest: None,
                    return_type: None,
                    body: vec![statement(StatementKind::Return(Some(identifier(
                        "module_constant",
                    ))))],
                    span: Span::default(),
                })),
                span: Span::default(),
            }),
        },
        span: Span::default(),
    };
    let program = compile(&module(vec![
        declaration(VariableKind::Const, "module_constant", Some(number(1.0))),
        declaration(
            VariableKind::Const,
            "prototype",
            Some(Expression {
                kind: ExpressionKind::Object {
                    members: Vec::new(),
                },
                span: Span::default(),
            }),
        ),
        statement(StatementKind::Expression(assigned)),
    ]))
    .expect("prototype assignment compiles");
    let method = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("match"))
        .expect("assigned function proto");
    assert!(method.code.contains(&Instruction::LoadUpvalue(0)));
    assert!(
        !method
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
        "an exported method must carry the module binding it reads: {:?}",
        method.code
    );
    assert!(program.code.contains(&Instruction::NewCell(0)));
}

#[test]
fn a_function_does_not_capture_a_local_it_declares_itself() {
    let program = compile(&module(vec![
        declaration(VariableKind::Const, "DEFAULT_DELIMITER", Some(string("/"))),
        function(
            "uses_default",
            &[],
            vec![
                declaration(
                    VariableKind::Const,
                    "delimiter",
                    Some(identifier("DEFAULT_DELIMITER")),
                ),
                statement(StatementKind::Return(Some(identifier("delimiter")))),
            ],
        ),
    ]))
    .expect("function with a local compiles");
    let body = &program.functions[0];
    assert!(
        body.code.contains(&Instruction::LoadUpvalue(0)),
        "the outer module binding is the capture: {:?}",
        body.code
    );
    assert!(
        !body.code.iter().any(|instruction| matches!(
            instruction,
            Instruction::LoadGlobal(_) | Instruction::StoreGlobal(_)
        )),
        "the function's own local must not force caller-relative module reads: {:?}",
        body.code
    );
}

#[test]
fn a_class_method_default_captures_its_module_binding() {
    let method = kiln_hir::Function {
        strict: false,
        binds_own_name: false,
        asynchronous: false,
        name: Some("read".to_owned()),
        arrow: false,
        generator: false,
        parameters: vec![kiln_hir::Parameter {
            pattern: Pattern::WithDefault {
                inner: Box::new(Pattern::Identifier("value".to_owned())),
                default: Box::new(identifier("shared")),
            },
            type_annotation: None,
            span: Span::default(),
        }],
        rest: None,
        return_type: None,
        body: vec![statement(StatementKind::Return(Some(identifier("value"))))],
        span: Span::default(),
    };
    let program = compile(&module(vec![
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: Pattern::Identifier("shared".to_owned()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        }),
        statement(StatementKind::Class(Box::new(kiln_hir::Class {
            inferred_name: None,
            name: Some("Reader".to_owned()),
            superclass: None,
            constructor: None,
            methods: vec![kiln_hir::ClassMethod {
                key: kiln_hir::ObjectKey::Static("read".to_owned()),
                function: method,
                is_static: false,
                kind: kiln_hir::MethodKind::Normal,
            }],
            initialisers: Vec::new(),
            span: Span::default(),
        }))),
    ]))
    .expect("class method compiles");
    let read = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("read"))
        .expect("method proto");
    assert!(read.code.contains(&Instruction::LoadUpvalue(0)));
    assert!(
        !read
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
        "an exported method must carry its module binding instead of indexing its caller"
    );
    assert!(program.code.contains(&Instruction::NewCell(0)));
}

#[test]
fn a_nested_class_resolves_the_private_name_declared_by_its_outer_class() {
    let read = kiln_hir::Function {
        strict: true,
        binds_own_name: false,
        asynchronous: false,
        name: Some("read".to_owned()),
        arrow: false,
        generator: false,
        parameters: Vec::new(),
        rest: None,
        return_type: None,
        body: vec![statement(StatementKind::Return(Some(Expression {
            kind: ExpressionKind::PrivateMember {
                object: Box::new(identifier("Outer")),
                name: "x".to_owned(),
                optional: false,
            },
            span: Span::default(),
        })))],
        span: Span::default(),
    };
    let inner = kiln_hir::Class {
        inferred_name: None,
        name: Some("Inner".to_owned()),
        superclass: None,
        constructor: None,
        methods: vec![kiln_hir::ClassMethod {
            key: ObjectKey::Static("read".to_owned()),
            function: read,
            is_static: true,
            kind: kiln_hir::MethodKind::Normal,
        }],
        initialisers: Vec::new(),
        span: Span::default(),
    };
    let make = kiln_hir::Function {
        strict: true,
        binds_own_name: false,
        asynchronous: false,
        name: Some("make".to_owned()),
        arrow: false,
        generator: false,
        parameters: Vec::new(),
        rest: None,
        return_type: None,
        body: vec![statement(StatementKind::Class(Box::new(inner)))],
        span: Span::default(),
    };
    let program = compile(&module(vec![statement(StatementKind::Class(Box::new(
        kiln_hir::Class {
            inferred_name: None,
            name: Some("Outer".to_owned()),
            superclass: None,
            constructor: None,
            methods: vec![kiln_hir::ClassMethod {
                key: ObjectKey::Static("make".to_owned()),
                function: make,
                is_static: true,
                kind: kiln_hir::MethodKind::Normal,
            }],
            initialisers: vec![ClassInitialiser::Field(ClassField {
                key: ObjectKey::Private("x".to_owned()),
                value: Some(number(1.0)),
                is_static: true,
            })],
            span: Span::default(),
        },
    )))]))
    .expect("nested class compiles");
    program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("read"))
        .expect("nested read method");
    assert!(
        program
            .constants
            .contains(&Constant::String("@@private:0:x".to_owned())),
        "the nested class reads the private key declared by Outer: {:?}",
        program.constants
    );
    assert!(
        !program
            .constants
            .contains(&Constant::String("@@private:1:x".to_owned())),
        "the nested class must not invent a private key of its own"
    );
}

#[test]
fn a_class_constructor_inside_a_wrapper_captures_the_wrapper_binding() {
    let constructor = kiln_hir::Function {
        strict: false,
        binds_own_name: false,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        parameters: vec![kiln_hir::Parameter {
            pattern: Pattern::Identifier("value".to_owned()),
            type_annotation: None,
            span: Span::default(),
        }],
        rest: None,
        return_type: None,
        body: vec![statement(StatementKind::Return(Some(Expression {
            kind: ExpressionKind::StaticMember {
                object: Box::new(identifier("exports")),
                property: "IDENTIFIER".to_owned(),
                optional: false,
            },
            span: Span::default(),
        })))],
        span: Span::default(),
    };
    let wrapper = kiln_hir::Function {
        strict: false,
        binds_own_name: false,
        asynchronous: false,
        name: Some("wrapper".to_owned()),
        arrow: false,
        generator: false,
        parameters: vec![kiln_hir::Parameter {
            pattern: Pattern::Identifier("exports".to_owned()),
            type_annotation: None,
            span: Span::default(),
        }],
        rest: None,
        return_type: None,
        body: vec![statement(StatementKind::Class(Box::new(kiln_hir::Class {
            inferred_name: None,
            name: Some("Name".to_owned()),
            superclass: None,
            constructor: Some(constructor),
            methods: Vec::new(),
            initialisers: Vec::new(),
            span: Span::default(),
        })))],
        span: Span::default(),
    };
    let program = compile(&module(vec![statement(StatementKind::Function(Box::new(
        wrapper,
    )))]))
    .expect("CommonJS-shaped wrapper compiles");
    let wrapper = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("wrapper"))
        .expect("wrapper proto");
    assert!(
        wrapper.code.contains(&Instruction::NewCell(0)),
        "the wrapper parameter must be boxed for the nested class constructor"
    );
    let constructor = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("Name"))
        .expect("class constructor proto");
    assert!(constructor.code.contains(&Instruction::LoadUpvalue(0)));
    assert!(
        !constructor
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
        "the constructor must carry the wrapper binding across module calls"
    );
}

#[test]
fn an_exported_derived_constructor_captures_its_hidden_super_binding() {
    let program = compile(&module(vec![
        statement(StatementKind::Class(Box::new(kiln_hir::Class {
            inferred_name: None,
            name: Some("Base".to_owned()),
            superclass: None,
            constructor: None,
            methods: Vec::new(),
            initialisers: Vec::new(),
            span: Span::default(),
        }))),
        statement(StatementKind::Class(Box::new(kiln_hir::Class {
            inferred_name: None,
            name: Some("Derived".to_owned()),
            superclass: Some(identifier("Base")),
            constructor: Some(kiln_hir::Function {
                strict: false,
                binds_own_name: false,
                asynchronous: false,
                name: None,
                arrow: false,
                generator: false,
                parameters: Vec::new(),
                rest: None,
                return_type: None,
                body: vec![statement(StatementKind::Expression(Expression {
                    kind: ExpressionKind::SuperCall {
                        arguments: Vec::new(),
                    },
                    span: Span::default(),
                }))],
                span: Span::default(),
            }),
            methods: Vec::new(),
            initialisers: Vec::new(),
            span: Span::default(),
        }))),
    ]))
    .expect("derived class compiles");
    let derived = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("Derived"))
        .expect("derived constructor proto");
    assert!(derived.code.contains(&Instruction::LoadUpvalue(0)));
    assert!(
        !derived
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
        "super must travel with the exported constructor instead of indexing its caller"
    );
}

/// Step 1a refused captures outright; 1b compiles them. What must remain true is that a
/// capture never becomes a `LoadLocal` on the enclosing frame's slot -- that would read
/// whatever the callee's own frame happened to hold there, which is a plausible wrong
/// answer rather than a diagnostic.
#[test]
fn a_capture_never_compiles_to_a_frame_local_read() {
    let program = compile(&module(vec![function(
        "outer",
        &["x"],
        vec![function(
            "inner",
            &[],
            vec![statement(StatementKind::Return(Some(identifier("x"))))],
        )],
    )]))
    .expect("compiles");
    let inner = program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("inner"))
        .expect("inner proto");
    assert!(
        !inner
            .code
            .iter()
            .any(|i| matches!(i, Instruction::LoadLocal(_))),
        "inner has no locals of its own; reading x must go through an upvalue"
    );
    assert!(inner.code.contains(&Instruction::LoadUpvalue(0)));
}

#[test]
fn return_outside_a_function_is_refused() {
    let messages = errors(vec![statement(StatementKind::Return(None))]);
    assert!(
        messages.iter().any(|m| m.contains("outside of a function")),
        "got: {messages:?}"
    );
}

#[test]
fn duplicate_parameters_are_refused() {
    let messages = errors(vec![function("dup", &["a", "a"], Vec::new())]);
    assert!(
        messages.iter().any(|m| m.contains("duplicate parameter")),
        "got: {messages:?}"
    );
}

#[test]
fn var_may_redeclare_a_function_parameter() {
    let program = compile(&module(vec![function(
        "f",
        &["codepoint"],
        vec![declaration(
            VariableKind::Var,
            "codepoint",
            Some(number(1.0)),
        )],
    )]))
    .expect("a var declaration reuses its parameter binding");
    assert!(
        program.functions[0]
            .code
            .contains(&Instruction::StoreLocal(0))
    );
}

/// A bare `return;` and falling off the end both produce undefined, and both must leave a
/// value on the stack: a caller unconditionally pops one.
#[test]
fn every_exit_from_a_function_yields_a_value() {
    let program = compile(&module(vec![function(
        "bare",
        &[],
        vec![statement(StatementKind::Return(None))],
    )]))
    .expect("compiles");
    let code = &program.functions[0].code;
    let returns = code
        .iter()
        .filter(|instruction| **instruction == Instruction::Return)
        .count();
    assert_eq!(returns, 2, "explicit return plus the fall-through epilogue");
    assert!(matches!(code.first(), Some(Instruction::Const(_))));
}

#[test]
fn an_anonymous_class_expression_does_not_report_its_hidden_binding_as_a_name() {
    // The binding a nameless class expression is held in carries a `%` so it can never
    // collide with an identifier the program wrote. That is scaffolding, and it used to
    // reach the constructor's `name`, so `(class {}).name` read back `"%class0"` -- a value
    // no engine produces and no program could have written.
    let program = compile(&module(vec![statement(StatementKind::Expression(
        Expression {
            kind: ExpressionKind::Class(Box::new(kiln_hir::Class {
                inferred_name: None,
                name: None,
                superclass: None,
                constructor: None,
                methods: Vec::new(),
                initialisers: Vec::new(),
                span: Span::default(),
            })),
            span: Span::default(),
        },
    ))]))
    .expect("an empty class compiles");

    assert!(
        program
            .functions
            .iter()
            .all(|proto| proto.name.as_deref() == Some("")),
        "{:?}",
        program
            .functions
            .iter()
            .map(|proto| proto.name.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compiles_variables_and_arithmetic() {
    let body = vec![
        Statement {
            kind: StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("answer".into()),
                    initializer: Some(Expression {
                        kind: ExpressionKind::Binary {
                            operator: BinaryOperator::Add,
                            left: Box::new(number(40.0)),
                            right: Box::new(number(2.0)),
                        },
                        span: Span::default(),
                    }),
                    span: Span::default(),
                }],
            },
            span: Span::default(),
        },
        Statement {
            kind: StatementKind::Expression(Expression {
                kind: ExpressionKind::Assignment {
                    operator: AssignmentOperator::Add,
                    target: "answer".into(),
                    value: Box::new(number(1.0)),
                },
                span: Span::default(),
            }),
            span: Span::default(),
        },
    ];
    let program = compile(&module(body)).expect("valid HIR compiles");
    assert_eq!(program.version, BYTECODE_VERSION);
    assert_eq!(program.local_count, 1);
    assert!(
        program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::Binary(BinaryOp::Add)))
    );
    assert!(
        program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::StoreLocal(0)))
    );
}

#[test]
fn emits_short_circuit_jumps_and_host_call() {
    let call = Expression {
        kind: ExpressionKind::Call {
            callee: Box::new(Expression {
                kind: ExpressionKind::StaticMember {
                    optional: false,
                    object: Box::new(identifier("console")),
                    property: "log".into(),
                },
                span: Span::default(),
            }),
            arguments: vec![kiln_hir::ListElement::Item(number(1.0))],
            optional: false,
        },
        span: Span::default(),
    };
    let body = vec![Statement {
        kind: StatementKind::Expression(Expression {
            kind: ExpressionKind::Logical {
                operator: LogicalOperator::And,
                left: Box::new(Expression {
                    kind: ExpressionKind::Literal(Literal::Boolean(true)),
                    span: Span::default(),
                }),
                right: Box::new(call),
            },
            span: Span::default(),
        }),
        span: Span::default(),
    }];
    let program = compile(&module(body)).expect("valid HIR compiles");
    assert!(
        program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::JumpIfFalse(_)))
    );
    assert!(program.code.iter().any(|instruction| matches!(
        instruction,
        Instruction::HostCall {
            op: HostOp::WriteStdout,
            argc: 1
        }
    )));
}

/// A redeclaration is a compile error; an unresolved identifier is not.
///
/// The two used to be reported together. ADR 0017 separated them: a name declared twice is a
/// statement about the text and can only ever be wrong, while a name that resolves to
/// nothing may be a host global this realm does not provide, behind a `typeof` guard that
/// never gets to run if lowering refuses the module. The unresolved one lowers to
/// `ThrowReference` and raises a `ReferenceError` if control reaches it.
#[test]
fn rejects_redeclared_locals_but_defers_unresolved_ones_to_runtime() {
    let body = vec![
        Statement {
            kind: StatementKind::Expression(identifier("missing")),
            span: Span::new(2, 9),
        },
        Statement {
            kind: StatementKind::Variable {
                kind: VariableKind::Let,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("x".into()),
                    initializer: None,
                    span: Span::new(10, 11),
                }],
            },
            span: Span::default(),
        },
        Statement {
            kind: StatementKind::Variable {
                kind: VariableKind::Var,
                declarations: vec![kiln_hir::VariableDeclarator {
                    pattern: kiln_hir::Pattern::Identifier("x".into()),
                    initializer: None,
                    span: Span::new(12, 13),
                }],
            },
            span: Span::default(),
        },
    ];
    let diagnostics = compile(&module(body)).expect_err("a redeclaration is rejected");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("redeclaration"))
    );
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("unresolved identifier")),
        "an unresolved identifier is a runtime ReferenceError, not a compile error"
    );

    // On its own it compiles, and what it compiles to is the throw.
    let program = compile(&module(vec![Statement {
        kind: StatementKind::Expression(identifier("missing")),
        span: Span::new(2, 9),
    }]))
    .expect("an unresolved identifier compiles");
    assert!(
        program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::ThrowReference(_)))
    );
}

#[test]
fn rejects_module_dependencies() {
    let mut module = module(Vec::new());
    module.requested_modules.push(kiln_hir::RequestedModule {
        specifier: "./dep.js".into(),
        occurrences: vec![kiln_hir::ModuleRequestOccurrence {
            statement_span: Span::new(0, 9),
            specifier_span: Span::new(0, 9),
            type_only: false,
            kind: kiln_hir::ModuleRequestKind::Import,
        }],
    });
    let diagnostics = compile(&module).expect_err("dependencies are not executable yet");
    assert!(diagnostics[0].message.contains("module dependency"));
    assert_eq!(diagnostics[0].span, Some(Span::new(0, 9)));
}

#[test]
fn linkage_compilation_maps_named_imports_and_local_exports_to_slots() {
    let mut module = module(vec![Statement {
        kind: StatementKind::Variable {
            kind: VariableKind::Const,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("answer".into()),
                initializer: Some(Expression {
                    kind: ExpressionKind::Binary {
                        operator: BinaryOperator::Multiply,
                        left: Box::new(identifier("input")),
                        right: Box::new(number(2.0)),
                    },
                    span: Span::default(),
                }),
                span: Span::default(),
            }],
        },
        span: Span::default(),
    }]);
    module.requested_modules.push(kiln_hir::RequestedModule {
        specifier: "./dep.js".into(),
        occurrences: vec![kiln_hir::ModuleRequestOccurrence {
            statement_span: Span::new(0, 40),
            specifier_span: Span::new(23, 33),
            type_only: false,
            kind: kiln_hir::ModuleRequestKind::Import,
        }],
    });
    module.imports.push(kiln_hir::Import {
        source: "./dep.js".into(),
        imported: ImportName::Name("value".into()),
        local: "input".into(),
        type_only: false,
        span: Span::new(9, 14),
        statement_span: Span::new(0, 40),
    });
    module.exports.push(kiln_hir::Export {
        exported: Some(ExportName::Name("answer".into())),
        origin: ExportOrigin::Local {
            local: "answer".into(),
        },
        type_only: false,
        span: Span::default(),
        statement_span: Span::default(),
    });

    let linked = compile_module(&module).expect("named scalar linkage compiles");
    assert_eq!(linked.dependencies, ["./dep.js"]);
    assert_eq!(linked.imports[0].imported, "value");
    assert_eq!(linked.imports[0].local_slot, 0);
    assert_eq!(linked.exports[0].exported, "answer");
    assert_eq!(linked.exports[0].local_slot, 1);
    // The import reads through a cell, because an import is a live view of the exporter's
    // binding rather than a copy of it. The export here is a `const` and so is *not* boxed:
    // a snapshot of something that cannot change is the same thing as a live view of it, and
    // leaving it unboxed is what keeps such a module compilable by the native backend.
    assert!(linked.program.code.contains(&Instruction::LoadCell(0)));
    assert!(linked.program.code.contains(&Instruction::StoreLocal(1)));
}

#[test]
fn linkage_compilation_rejects_assignment_to_import() {
    let mut module = module(vec![Statement {
        kind: StatementKind::Expression(Expression {
            kind: ExpressionKind::Assignment {
                operator: AssignmentOperator::Assign,
                target: "input".into(),
                value: Box::new(number(1.0)),
            },
            span: Span::default(),
        }),
        span: Span::default(),
    }]);
    module.imports.push(kiln_hir::Import {
        source: "./dep.js".into(),
        imported: ImportName::Name("value".into()),
        local: "input".into(),
        type_only: false,
        span: Span::default(),
        statement_span: Span::default(),
    });

    let diagnostics = compile_module(&module).expect_err("imports are immutable bindings");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("imported binding"))
    );
}

#[test]
fn an_object_literal_without_a_spread_stays_one_instruction() {
    // The segment machinery only earns its cost when a spread is present; the common literal
    // must not pay for it with a merge per property.
    let program = compile(&module(vec![statement(StatementKind::Expression(
        Expression {
            kind: ExpressionKind::Object {
                members: vec![
                    kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                        key: kiln_hir::ObjectKey::Static("a".into()),
                        value: number(1.0),
                        form: kiln_hir::PropertyForm::KeyValue,
                        span: Span::default(),
                    }),
                    kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                        key: kiln_hir::ObjectKey::Static("b".into()),
                        value: number(2.0),
                        form: kiln_hir::PropertyForm::KeyValue,
                        span: Span::default(),
                    }),
                ],
            },
            span: Span::default(),
        },
    ))]))
    .expect("compiles");
    assert_eq!(
        program
            .code
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::NewObject { .. }))
            .count(),
        1
    );
    assert!(!program.code.contains(&Instruction::SpreadObject));
}

#[test]
fn a_spread_splits_the_literal_into_merged_segments() {
    // `{ a: 1, ...source, b: 2 }`: the leading run, the spread, and the trailing run each
    // merge in source order, which is what makes the later property win.
    let program = compile(&module(vec![statement(StatementKind::Expression(
        Expression {
            kind: ExpressionKind::Object {
                members: vec![
                    kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                        key: kiln_hir::ObjectKey::Static("a".into()),
                        value: number(1.0),
                        form: kiln_hir::PropertyForm::KeyValue,
                        span: Span::default(),
                    }),
                    kiln_hir::ObjectMember::Spread(number(0.0)),
                    kiln_hir::ObjectMember::Property(kiln_hir::ObjectProperty {
                        key: kiln_hir::ObjectKey::Static("b".into()),
                        value: number(2.0),
                        form: kiln_hir::PropertyForm::KeyValue,
                        span: Span::default(),
                    }),
                ],
            },
            span: Span::default(),
        },
    ))]))
    .expect("compiles");
    let objects = program
        .code
        .iter()
        .filter(|instruction| matches!(instruction, Instruction::NewObject { .. }))
        .count();
    let merges = program
        .code
        .iter()
        .filter(|instruction| matches!(instruction, Instruction::SpreadObject))
        .count();
    assert_eq!((objects, merges), (2, 2));
}

#[test]
fn consecutive_spreads_do_not_build_throwaway_objects() {
    // `{ ...a, ...b }` has no properties of its own, so only the accumulator is allocated.
    let program = compile(&module(vec![statement(StatementKind::Expression(
        Expression {
            kind: ExpressionKind::Object {
                members: vec![
                    kiln_hir::ObjectMember::Spread(number(0.0)),
                    kiln_hir::ObjectMember::Spread(number(1.0)),
                ],
            },
            span: Span::default(),
        },
    ))]))
    .expect("compiles");
    assert_eq!(
        program
            .code
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::NewObject { .. }))
            .count(),
        1
    );
}

#[test]
fn a_captured_let_in_a_for_head_is_rebound_each_iteration() {
    // `for (let i = 0; ...; i++) fs.push(() => i)` must collect closures over 0, 1, 2 -- not
    // three over the value the loop ended on. The re-binding is a fresh cell per pass, and it
    // has to sit before the update so the update writes the *next* iteration's cell.
    let program = compile(&module(vec![statement(StatementKind::For {
        init: Some(Box::new(statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("i".into()),
                initializer: Some(number(0.0)),
                span: Span::default(),
            }],
        }))),
        test: None,
        update: None,
        body: Box::new(statement(StatementKind::Expression(arrow(
            &[],
            vec![statement(StatementKind::Return(Some(Expression {
                kind: ExpressionKind::Identifier("i".into()),
                span: Span::default(),
            })))],
        )))),
    })]))
    .expect("compiles");
    assert!(
        program.code.windows(3).any(|window| matches!(
            window,
            [
                Instruction::LoadCell(a),
                Instruction::NewCell(b),
                Instruction::StoreCell(c)
            ] if a == b && b == c
        )),
        "expected a per-iteration re-binding, got {:?}",
        program.code
    );
}

#[test]
fn an_uncaptured_let_in_a_for_head_is_not_rebound() {
    // Nothing can observe the difference without a closure, so nothing should pay for it.
    let program = compile(&module(vec![statement(StatementKind::For {
        init: Some(Box::new(statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("i".into()),
                initializer: Some(number(0.0)),
                span: Span::default(),
            }],
        }))),
        test: None,
        update: None,
        body: Box::new(statement(StatementKind::Empty)),
    })]))
    .expect("compiles");
    assert!(
        !program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::NewCell(_))),
        "an unboxed loop variable needs no per-iteration cell"
    );
}

#[test]
fn a_switch_evaluates_its_discriminant_once() {
    // Once, however many arms are compared against it -- so it lands in a hidden binding
    // rather than being re-emitted per test.
    let program = compile(&module(vec![statement(StatementKind::Switch {
        discriminant: number(1.0),
        cases: vec![
            kiln_hir::SwitchCase {
                test: Some(number(1.0)),
                body: Vec::new(),
            },
            kiln_hir::SwitchCase {
                test: Some(number(2.0)),
                body: Vec::new(),
            },
        ],
    })]))
    .expect("compiles");
    assert_eq!(
        program
            .code
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::StoreLocal(_)))
            .count(),
        1,
        "the discriminant is stored once: {:?}",
        program.code
    );
}

#[test]
fn continue_steps_over_a_switch_to_reach_its_loop() {
    // A `switch` catches `break` and is transparent to `continue`, so the two jumps do not
    // always choose the same enclosing construct.
    let program = compile(&module(vec![statement(StatementKind::While {
        test: number(1.0),
        body: Box::new(statement(StatementKind::Switch {
            discriminant: number(1.0),
            cases: vec![kiln_hir::SwitchCase {
                test: Some(number(1.0)),
                body: vec![statement(StatementKind::Continue { label: None })],
            }],
        })),
    })]));
    assert!(
        program.is_ok(),
        "`continue` inside a switch belongs to the loop around it"
    );
}

#[test]
fn a_continue_with_no_loop_at_all_is_still_refused() {
    let messages = errors(vec![statement(StatementKind::Switch {
        discriminant: number(1.0),
        cases: vec![kiln_hir::SwitchCase {
            test: Some(number(1.0)),
            body: vec![statement(StatementKind::Continue { label: None })],
        }],
    })]);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("`continue` outside of a loop")),
        "{messages:?}"
    );
}

#[test]
fn a_template_concatenates_from_a_string() {
    // Starting from a string is what makes every `+` after it an append rather than a
    // numeric add, so `${1}${2}` is "12".
    let program = compile(&module(vec![statement(StatementKind::Expression(
        Expression {
            kind: ExpressionKind::Template {
                quasis: vec![String::new(), String::new(), String::new()],
                expressions: vec![number(1.0), number(2.0)],
            },
            span: Span::default(),
        },
    ))]))
    .expect("compiles");
    assert!(matches!(program.code.first(), Some(Instruction::Const(_))));
    assert_eq!(
        program
            .code
            .iter()
            .filter(|instruction| matches!(instruction, Instruction::Binary(BinaryOp::Add)))
            .count(),
        2,
        "one append per hole, and none for the empty text between them"
    );
}

#[test]
fn a_call_without_a_spread_keeps_its_arity_in_the_bytecode() {
    // The fixed-arity path is the common one and must not pay for the general one: no array
    // is built and no `Apply` is emitted.
    let program = compile(&module(vec![
        function("f", &["x"], vec![]),
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(Expression {
                    kind: ExpressionKind::Identifier("f".into()),
                    span: Span::default(),
                }),
                arguments: vec![kiln_hir::ListElement::Item(number(1.0))],
                optional: false,
            },
            span: Span::default(),
        })),
    ]))
    .expect("compiles");
    assert!(program.code.contains(&Instruction::Call { argc: 1 }));
    assert!(!program.code.contains(&Instruction::Apply));
}

#[test]
fn a_call_with_a_spread_builds_its_arguments_at_runtime() {
    // A spread's length is not known until it runs, so the arity leaves the bytecode and
    // becomes the length of an array the program computes.
    let program = compile(&module(vec![
        function("f", &["x"], vec![]),
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(Expression {
                    kind: ExpressionKind::Identifier("f".into()),
                    span: Span::default(),
                }),
                arguments: vec![kiln_hir::ListElement::Spread(number(1.0))],
                optional: false,
            },
            span: Span::default(),
        })),
    ]))
    .expect("compiles");
    assert!(program.code.contains(&Instruction::Apply));
    assert!(
        !program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::Call { .. }))
    );
}

#[test]
fn a_function_declaration_is_emitted_before_the_statements_around_it() {
    // `f(); function f() {}` is ordinary JavaScript, so the value has to exist before the
    // line that writes it.
    let program = compile(&module(vec![
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::Call {
                callee: Box::new(identifier("f")),
                arguments: Vec::new(),
                optional: false,
            },
            span: Span::default(),
        })),
        function("f", &[], vec![]),
    ]))
    .expect("compiles");
    let make = program
        .code
        .iter()
        .position(|instruction| matches!(instruction, Instruction::MakeFunction(_)))
        .expect("the function is built");
    let call = program
        .code
        .iter()
        .position(|instruction| matches!(instruction, Instruction::Call { .. }))
        .expect("the call is emitted");
    assert!(make < call, "the binding is filled before it is read");
}

#[test]
fn a_direct_read_before_a_declaration_throws_where_the_read_is() {
    // `x; let x = 1` is well-formed JavaScript: it compiles, and the read throws a
    // `ReferenceError` when it is reached. Pre-declaring every name in a block would
    // otherwise turn that into a silent `undefined`, which is the trade the dead-zone
    // marker exists to avoid -- so the read becomes one of a cell that is uninitialised by
    // construction, and the source is not refused.
    let program = compile(&module(vec![
        statement(StatementKind::Expression(identifier("x"))),
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("x".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        }),
    ]))
    .expect("a temporal dead zone is not a compile error");
    assert!(
        program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::MarkUninitialized(_))),
        "{:?}",
        program.code
    );
}

#[test]
fn an_uncaptured_module_lexical_needs_a_runtime_dead_zone_marker() {
    let program = compile(&module(vec![statement(StatementKind::Variable {
        kind: VariableKind::Let,
        declarations: vec![kiln_hir::VariableDeclarator {
            pattern: kiln_hir::Pattern::Identifier("value".into()),
            initializer: Some(number(1.0)),
            span: Span::default(),
        }],
    })]))
    .expect("compiles");

    assert!(
        program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::MarkUninitialized(_)))
    );
}

#[test]
fn a_module_lexical_captured_by_a_nested_function_needs_a_dead_zone_marker() {
    let program = compile(&module(vec![
        function(
            "read",
            &[],
            vec![statement(StatementKind::Return(Some(identifier("value"))))],
        ),
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("value".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        }),
    ]))
    .expect("compiles");

    assert!(
        program
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::MarkUninitialized(_)))
    );
}

#[test]
fn a_script_rejects_top_level_await_with_a_module_only_diagnostic() {
    let source = statement(StatementKind::Expression(Expression {
        kind: ExpressionKind::Await(Box::new(number(1.0))),
        span: Span::new(4, 9),
    }));
    let diagnostics = compile(&module(vec![source])).expect_err("script await is rejected");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.message == "top-level `await` is only available in a module"
            && diagnostic.span == Some(Span::new(4, 9))
    }));
}

#[test]
fn a_var_read_before_its_declaration_is_undefined_rather_than_refused() {
    // The one kind that is initialised on entry, which is the whole difference.
    let program = compile(&module(vec![
        statement(StatementKind::Expression(identifier("x"))),
        statement(StatementKind::Variable {
            kind: VariableKind::Var,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("x".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        }),
    ]));
    assert!(program.is_ok(), "{program:?}");
}

#[test]
fn an_enclosing_functions_binding_shadows_a_module_binding() {
    // Resolution used to go from the inner function straight to module scope, skipping every
    // function in between -- so an inner parameter sharing a name with a module binding read
    // the wrong one. Invisible until a module binding existed before the line declaring it.
    let program = compile(&module(vec![
        statement(StatementKind::Function(Box::new(kiln_hir::Function {
            strict: false,
            binds_own_name: false,
            asynchronous: false,
            name: Some("outer".into()),
            arrow: false,
            generator: false,
            parameters: vec![kiln_hir::Parameter {
                pattern: kiln_hir::Pattern::Identifier("shared".into()),
                type_annotation: None,
                span: Span::default(),
            }],
            rest: None,
            return_type: None,
            body: vec![statement(StatementKind::Return(Some(arrow(
                &[],
                vec![statement(StatementKind::Return(Some(identifier("shared"))))],
            ))))],
            span: Span::default(),
        }))),
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("shared".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        }),
    ]))
    .expect("compiles");
    // The arrow reads the parameter it captured, not the module slot.
    let arrow_body = &program.functions[0].code;
    assert!(
        arrow_body
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadUpvalue(_))),
        "expected a capture, got {arrow_body:?}"
    );
    assert!(
        !arrow_body
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_)))
    );
}

/// `[o.p] = xs` evaluates `o` before it reads the element, not after.
///
/// The order is observable whenever either side throws or runs a getter, and it is the one
/// thing a pattern walk cannot get right by emitting the value first and the target second --
/// which is what every other leaf does.
#[test]
fn a_reference_target_is_evaluated_before_the_value_it_receives() {
    let body = vec![
        statement(StatementKind::Variable {
            kind: VariableKind::Var,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("o".into()),
                initializer: None,
                span: Span::default(),
            }],
        }),
        statement(StatementKind::Variable {
            kind: VariableKind::Var,
            declarations: vec![kiln_hir::VariableDeclarator {
                pattern: kiln_hir::Pattern::Identifier("xs".into()),
                initializer: None,
                span: Span::default(),
            }],
        }),
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::PatternAssignment {
                pattern: kiln_hir::Pattern::Array {
                    elements: vec![Some(kiln_hir::Pattern::Member {
                        object: Box::new(identifier("o")),
                        property: "p".into(),
                    })],
                    rest: None,
                },
                value: Box::new(identifier("xs")),
            },
            span: Span::default(),
        })),
    ];
    let program = compile(&module(body)).expect("valid HIR compiles");
    let at = |wanted: &Instruction| {
        program
            .code
            .iter()
            .position(|instruction| instruction == wanted)
            .expect("instruction is emitted")
    };
    let receiver = at(&Instruction::LoadLocal(0));
    // The value an element receives is whatever stepping the iterator produced, so the step
    // is what has to come after the receiver and before the store. The step is an `Apply` of
    // the `next` the record captured, not a fresh read of it: the read happens once, when the
    // iterator is opened, which is before the first element's receiver is even evaluated.
    let next = program
        .constants
        .iter()
        .position(|constant| matches!(constant, Constant::String(name) if name == "next"))
        .expect("the walk names the method it captures") as u32;
    let captured = at(&Instruction::GetProperty(next));
    assert!(
        captured < receiver,
        "the iterator record is complete before any element is taken"
    );
    let read = at(&Instruction::Apply);
    let store = program
        .code
        .iter()
        .position(|instruction| {
            matches!(
                instruction,
                Instruction::SetProperty(_) | Instruction::SetPropertyStrict { .. }
            )
        })
        .expect("the element is stored through the reference");
    assert!(receiver < read, "the receiver is evaluated before the read");
    assert!(read < store, "the value exists before it is stored");
}

/// `let [o.p] = xs` is a syntax error, and the parser says so first -- but nothing in the
/// HIR shape prevents a frontend from building it, so the walk refuses it rather than
/// storing through a reference in a position that only ever declares names.
#[test]
fn a_declaration_cannot_bind_through_a_reference() {
    let body = vec![statement(StatementKind::Variable {
        kind: VariableKind::Let,
        declarations: vec![kiln_hir::VariableDeclarator {
            pattern: kiln_hir::Pattern::Array {
                elements: vec![Some(kiln_hir::Pattern::Member {
                    object: Box::new(identifier("o")),
                    property: "p".into(),
                })],
                rest: None,
            },
            initializer: Some(identifier("xs")),
            span: Span::default(),
        }],
    })];
    let diagnostics = compile(&module(body)).expect_err("a declaration is refused");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("property reference")),
        "the diagnostic names what it refused: {diagnostics:?}"
    );
}

#[test]
fn compile_module_detects_immutable_and_mutable_bindings() {
    let mut m = module(vec![
        statement(StatementKind::Variable {
            kind: VariableKind::Const,
            declarations: vec![VariableDeclarator {
                pattern: Pattern::Identifier("a".into()),
                initializer: Some(number(1.0)),
                span: Span::default(),
            }],
        }),
        statement(StatementKind::Variable {
            kind: VariableKind::Let,
            declarations: vec![VariableDeclarator {
                pattern: Pattern::Identifier("b".into()),
                initializer: Some(number(2.0)),
                span: Span::default(),
            }],
        }),
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::Assignment {
                operator: AssignmentOperator::Assign,
                target: "b".into(),
                value: Box::new(number(3.0)),
            },
            span: Span::default(),
        })),
        function("c", &[], vec![]),
    ]);
    m.exports.push(Export {
        exported: Some(ExportName::Name("a".into())),
        origin: ExportOrigin::Local { local: "a".into() },
        type_only: false,
        span: Span::default(),
        statement_span: Span::default(),
    });
    m.exports.push(Export {
        exported: Some(ExportName::Name("b".into())),
        origin: ExportOrigin::Local { local: "b".into() },
        type_only: false,
        span: Span::default(),
        statement_span: Span::default(),
    });
    m.exports.push(Export {
        exported: Some(ExportName::Name("c".into())),
        origin: ExportOrigin::Local { local: "c".into() },
        type_only: false,
        span: Span::default(),
        statement_span: Span::default(),
    });

    let linked = compile_module(&m).expect("compilation succeeds");
    assert!(
        linked
            .link_facts
            .immutable_bindings
            .contains(&"a".to_string())
    );
    assert!(
        !linked
            .link_facts
            .immutable_bindings
            .contains(&"b".to_string())
    );
    assert!(
        linked
            .link_facts
            .immutable_bindings
            .contains(&"c".to_string())
    );
    assert!(!linked.link_facts.direct_eval);
    assert!(!linked.link_facts.new_function);
    assert!(linked.link_facts.dynamic_imports.is_empty());
}

#[test]
fn compile_module_detects_direct_eval() {
    let m = module(vec![statement(StatementKind::Expression(Expression {
        kind: ExpressionKind::Call {
            callee: Box::new(identifier("eval")),
            arguments: vec![ListElement::Item(Expression {
                kind: ExpressionKind::Literal(Literal::String("1 + 1".into())),
                span: Span::default(),
            })],
            optional: false,
        },
        span: Span::default(),
    }))]);
    let linked = compile_module(&m).expect("compilation succeeds");
    assert!(linked.link_facts.direct_eval);
}

#[test]
fn compile_module_detects_new_function() {
    let m = module(vec![statement(StatementKind::Expression(Expression {
        kind: ExpressionKind::New {
            callee: Box::new(identifier("Function")),
            arguments: vec![ListElement::Item(Expression {
                kind: ExpressionKind::Literal(Literal::String("return 42;".into())),
                span: Span::default(),
            })],
        },
        span: Span::default(),
    }))]);
    let linked = compile_module(&m).expect("compilation succeeds");
    assert!(linked.link_facts.new_function);
}

#[test]
fn compile_module_detects_dynamic_imports() {
    let m = module(vec![
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::ImportCall {
                specifier: Box::new(Expression {
                    kind: ExpressionKind::Literal(Literal::String("foo.js".into())),
                    span: Span::default(),
                }),
                options: None,
            },
            span: Span::default(),
        })),
        statement(StatementKind::Expression(Expression {
            kind: ExpressionKind::ImportCall {
                specifier: Box::new(identifier("runtimeSpecifier")),
                options: None,
            },
            span: Span::default(),
        })),
    ]);
    let linked = compile_module(&m).expect("compilation succeeds");
    assert_eq!(
        linked.link_facts.dynamic_imports,
        vec![
            DynamicImportTarget::Static("foo.js".into()),
            DynamicImportTarget::Dynamic
        ]
    );
}
