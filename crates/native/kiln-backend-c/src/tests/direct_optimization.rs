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

    let selected = crate::direct::select_generic_optimization_functions(
        &prog,
        &[false, false, false, false],
        None,
    );
    assert_eq!(selected, [true, false, false, true]);
    assert_eq!(
        selected,
        crate::direct::select_generic_optimization_functions(
            &prog,
            &[false, false, false, false],
            None
        ),
        "selection is deterministic"
    );
    assert_eq!(
        crate::direct::select_generic_optimization_functions(&prog, &[false], None),
        [true, false, false, false],
        "missing publication facts conservatively exclude later functions"
    );
}

#[test]
fn generic_optimization_budget_excludes_unavailable_declarations() {
    let mut prog = program(Vec::new(), vec![Instruction::Return], 0);
    // Both caps would be spent before reaching the live bodies if dead declarations counted.
    prog.functions = (0..64)
        .map(|_| function(vec![Instruction::Return; 320]))
        .chain((0..65).map(|_| function(vec![Instruction::Return])))
        .collect();
    let publishes = vec![false; prog.functions.len()];
    let mut available = vec![false; prog.functions.len()];
    available[64..].fill(true);
    let selected =
        crate::direct::select_generic_optimization_functions(&prog, &publishes, Some(&available));
    assert!(selected[..64].iter().all(|selected| !selected));
    assert!(selected[64..128].iter().all(|selected| *selected));
    assert!(!selected[128], "the retained function cap remains 64");
}

#[test]
fn generic_optimization_selects_loop_heavy_frame_bodies_in_a_separate_bounded_tier() {
    let unlooped_generator = {
        let mut proto = function(vec![Instruction::Return; 20]);
        proto.generator = true;
        proto
    };
    let short_loop = || {
        let mut code = vec![Instruction::Return; 20];
        code[19] = Instruction::Jump(18);
        code
    };
    let long_loop = || {
        let mut code = vec![Instruction::Return; 916];
        code[915] = Instruction::Jump(0);
        code
    };
    let too_large = || {
        let mut code = vec![Instruction::Return; 1025];
        code[1024] = Instruction::Jump(0);
        code
    };

    let frame_publisher = function(long_loop());
    let mut second_loop = function({
        let mut code = vec![Instruction::Return; 500];
        code[499] = Instruction::Jump(0);
        code
    });
    second_loop.generator = true;
    let mut oversized_loop = function(too_large());
    oversized_loop.generator = true;
    let mut prog = program(Vec::new(), vec![Instruction::Return], 0);
    prog.functions = vec![
        unlooped_generator,
        function(short_loop()),
        frame_publisher,
        second_loop,
        oversized_loop,
    ];
    // Proto 2 has a publication fact even though it is not itself a generator; this candidate is
    // higher-ranked than the two shorter loop bodies.
    let publishes = [true, true, true, true, true];

    let selected = crate::direct::select_generic_optimization_functions(&prog, &publishes, None);
    assert_eq!(selected, [false, false, true, true, false]);
    assert_eq!(
        selected,
        crate::direct::select_generic_optimization_functions(&prog, &publishes, None),
        "loop-work ranking and tie breaks are deterministic"
    );

    let mut available = [true, true, false, true, true];
    let retained =
        crate::direct::select_generic_optimization_functions(&prog, &publishes, Some(&available));
    assert_eq!(retained, [false, true, false, true, false]);
    available[1] = false;
    available[3] = false;
    assert_eq!(
        crate::direct::select_generic_optimization_functions(&prog, &publishes, Some(&available),),
        [false; 5],
        "unavailable functions never spend the loop tier budget"
    );
}

/// Read-only diagnostic for the pinned Sharding source. It compiles HIR and runs the selector,
/// without emitting C or changing the compiler selection rules.
#[test]
#[ignore = "one-off pinned Sharding generic O1 selection diagnostic"]
fn diagnose_pinned_sharding_generic_optimization_selection() {
    let root = std::path::PathBuf::from(
        std::env::var_os("KILN_EFFECT_SOURCE_ROOT")
            .expect("set KILN_EFFECT_SOURCE_ROOT to the pinned Effect source root"),
    );
    let path = root.join("packages/effect/src/cluster/Sharding.ts");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let hir = kiln_frontend_oxc::parse(&path.to_string_lossy(), &source);
    let program = kiln_ir::compile_module(&hir)
        .unwrap_or_else(|error| panic!("compile {}: {error:?}", path.display()))
        .program;
    let publishes = crate::compute_publishes_frame(&program);
    let selected = crate::direct::select_generic_optimization_functions(&program, &publishes, None);
    let rows = program
        .functions
        .iter()
        .enumerate()
        .filter(|(index, _)| selected[*index])
        .map(|(index, proto)| {
            serde_json::json!({
                "proto": index,
                "name": proto.effective_name_units().map(|name| name.to_lossy_string()),
                "instructions": proto.code.len(),
                "generator": proto.generator,
                "async": proto.asynchronous,
                "async_generator": proto.async_generator,
                "publishes_frame": publishes.get(index).copied().unwrap_or(true),
            })
        })
        .collect::<Vec<_>>();
    let target = program
        .functions
        .get(77)
        .expect("profiled Sharding proto 77 exists");
    println!(
        "{}",
        serde_json::json!({
            "source": "packages/effect/src/cluster/Sharding.ts",
            "proto_count": program.functions.len(),
            "total_instructions": program.functions.iter().map(|proto| proto.code.len()).sum::<usize>(),
            "selected": rows,
            "proto_77": {
                "name": target.effective_name_units().map(|name| name.to_lossy_string()),
                "instructions": target.code.len(),
                "generator": target.generator,
                "publishes_frame": publishes.get(77).copied().unwrap_or(true),
                "selected": selected[77],
            },
        })
    );
    assert!(
        selected[77],
        "the profiled 916-instruction Sharding loop should enter the bounded tier"
    );
}
