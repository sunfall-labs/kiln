use super::*;

#[test]
fn generic_optimization_selection_is_bounded_ordered_and_frame_conservative() {
    let first = function(vec![Instruction::Return]);
    let mut generator = function(vec![Instruction::Return]);
    generator.generator = true;
    let third = function(vec![Instruction::Return; 321]);
    let fourth = function(vec![Instruction::Return]);
    let mut prog = program(Vec::new(), vec![Instruction::Return], 0);
    prog.functions = vec![first, generator, third, fourth];

    let selected =
        crate::direct::select_generic_optimization_functions(&prog, &[false, false, false, false]);
    assert_eq!(selected, [true, false, false, true]);
    assert_eq!(
        selected,
        crate::direct::select_generic_optimization_functions(&prog, &[false, false, false, false],),
        "selection is deterministic"
    );
    assert_eq!(
        crate::direct::select_generic_optimization_functions(&prog, &[false]),
        [true, false, false, false],
        "missing publication facts conservatively exclude later functions"
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
        inferred_name: None,
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::Number(0), Constant::Number(1)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
    // The unchanged parameter's trivial phi disappears; the live sum and induction phis
    // remain numeric. Renumbering must not turn the actual parameter into a proven number.
    assert!(!numbers.contains(&graph.initial_locals[0]));
    assert_eq!(phis.len(), 2);
    assert!(phis.iter().all(|value| numbers.contains(value)));

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
        inferred_name: None,
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::Number(0), Constant::Number(1)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
    assert_eq!(assert_speculation_invariants(SPECULATION_INVARIANTS), 8);
    for name in [
        "parameter_number",
        "loop_speculation_exit",
        "upvalue_array",
        "parameter_array",
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
        inferred_name: None,
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::Number(0), Constant::Number(1)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let generated = emit_with_policy(
        &prog,
        EmissionPolicy {
            speculate: Some(false),
            ..Default::default()
        },
    )
    .expect("emits with speculation disabled");
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
        inferred_name: None,
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(0),
            Constant::Number(10),
            Constant::Number(1),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
        inferred_name: None,
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(0),
            Constant::Number(10),
            Constant::Number(1),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &program).unwrap();
    assert!(!graph.blocks.is_empty());
    assert!(verify_function_graph(&graph).is_ok());
}

#[test]
fn value_graph_builds_and_verifies_try_catch() {
    let proto = FunctionProto {
        inferred_name: None,
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::String("err".into()), Constant::Number(42)],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &program).unwrap();
    assert!(verify_function_graph(&graph).is_ok());
}

#[test]
fn value_graph_does_not_protect_an_operation_before_try() {
    let proto = FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: Some("pre_try_call".into()),
        parameter_count: 0,
        local_count: 0,
        code: vec![
            Instruction::Const(0),
            Instruction::Call { argc: 0 },
            Instruction::Pop,
            Instruction::PushHandler(8),
            Instruction::Const(0),
            Instruction::Call { argc: 0 },
            Instruction::PopHandler,
            Instruction::Return,
            Instruction::Return,
        ],
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
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::Undefined],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let graph = build_function_graph(&proto, &program).unwrap();
    let mut handler_by_call_pc = std::collections::BTreeMap::new();
    for block in &graph.blocks {
        for (value, operation) in &block.operations {
            if matches!(operation, GraphOp::Call { .. }) {
                handler_by_call_pc.insert(graph.origins[value].pc, block.exception_handler);
            }
        }
    }

    assert_eq!(handler_by_call_pc[&1], None);
    assert!(handler_by_call_pc[&5].is_some());
    assert!(verify_function_graph(&graph).is_ok());
}

#[test]
fn closed_world_module_omits_an_unread_exported_function_body() {
    let function = || FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: None,
        parameter_count: 0,
        local_count: 0,
        code: vec![Instruction::Const(0), Instruction::Return],
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
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::MakeFunction(1),
                Instruction::StoreLocal(1),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 2,
            functions: vec![function(), function()],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![
            kiln_ir::ExportBinding {
                exported: "kept".into(),
                local_slot: 0,
            },
            kiln_ir::ExportBinding {
                exported: "dropped".into(),
                local_slot: 1,
            },
        ],
    );
    for compact in [true, false] {
        let emitted = emit_module_with_logical_path_and_policy(
            &module,
            "fixture.js",
            EmissionPolicy {
                compact,
                live_module_slots: Some(std::sync::Arc::new([0].into_iter().collect())),
                ..EmissionPolicy::default()
            },
        )
        .expect("closed-world module emits");
        let kept = if compact { "ku_fn_0(" } else { "_fn_0(" };
        let dropped = if compact { "ku_fn_1" } else { "_fn_1" };

        assert!(emitted.source.contains(kept));
        assert!(!emitted.source.contains(dropped));
    }
}

#[test]
fn closed_world_module_keeps_a_declaration_captured_by_a_live_function() {
    let function = |upvalues: Vec<Upvalue>| FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: None,
        parameter_count: 0,
        local_count: 0,
        code: vec![Instruction::Const(0), Instruction::Return],
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        mapped_arguments: false,
        upvalues,
        type_annotations: None,
    };
    let module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::MakeFunction(1),
                Instruction::StoreLocal(1),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 2,
            functions: vec![
                function(Vec::new()),
                function(vec![Upvalue::ParentLocal(0)]),
            ],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "capturing".into(),
            local_slot: 1,
        }],
    );
    let emitted = emit_module_with_logical_path_and_policy(
        &module,
        "fixture.js",
        EmissionPolicy {
            compact: true,
            live_module_slots: Some(std::sync::Arc::new([1].into_iter().collect())),
            ..EmissionPolicy::default()
        },
    )
    .expect("closed-world module emits");

    assert!(emitted.source.contains("ku_fn_0("));
    assert!(emitted.source.contains("ku_fn_1("));
    assert!(
        emitted
            .source
            .contains("exports[0] = kiln_val_binding_to_boundary(locals[1]);")
    );
    let runtime = runtime_prelude_source();
    assert!(runtime.contains("kiln_value kiln_val_binding_to_boundary(kiln_val v)"));
    assert!(runtime.contains("v = *(const kiln_val *)KILN_VAL_PTR(v);"));
}

#[test]
fn split_module_instantiation_retains_cells_captured_by_live_hoisted_functions() {
    let function = FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: Some("cached".into()),
        parameter_count: 0,
        local_count: 0,
        code: vec![Instruction::LoadUpvalue(0), Instruction::Return],
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        mapped_arguments: false,
        upvalues: vec![Upvalue::ParentLocal(1)],
        type_annotations: None,
    };
    let mut module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::NewCell(1),
                Instruction::MarkUninitialized(1),
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::StoreCell(1),
                Instruction::Return,
            ],
            local_count: 3,
            functions: vec![function],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "cached".into(),
            local_slot: 0,
        }],
    );
    module.link_facts.module_instantiation_end = 4;
    // Evaluation-only dead declaration metadata must not erase instantiation's new return.
    module
        .link_facts
        .removable_declarations
        .push(kiln_ir::RemovableDeclaration {
            local_slots: vec![2],
            pc_start: 4,
            pc_end: 7,
        });
    let emitted = emit_module_with_logical_path_and_policy_and_instantiation(
        &module,
        "fixture.js",
        EmissionPolicy {
            compact: true,
            live_module_slots: Some(std::sync::Arc::new([0].into_iter().collect())),
            ..EmissionPolicy::default()
        },
        true,
    )
    .expect("split module emits");

    let instantiation = emitted
        .instantiation
        .expect("split module has an instantiation entrypoint");
    assert!(
        instantiation
            .source
            .contains("kiln_c99_val_make_cell_owned(&locals[1]")
    );
    assert!(instantiation.source.contains("KILN_VAL_UNINITIALIZED"));
    assert!(
        instantiation
            .source
            .contains(&format!("{}_fn_0(", emitted.symbol))
    );
    assert!(instantiation.source.contains("return KILN_C99_OK;"));
    assert!(
        !instantiation
            .source
            .contains("\n    goto kiln_refusal;\nkiln_refusal:;")
    );
}

#[test]
fn split_module_without_closures_gives_each_phase_a_distinct_entry_symbol() {
    let module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "value".into(),
            local_slot: 0,
        }],
    );
    let emitted = emit_module_with_logical_path_and_policy_and_instantiation(
        &module,
        "cycle/value.mjs",
        EmissionPolicy::default(),
        true,
    )
    .expect("split module emits");
    let instantiation = emitted.instantiation.expect("instantiation unit");

    assert!(emitted.source.contains(&format!("int {}(", emitted.symbol)));
    assert!(
        instantiation
            .source
            .contains(&format!("int {}_instantiate(", emitted.symbol))
    );
    assert!(
        !instantiation
            .source
            .contains(&format!("int {}(", emitted.symbol))
    );
}

#[test]
fn split_module_instantiation_keeps_transitively_reachable_hoisted_functions() {
    let function = |upvalues: Vec<Upvalue>| FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: None,
        parameter_count: 0,
        local_count: 0,
        code: if upvalues.is_empty() {
            vec![Instruction::Const(0), Instruction::Return]
        } else {
            vec![Instruction::LoadUpvalue(0), Instruction::Return]
        },
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        mapped_arguments: false,
        upvalues,
        type_annotations: None,
    };
    let mut module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::MakeFunction(1),
                Instruction::StoreLocal(1),
                Instruction::MakeFunction(2),
                Instruction::StoreLocal(2),
                Instruction::MakeFunction(3),
                Instruction::StoreLocal(3),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 4,
            functions: vec![
                function(Vec::new()),
                function(vec![Upvalue::ParentLocal(0)]),
                function(vec![Upvalue::ParentLocal(1)]),
                function(Vec::new()),
            ],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "run".into(),
            local_slot: 2,
        }],
    );
    module.link_facts.module_instantiation_end = 8;
    let emitted = emit_module_with_logical_path_and_policy_and_instantiation(
        &module,
        "fixture.js",
        EmissionPolicy {
            compact: true,
            live_module_slots: Some(std::sync::Arc::new([2].into_iter().collect())),
            ..EmissionPolicy::default()
        },
        true,
    )
    .expect("split module emits");
    let instantiation = emitted.instantiation.expect("instantiation entrypoint");
    for function in [0, 1, 2] {
        assert!(
            instantiation
                .source
                .contains(&format!("{}_fn_{function}(", emitted.symbol)),
            "function {function} must remain available to evaluation"
        );
    }
    assert!(
        !instantiation
            .source
            .contains(&format!("{}_fn_3(", emitted.symbol)),
        "unreachable declaration remains dead"
    );
}

#[test]
fn closed_world_module_omits_an_unread_declaration_inside_a_live_function() {
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
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: vec![
                function(
                    1,
                    vec![
                        Instruction::MakeFunction(1),
                        Instruction::StoreLocal(0),
                        Instruction::Const(0),
                        Instruction::Return,
                    ],
                ),
                function(0, vec![Instruction::Const(0), Instruction::Return]),
            ],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "outer".into(),
            local_slot: 0,
        }],
    );
    let emitted = emit_module_with_logical_path_and_policy(
        &module,
        "fixture.js",
        EmissionPolicy {
            compact: true,
            live_module_slots: Some(std::sync::Arc::new([0].into_iter().collect())),
            ..EmissionPolicy::default()
        },
    )
    .expect("closed-world module emits");

    assert!(emitted.source.contains("ku_fn_0("));
    assert!(!emitted.source.contains("ku_fn_1("));
}

#[test]
fn closed_world_module_keeps_a_nested_declaration_captured_by_a_live_sibling() {
    let function = |local_count, code, upvalues| FunctionProto {
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
        upvalues,
        type_annotations: None,
    };
    let module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: vec![
                function(
                    2,
                    vec![
                        Instruction::MakeFunction(1),
                        Instruction::StoreLocal(0),
                        Instruction::MakeFunction(2),
                        Instruction::StoreLocal(1),
                        Instruction::LoadLocal(1),
                        Instruction::Return,
                    ],
                    Vec::new(),
                ),
                function(
                    0,
                    vec![Instruction::Const(0), Instruction::Return],
                    Vec::new(),
                ),
                function(
                    0,
                    vec![Instruction::LoadUpvalue(0), Instruction::Return],
                    vec![Upvalue::ParentLocal(0)],
                ),
            ],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "outer".into(),
            local_slot: 0,
        }],
    );
    let emitted = emit_module_with_logical_path_and_policy(
        &module,
        "fixture.js",
        EmissionPolicy {
            compact: true,
            live_module_slots: Some(std::sync::Arc::new([0].into_iter().collect())),
            ..EmissionPolicy::default()
        },
    )
    .expect("closed-world module emits");

    assert!(emitted.source.contains("ku_fn_0("));
    assert!(emitted.source.contains("ku_fn_1("));
    assert!(emitted.source.contains("ku_fn_2("));
}

#[test]
fn nested_parent_local_does_not_keep_an_unrelated_module_slot() {
    let proto = |code, upvalues| FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 0,
        name: None,
        parameter_count: 0,
        local_count: 1,
        code,
        arrow: false,
        generator: false,
        asynchronous: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        mapped_arguments: false,
        upvalues,
        type_annotations: None,
    };
    let module = module(
        Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::MakeFunction(0),
                Instruction::StoreLocal(0),
                Instruction::MakeFunction(1),
                Instruction::StoreLocal(1),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 2,
            functions: vec![
                proto(vec![Instruction::Const(0), Instruction::Return], Vec::new()),
                proto(
                    vec![
                        Instruction::MakeFunction(2),
                        Instruction::StoreLocal(0),
                        Instruction::LoadLocal(0),
                        Instruction::Return,
                    ],
                    Vec::new(),
                ),
                proto(
                    vec![Instruction::LoadUpvalue(0), Instruction::Return],
                    vec![Upvalue::ParentLocal(0)],
                ),
            ],
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        },
        Vec::new(),
        vec![kiln_ir::ExportBinding {
            exported: "outer".into(),
            local_slot: 1,
        }],
    );
    let emitted = emit_module_with_logical_path_and_policy(
        &module,
        "fixture.js",
        EmissionPolicy {
            compact: true,
            live_module_slots: Some(std::sync::Arc::new([1].into_iter().collect())),
            ..EmissionPolicy::default()
        },
    )
    .expect("closed-world module emits");

    assert!(!emitted.source.contains("ku_fn_0("));
    assert!(emitted.source.contains("ku_fn_1("));
    assert!(emitted.source.contains("ku_fn_2("));
}

#[test]
fn closed_world_namespace_records_named_reads_across_argument_evaluation() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::String("Literals".into()), Constant::Number(1)],
        code: vec![
            Instruction::LoadLocal(0),
            Instruction::Const(1),
            Instruction::NewArray { count: 1 },
            Instruction::CallMethod { name: 0, argc: 1 },
            Instruction::Pop,
            Instruction::Const(1),
            Instruction::Return,
        ],
        local_count: 1,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    assert_eq!(
        closed_world_namespace_members(&program, 0, false),
        Some(["Literals".to_owned()].into_iter().collect())
    );
}

#[test]
fn closed_world_namespace_traces_a_captured_module_binding() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::String("fromInputUnsafe".into())],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(1),
            Instruction::Const(0),
            Instruction::Return,
        ],
        local_count: 2,
        functions: vec![FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            declared_length: 0,
            name: None,
            parameter_count: 0,
            local_count: 0,
            code: vec![
                Instruction::LoadUpvalue(0),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::Return,
            ],
            arrow: true,
            generator: false,
            asynchronous: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            mapped_arguments: false,
            upvalues: vec![Upvalue::ParentLocal(0)],
            type_annotations: None,
        }],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    assert_eq!(
        closed_world_namespace_members(&program, 0, false),
        Some(["fromInputUnsafe".to_owned()].into_iter().collect())
    );
}

#[test]
fn closed_world_namespace_traces_local_alias_across_generator_suspension() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::String("member".into()), Constant::Number(1)],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(1),
            Instruction::Const(1),
            Instruction::Return,
        ],
        local_count: 2,
        functions: vec![FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            declared_length: 0,
            name: None,
            parameter_count: 0,
            local_count: 1,
            code: vec![
                Instruction::LoadUpvalue(0),
                Instruction::StoreLocal(0),
                Instruction::Const(1),
                Instruction::Yield,
                Instruction::Pop,
                Instruction::LoadLocal(0),
                Instruction::CallMethod { name: 0, argc: 0 },
                Instruction::Return,
            ],
            arrow: false,
            generator: true,
            asynchronous: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            mapped_arguments: false,
            upvalues: vec![Upvalue::ParentLocal(0)],
            type_annotations: None,
        }],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    assert_eq!(
        closed_world_namespace_members(&program, 0, false),
        Some(["member".to_owned()].into_iter().collect())
    );
}

#[test]
fn closed_world_namespace_keeps_every_member_when_a_capture_escapes() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![Constant::Number(0)],
        code: vec![
            Instruction::MakeFunction(0),
            Instruction::StoreLocal(1),
            Instruction::Const(0),
            Instruction::Return,
        ],
        local_count: 2,
        functions: vec![FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            declared_length: 0,
            name: None,
            parameter_count: 0,
            local_count: 0,
            code: vec![Instruction::LoadUpvalue(0), Instruction::Return],
            arrow: true,
            generator: false,
            asynchronous: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            mapped_arguments: false,
            upvalues: vec![Upvalue::ParentLocal(0)],
            type_annotations: None,
        }],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    assert_eq!(closed_world_namespace_members(&program, 0, false), None);
}

#[test]
fn closed_world_namespace_keeps_every_member_when_the_value_escapes() {
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::LoadLocal(0), Instruction::Return],
        local_count: 1,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    assert_eq!(closed_world_namespace_members(&program, 0, false), None);
}

#[test]
fn value_graph_lowers_to_c_without_stack_or_sp() {
    let proto = FunctionProto {
        inferred_name: None,
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
            Instruction::JumpIfFalse(17),
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };

    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(0),
            Constant::Number(10),
            Constant::Number(1),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
        inferred_name: None,
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
        is_class: false,
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![untyped_proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let typed_prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![typed_proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
    let compact_typed_c = emit_with_policy(
        &typed_prog,
        EmissionPolicy {
            compact: true,
            ..EmissionPolicy::default()
        },
    )
    .unwrap();

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
    assert!(
        compact_typed_c
            .source
            .contains("/* Clone A: guarded fast path */")
    );
    assert!(
        compact_typed_c
            .source
            .contains("KILN_VAL_IS_DOUBLE(args[0])")
    );

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
        inferred_name: None,
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
        is_class: false,
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![untyped_proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let typed_prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![typed_proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
        inferred_name: None,
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
        is_class: false,
        mapped_arguments: false,
        upvalues: Vec::new(),
        type_annotations: None,
    };

    let proto = make_proto();
    let prog = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
            generic_only: true,
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
fn pure_closure_classification_and_parallel_map_emission() {
    use crate::graph::build_function_graph;

    // A pure callback: (x) => x * 2
    let callback = FunctionProto {
        inferred_name: None,
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(2f64.to_bits()),
            Constant::String("parallelMap".into()),
        ],
        code: Vec::new(),
        local_count: 0,
        functions: vec![callback.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(2f64.to_bits()),
            Constant::String("parallelMap".into()),
        ],
        code: module_code,
        local_count: 0,
        functions: vec![callback],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    let generated = emit(&full_program).expect("parallelMap emission succeeds");
    assert!(
        generated.source.contains("built_arr"),
        "emitted C should contain native array lowering for parallelMap"
    );
    assert!(
        generated
            .source
            .contains("#pragma clang loop vectorize(enable)"),
        "emitted C should contain vectorization hint for chunk workers"
    );
    assert!(
        generated.source.contains("in_arr"),
        "emitted C should use in_arr restrict pointer in chunk workers"
    );
    assert!(
        generated.source.contains("out_arr"),
        "emitted C should use out_arr restrict pointer in chunk workers"
    );
    assert!(
        generated.source.contains("_pmap_f64_"),
        "emitted C should contain specialized f64 chunk worker"
    );
    assert!(
        generated.source.contains("const double * restrict in_arr"),
        "emitted C should define in_arr restrict pointer for f64"
    );
    assert!(
        generated.source.contains("double * restrict out_arr"),
        "emitted C should define out_arr restrict pointer for f64"
    );
}

#[test]
fn test_proven_types_recognize_new_object_and_method_returns() {
    use crate::graph::{Proven, build_function_graph, proven_types_with_bounds};
    use std::collections::{HashMap, HashSet};

    let mut proto = function(vec![
        Instruction::NewObject { count: 0 },
        Instruction::StoreLocal(0), // obj
        Instruction::NewArray { count: 0 },
        Instruction::StoreLocal(1), // arr
        Instruction::LoadLocal(1),
        Instruction::Const(5),
        Instruction::CallMethod { name: 0, argc: 1 }, // arr.indexOf(0)
        Instruction::StoreLocal(2),                   // idx
        Instruction::LoadLocal(1),
        Instruction::Const(6),
        Instruction::CallMethod { name: 1, argc: 1 }, // arr.push(1)
        Instruction::StoreLocal(3),                   // len
        Instruction::Const(4),                        // "abc"
        Instruction::Const(5),                        // 0
        Instruction::Const(6),                        // 1
        Instruction::CallMethod { name: 2, argc: 2 }, // "abc".substring(0, 1)
        Instruction::StoreLocal(4),                   // sub
        Instruction::LoadLocal(4),
        Instruction::GetProperty(3), // sub.length
        Instruction::StoreLocal(5),  // subLen
        Instruction::Return,
    ]);
    proto.local_count = 6;
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::String("indexOf".to_owned().into()),   // 0
            Constant::String("push".to_owned().into()),      // 1
            Constant::String("substring".to_owned().into()), // 2
            Constant::String("length".to_owned().into()),    // 3
            Constant::String("abc".to_owned().into()),       // 4
            Constant::Number(0.0f64.to_bits()),              // 5
            Constant::Number(1.0f64.to_bits()),              // 6
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let graph = build_function_graph(&proto, &program).unwrap();
    let proven = proven_types_with_bounds(&graph, &HashMap::new(), &HashSet::new());
    let intrinsics = crate::direct::intrinsic_results(&graph, &program);

    // 1. NewObject is Proven::Object
    let obj_vid = graph.blocks[0]
        .operations
        .iter()
        .find_map(|(v, op)| matches!(op, GraphOp::NewObject { .. }).then_some(*v))
        .expect("NewObject operation exists");
    assert_eq!(proven.get(&obj_vid), Some(&Proven::Object));

    // 2. indexOf is Proven::Int
    let index_of_vid = graph.blocks[0]
        .operations
        .iter()
        .find_map(|(v, op)| {
            if let GraphOp::CallMethod { name, .. } = op
                && *name == 0
            {
                Some(*v)
            } else {
                None
            }
        })
        .expect("indexOf operation exists");
    assert_eq!(intrinsics.get(&index_of_vid), Some(&Proven::Int));

    // 3. push is Proven::Int
    let push_vid = graph.blocks[0]
        .operations
        .iter()
        .find_map(|(v, op)| {
            if let GraphOp::CallMethod { name, .. } = op
                && *name == 1
            {
                Some(*v)
            } else {
                None
            }
        })
        .expect("push operation exists");
    assert_eq!(intrinsics.get(&push_vid), Some(&Proven::Int));

    // 4. substring is Proven::String
    let substring_vid = graph.blocks[0]
        .operations
        .iter()
        .find_map(|(v, op)| {
            if let GraphOp::CallMethod { name, .. } = op
                && *name == 2
            {
                Some(*v)
            } else {
                None
            }
        })
        .expect("substring operation exists");
    assert_eq!(intrinsics.get(&substring_vid), Some(&Proven::String));

    // 5. sub.length is Proven::Int
    let sub_len_vid = graph.blocks[0]
        .operations
        .iter()
        .find_map(|(v, op)| {
            if let GraphOp::GetProperty { name, .. } = op
                && *name == 3
            {
                Some(*v)
            } else {
                None
            }
        })
        .expect("GetProperty length operation exists");
    assert_eq!(intrinsics.get(&sub_len_vid), Some(&Proven::Int));
}

#[test]
fn integer_remainder_is_an_int_only_for_a_non_negative_dividend() {
    // function (x) { const a = (x | 0) % 2; const b = (x >>> 0) % 2; return a; }
    //
    // `-4 % 2` is `-0`, which an `int64_t` twin cannot hold (issue 20): `a`'s dividend may be
    // negative, so `a` is a number but not an int; `b`'s cannot, so `b` stays an int.
    let proto = FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        declared_length: 1,
        name: Some("remainders".into()),
        parameter_count: 1,
        local_count: 3,
        code: vec![
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::BitwiseOr),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Remainder),
            Instruction::StoreLocal(1),
            Instruction::LoadLocal(0),
            Instruction::Const(0),
            Instruction::Binary(BinaryOp::UnsignedShiftRight),
            Instruction::Const(1),
            Instruction::Binary(BinaryOp::Remainder),
            Instruction::StoreLocal(2),
            Instruction::LoadLocal(1),
            Instruction::LoadLocal(2),
            Instruction::Binary(BinaryOp::Add),
            Instruction::Return,
        ],
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
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::Number(0.0f64.to_bits()),
            Constant::Number(2.0f64.to_bits()),
        ],
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![proto.clone()],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    let graph = build_function_graph(&proto, &program).unwrap();
    verify_function_graph(&graph).unwrap();
    // `x` a number, as a typed entry's parameter is.
    let assumptions =
        std::collections::HashMap::from([(graph.initial_locals[0], crate::graph::Proven::Number)]);
    let numbers = crate::graph::proven_numbers_with_assumptions(&graph, &assumptions);
    let ints = crate::graph::proven_ints_with_assumptions(&graph, &assumptions);
    let defs: std::collections::HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let remainders: Vec<(ValueId, BinaryOp)> = defs
        .iter()
        .filter_map(|(vid, op)| match op {
            GraphOp::Binary {
                op: BinaryOp::Remainder,
                left,
                ..
            } => match defs.get(left) {
                Some(GraphOp::Binary { op, .. }) => Some((*vid, *op)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(remainders.len(), 2, "{remainders:?}");
    for (vid, dividend) in remainders {
        assert!(numbers.contains(&vid));
        match dividend {
            BinaryOp::BitwiseOr => assert!(!ints.contains(&vid), "(x | 0) % 2 may be -0"),
            BinaryOp::UnsignedShiftRight => assert!(ints.contains(&vid), "(x >>> 0) % 2 is >= 0"),
            other => panic!("unexpected dividend {other:?}"),
        }
    }
}

#[test]
fn large_generic_units_share_planning_tables_and_publish_updates() {
    use std::sync::Arc;
    let mut program = program(
        vec![Constant::Undefined],
        vec![Instruction::Const(0), Instruction::Return],
        0,
    );
    program.functions = vec![entry_proto(&program); 1024];
    let (_, _, _, mut plans) = plan_unit(
        &program,
        "large npm unit",
        true,
        false,
        true,
        false,
        None,
        None,
    )
    .expect("a generic unit should plan without specialization");
    for plan in &plans.functions {
        assert!(Arc::ptr_eq(&plan.typed_returns, &plans.entry.typed_returns));
        assert!(Arc::ptr_eq(&plan.typed_needs, &plans.entry.typed_needs));
        assert!(Arc::ptr_eq(
            &plan.publishes_frame,
            &plans.entry.publishes_frame
        ));
    }
    // Updating one unit-wide fact publishes a new immutable table to every body, without
    // changing plans retained by a speculative clone or another reader.
    let previous = plans.entry.fast.clone();
    let mut fast = vec![false; program.functions.len()];
    fast[17] = true;
    plans.set_fast(&fast);
    assert!(!previous[17]);
    assert!(plans.entry.fast[17]);
    assert!(
        plans
            .functions
            .iter()
            .all(|p| Arc::ptr_eq(&p.fast, &plans.entry.fast))
    );
    plans.entry.typed_entries = Arc::new(vec![true; program.functions.len()]);
    let mut available = vec![true; program.functions.len()];
    available[17] = false;
    plans.set_available(&available);
    assert!(!plans.entry.typed_entries[17]);
    assert!(plans.entry.typed_entries[18]);
    assert!(
        plans
            .functions
            .iter()
            .all(|p| Arc::ptr_eq(&p.typed_entries, &plans.entry.typed_entries))
    );
}
