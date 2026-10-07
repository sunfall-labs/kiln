use super::*;

#[test]
fn arguments_length_graph_fact_is_shared_by_frame_and_generic_tier_planning() {
    let source = r#"
function argcLoop(limit) {
  let index = 0;
  let total = 0;
  while (index < limit) {
    total += arguments.length;
    index++;
  }
  return total;
}
function simpleLength(parameter) { return arguments.length; }
function callsSimpleLengthWithExtraActuals() { return simpleLength(1, 2, 3); }
function aliasAndMutate() {
  const alias = arguments;
  alias.length = 1;
  return arguments.length;
}
function capturesArguments() {
  return () => arguments.length;
}
function capturesObjectAfterLength() {
  const before = arguments.length;
  return () => arguments[0];
}
function evaluatedArguments() {
  const beforeEval = arguments.length;
  return [beforeEval, eval('arguments.length')];
}
function* generatorArguments() { yield arguments.length; }
async function asyncArguments() { return arguments.length; }
function newTargetArguments() { return [arguments.length, new.target]; }
"#;
    let hir = kiln_frontend_oxc::parse("fixture/arguments-length-facts.ts", source);
    let program = kiln_ir::compile_module(&hir)
        .expect("arguments-length fixture compiles")
        .program;
    let function_index = |name: &str| {
        program
            .functions
            .iter()
            .position(|proto| {
                proto
                    .effective_name_units()
                    .is_some_and(|units| units.to_lossy_string() == name)
            })
            .unwrap_or_else(|| panic!("missing function {name}"))
    };
    let graphs = program
        .functions
        .iter()
        .map(|proto| crate::graph::build_function_graph(proto, &program).expect("build graph"))
        .collect::<Vec<_>>();
    let arguments_analysis = kiln_graph::ArgumentsAnalysis::new(&program);
    let facts = graphs
        .iter()
        .enumerate()
        .map(|(function, graph)| arguments_analysis.argument_length_fact(function, graph))
        .collect::<Vec<_>>();
    let publishes = crate::compute_publishes_frame_with_argument_facts(&program, &facts);

    let argc_loop = function_index("argcLoop");
    assert!(facts[argc_loop].reads_elided);
    assert!(!facts[argc_loop].values.is_empty());
    assert!(
        crate::compute_publishes_frame(&program)[argc_loop],
        "the conservative graphless answer still treats LoadArguments as observable"
    );
    assert!(
        !publishes[argc_loop],
        "an argc-only arguments read must not publish the observable arguments frame"
    );

    for name in [
        "aliasAndMutate",
        "capturesObjectAfterLength",
        "evaluatedArguments",
    ] {
        let index = function_index(name);
        assert!(
            !facts[index].reads_elided,
            "{name} observes or can mutate the arguments object"
        );
        assert!(publishes[index], "{name} requires its arguments frame");
    }
    let parent_capture = function_index("capturesArguments");
    assert!(facts[parent_capture].reads_elided);
    assert!(facts[parent_capture].values.is_empty());
    assert!(
        publishes[parent_capture],
        "a parent with no own LoadArguments still publishes its frame when a child arrow captures arguments"
    );
    for name in ["generatorArguments", "asyncArguments"] {
        let index = function_index(name);
        assert!(!facts[index].reads_elided, "{name} remains conservative");
        assert!(publishes[index], "{name} retains its resumable frame");
    }
    let new_target = function_index("newTargetArguments");
    assert!(facts[new_target].reads_elided);
    assert!(
        publishes[new_target],
        "eliding arguments.length must not elide an independent new.target frame use"
    );

    let entry_proto = crate::entry_proto(&program);
    let entry_graph =
        crate::graph::build_function_graph(&entry_proto, &program).expect("build entry graph");
    let generic = crate::direct::DirectPlan::plan(
        &program,
        &graphs,
        &entry_graph,
        &publishes,
        true,
        false,
        true,
        false,
        None,
    );
    assert!(
        generic.functions[argc_loop].generic_function_optimized,
        "the shared nonpublishing fact leaves the looping boxed body eligible for O1"
    );
    assert_eq!(
        generic.functions[argc_loop].publishes_frame[argc_loop], publishes[argc_loop],
        "the call plan carries the same per-callee publication fact"
    );

    let length_only = facts
        .iter()
        .map(|fact| !fact.values.is_empty())
        .collect::<Vec<_>>();
    let specialized = crate::direct::DirectPlan::plan_with_argument_length_facts(
        &program,
        &graphs,
        &entry_graph,
        &publishes,
        true,
        false,
        false,
        false,
        None,
        None,
        &length_only,
    );
    assert!(
        specialized.typed[argc_loop].is_none(),
        "a typed ABI without argc cannot receive a body that lowers arguments.length"
    );
    assert!(
        crate::direct::direct_callable(&program.functions[argc_loop], publishes[argc_loop]),
        "the frame-free boxed/direct-call path remains eligible"
    );
    let simple_length = function_index("simpleLength");
    assert!(facts[simple_length].reads_elided);
    assert!(
        specialized.typed[simple_length].is_none(),
        "a small length-only body still needs the boxed actual-argc ABI"
    );
    assert!(crate::direct::direct_callable(
        &program.functions[simple_length],
        publishes[simple_length]
    ));
    let wrong_arity_caller = function_index("callsSimpleLengthWithExtraActuals");
    assert!(
        specialized.functions[wrong_arity_caller]
            .known
            .values()
            .any(|&target| target == simple_length as u32)
    );
}

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
    assert_eq!(selected, [false, false, true, false, true]);
    assert_eq!(
        selected,
        crate::direct::select_generic_optimization_functions(&prog, &publishes, None),
        "loop-work ranking and tie breaks are deterministic"
    );

    let mut available = [true, true, false, true, true];
    let retained =
        crate::direct::select_generic_optimization_functions(&prog, &publishes, Some(&available));
    assert_eq!(retained, [false, false, false, true, true]);
    available[1] = false;
    available[3] = false;
    assert_eq!(
        crate::direct::select_generic_optimization_functions(&prog, &publishes, Some(&available),),
        [false, false, false, false, true],
        "unavailable functions never spend the loop tier budget"
    );
}

#[test]
fn generic_optimization_selects_large_loop_bodies_under_the_existing_loop_budget() {
    let loop_body = |length: usize| {
        let mut code = vec![Instruction::Return; length];
        code[length - 1] = Instruction::Jump(0);
        function(code)
    };
    let mut small_loop = loop_body(47);
    small_loop.generator = true;
    let mut prog = program(Vec::new(), vec![Instruction::Return], 0);
    // The source-ordered tier cannot take the 873- and 1,028-instruction bodies. They fit
    // together under the loop tier's 2,048-instruction cap and outrank the shorter loop.
    prog.functions = vec![
        loop_body(1028),
        loop_body(873),
        loop_body(1537),
        loop_body(306),
        loop_body(594),
        small_loop,
    ];
    let selected = crate::direct::select_generic_optimization_functions(&prog, &[false; 6], None);
    assert_eq!(selected, [true, true, false, true, false, false]);
    assert_eq!(
        selected,
        crate::direct::select_generic_optimization_functions(&prog, &[false; 6], None),
        "loop-work selection remains deterministic"
    );
}

#[test]
fn generic_loop_callee_weight_ranks_bounded_repeated_work_deterministically() {
    let loop_body = |length: usize| {
        let mut code = vec![Instruction::Return; length];
        code[length - 1] = Instruction::Jump(0);
        function(code)
    };
    let mut prog = program(Vec::new(), vec![Instruction::Return], 0);
    prog.functions = vec![
        loop_body(400),
        function(vec![Instruction::Return; 500]),
        loop_body(600),
    ];
    let weights = [1, 10, 5];
    let selected = crate::direct::select_generic_optimization_functions_with_weights(
        &prog,
        &[false; 3],
        None,
        Some(&weights),
    );
    assert_eq!(selected, [false, true, true]);
    assert_eq!(
        selected,
        crate::direct::select_generic_optimization_functions_with_weights(
            &prog,
            &[false; 3],
            None,
            Some(&weights),
        ),
        "weighted selection and proto-index tie breaks are deterministic"
    );
    assert_eq!(
        crate::direct::select_generic_optimization_functions_with_weights(
            &prog,
            &[false; 3],
            Some(&[true, false, true]),
            Some(&weights),
        ),
        [true, false, true],
        "an unavailable weighted candidate cannot spend the bounded tier"
    );
}

#[test]
fn generic_loop_callee_groups_keep_a_hot_call_chain_within_the_budget() {
    let source = r#"
function leaf(value: number): number { return value + 1; }
function wrapper(value: number): number { return run(value); }
function run(count: number): number {
  let total = 0;
  for (let index = 0; index < count; index++) total = wrapper(total);
  return leaf(total);
}
export { run };
"#;
    let hir = kiln_frontend_oxc::parse("fixture/loop-callee-group.ts", source);
    let program = kiln_ir::compile_module(&hir)
        .expect("loop-callee fixture compiles")
        .program;
    let find = |name: &str| {
        program
            .functions
            .iter()
            .position(|proto| {
                proto
                    .effective_name_units()
                    .is_some_and(|value| value.to_lossy_string() == name)
            })
            .unwrap_or_else(|| panic!("missing proto {name}"))
    };
    let leaf = find("leaf");
    let wrapper = find("wrapper");
    let run = find("run");
    let profile = crate::direct::generic_loop_profile_for_program(&program)
        .expect("small fixture fits profile bounds");
    let selected = crate::direct::select_generic_optimization_functions_with_profile(
        &program,
        &vec![true; program.functions.len()],
        None,
        &profile,
    );
    assert!(selected[wrapper], "loop-reached wrapper is selected");
    assert!(selected[leaf], "known leaf stays grouped with its wrapper");
    assert!(
        selected[run],
        "the repeated loop driver is selected with its known callee chain"
    );
    assert!(
        selected[leaf],
        "the known-callee walk crosses wrapper -> loop driver -> leaf"
    );

    let unavailable = vec![false; program.functions.len()];
    let zero_weights = vec![0; program.functions.len()];
    let too_few_members = profile.loop_group(
        &program,
        Some(run),
        wrapper,
        None,
        &unavailable,
        &zero_weights,
        1,
        1_536,
        4_096,
    );
    assert_eq!(
        too_few_members,
        vec![wrapper],
        "the caller is not admitted if it would crowd out the seed target"
    );
    let too_few_instructions = profile.loop_group(
        &program,
        Some(run),
        wrapper,
        None,
        &unavailable,
        &zero_weights,
        4,
        1_536,
        program.functions[wrapper].code.len(),
    );
    assert_eq!(
        too_few_instructions,
        vec![wrapper],
        "caller instructions are checked before group admission"
    );

    let unavailable_leaf = crate::direct::select_generic_optimization_functions_with_profile(
        &program,
        &vec![true; program.functions.len()],
        Some(
            &program
                .functions
                .iter()
                .enumerate()
                .map(|(index, _)| index != leaf)
                .collect::<Vec<_>>(),
        ),
        &profile,
    );
    assert!(!unavailable_leaf[leaf], "unavailable callees stay excluded");
    assert!(
        unavailable_leaf[wrapper],
        "the available wrapper remains eligible"
    );

    let caller_unavailable = crate::direct::select_generic_optimization_functions_with_profile(
        &program,
        &vec![true; program.functions.len()],
        Some(
            &program
                .functions
                .iter()
                .enumerate()
                .map(|(index, _)| index != run)
                .collect::<Vec<_>>(),
        ),
        &profile,
    );
    assert!(!caller_unavailable[run]);
    assert!(
        !caller_unavailable[wrapper] && !caller_unavailable[leaf],
        "a callee group is not selected when its only loop driver is unavailable"
    );
}

#[test]
fn generic_loop_callee_groups_cover_distinct_callers_before_extra_edges() {
    let large_leaf = |name: &str| {
        let mut body = format!("function {name}(value: number): number {{\n");
        for _ in 0..180 {
            body.push_str("value = value + 1;\n");
        }
        body.push_str("return value;\n}\n");
        body
    };
    let mut source = String::new();
    for name in ["aLeaf1", "aLeaf2", "aLeaf3", "bLeaf"] {
        source.push_str(&large_leaf(name));
    }
    source.push_str(
        "function a1(value: number): number { return aLeaf1(value); }\n\
         function a2(value: number): number { return aLeaf2(value); }\n\
         function a3(value: number): number { return aLeaf3(value); }\n\
         function b1(value: number): number { return bLeaf(value); }\n\
         function runA(count: number): number { let x = 0; for (let i = 0; i < count; i++) { x = a1(x); x = a2(x); x = a3(x); } return x; }\n\
         function runB(count: number): number { let x = 0; for (let i = 0; i < count; i++) x = b1(x); return x; }\n\
         export { runA, runB };\n",
    );
    let hir = kiln_frontend_oxc::parse("fixture/loop-caller-diversity.ts", &source);
    let program = kiln_ir::compile_module(&hir)
        .expect("caller-diversity fixture compiles")
        .program;
    let find = |name: &str| {
        program
            .functions
            .iter()
            .position(|proto| {
                proto
                    .effective_name_units()
                    .is_some_and(|value| value.to_lossy_string() == name)
            })
            .unwrap_or_else(|| panic!("missing proto {name}"))
    };
    let profile = crate::direct::generic_loop_profile_for_program(&program)
        .expect("fixture fits the bounded call profile");
    let selected = crate::direct::select_generic_optimization_functions_with_profile(
        &program,
        &vec![true; program.functions.len()],
        None,
        &profile,
    );
    let run_a = find("runA");
    let run_b = find("runB");
    let b1 = find("b1");
    let b_leaf = find("bLeaf");
    assert!(selected[run_a]);
    assert!(selected[run_b] && selected[b1] && selected[b_leaf]);
    assert!(
        selected.iter().filter(|selected| **selected).count() > 8,
        "the loop tier is no longer silently limited to eight supplemental bodies"
    );
    assert!(
        selected
            .iter()
            .enumerate()
            .filter(|(_, selected)| **selected)
            .map(|(index, _)| program.functions[index].code.len())
            .sum::<usize>()
            <= 32_768,
        "selected loop bodies remain within the profile instruction envelope"
    );
}

#[test]
fn older_instantiation_metadata_defaults_to_the_generic_o0_tier() {
    let older =
        r#"{"symbol":"fixture_instantiate","prepare_symbol":"fixture_prepare","source":""}"#;
    let unit: crate::CModuleUnit =
        serde_json::from_str(older).expect("older cached unit metadata remains readable");
    assert!(!unit.generic_mixed_optimization);
}

fn diagnostic_loop_work(proto: &kiln_ir::FunctionProto) -> Option<(usize, usize)> {
    let mut span = 0usize;
    let mut count = 0usize;
    for (pc, instruction) in proto.code.iter().enumerate() {
        let target = match instruction {
            Instruction::Jump(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::JumpIfReturnSignal(target) => *target as usize,
            _ => continue,
        };
        if target <= pc {
            span = span.saturating_add(pc.saturating_sub(target).saturating_add(1));
            count = count.saturating_add(1);
        }
    }
    (count > 0).then_some((span, count))
}

/// Read-only diagnostic for the pinned SchemaBinary source. It prints the selection and
/// backedge ranking for the per-element encode/decode functions in the timeout's loop stack.
#[test]
#[ignore = "one-off pinned SchemaBinary generic O1 selection diagnostic"]
fn diagnose_pinned_schema_binary_generic_optimization_selection() {
    let root = std::path::PathBuf::from(
        std::env::var_os("KILN_EFFECT_SOURCE_ROOT")
            .expect("set KILN_EFFECT_SOURCE_ROOT to the pinned Effect source root"),
    );
    let path = root.join("packages/effect/src/encoding/SchemaBinary.ts");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let hir = kiln_frontend_oxc::parse(&path.to_string_lossy(), &source);
    let module = kiln_ir::compile_module(&hir)
        .unwrap_or_else(|error| panic!("compile {}: {error:?}", path.display()));
    let program = module.program.clone();
    let graphs = program
        .functions
        .iter()
        .map(|proto| crate::graph::build_function_graph(proto, &program).expect("build graph"))
        .collect::<Vec<_>>();
    let entry_proto = crate::entry_proto(&program);
    let entry_graph =
        crate::graph::build_function_graph(&entry_proto, &program).expect("build entry graph");
    let publishes = crate::compute_publishes_frame(&program);
    let known_plan = crate::direct::DirectPlan::plan(
        &program,
        &graphs,
        &entry_graph,
        &publishes,
        true,
        false,
        false,
        false,
        None,
    );
    let generic_plan = crate::direct::DirectPlan::plan(
        &program,
        &graphs,
        &entry_graph,
        &publishes,
        true,
        false,
        true,
        false,
        None,
    );
    let selected = generic_plan
        .functions
        .iter()
        .map(|plan| plan.generic_function_optimized)
        .collect::<Vec<_>>();
    // The native ESM path emits a distinct instantiation entry whose bytecode ends before
    // evaluation-time stores. Re-run the exact planner on that prefix while retaining the same
    // function table to see whether those stores are what establish loop-callee ranking facts.
    let mut instantiation_program = program.clone();
    instantiation_program
        .code
        .truncate(module.link_facts.module_instantiation_end as usize);
    instantiation_program
        .code
        .push(kiln_ir::Instruction::Return);
    let instantiation_graphs = instantiation_program
        .functions
        .iter()
        .map(|proto| {
            crate::graph::build_function_graph(proto, &instantiation_program)
                .expect("build instantiation function graph")
        })
        .collect::<Vec<_>>();
    let instantiation_entry_proto = crate::entry_proto(&instantiation_program);
    let instantiation_entry_graph =
        crate::graph::build_function_graph(&instantiation_entry_proto, &instantiation_program)
            .expect("build instantiation entry graph");
    let instantiation_publishes = crate::compute_publishes_frame(&instantiation_program);
    let instantiation_plan = crate::direct::DirectPlan::plan(
        &instantiation_program,
        &instantiation_graphs,
        &instantiation_entry_graph,
        &instantiation_publishes,
        true,
        false,
        true,
        false,
        None,
    );
    let instantiation_selected = instantiation_plan
        .functions
        .iter()
        .map(|plan| plan.generic_function_optimized)
        .collect::<Vec<_>>();
    let instantiation_weights = instantiation_plan.generic_loop_profile_weights();
    let shared_profile = crate::direct::generic_loop_profile_for_program(&program);
    let mut shared_profile_plan = crate::direct::DirectPlan::plan_with_loop_profile(
        &instantiation_program,
        &instantiation_graphs,
        &instantiation_entry_graph,
        &instantiation_publishes,
        true,
        false,
        true,
        false,
        None,
        shared_profile.clone(),
    );
    shared_profile_plan.set_available(
        &instantiation_program,
        &vec![true; instantiation_program.functions.len()],
    );
    let shared_profile_selected = shared_profile_plan
        .functions
        .iter()
        .map(|plan| plan.generic_function_optimized)
        .collect::<Vec<_>>();
    let shared_profile_weights = shared_profile_plan.generic_loop_profile_weights();
    let decode_array_group = shared_profile.as_ref().map(|profile| {
        profile.loop_group(
            &program,
            Some(193),
            199,
            None,
            &vec![false; program.functions.len()],
            &shared_profile_weights.clone().unwrap_or_default(),
            4,
            1_536,
            4_096,
        )
    });
    let targets = [
        (156, "encodeArray"),
        (165, "encodeValue"),
        (193, "decodeArray"),
        (200, "decodeValueUnchecked"),
    ];
    let loop_weights = generic_plan.generic_loop_profile_weights();
    let rows = targets
        .into_iter()
        .map(|(index, expected_name)| {
            let proto = &program.functions[index];
            let name = proto
                .effective_name_units()
                .map(|name| name.to_lossy_string());
            let loop_work = diagnostic_loop_work(proto);
            serde_json::json!({
                "proto": index,
                "expected_name": expected_name,
                "name": name,
                "instructions": proto.code.len(),
                "publishes_frame": publishes.get(index).copied().unwrap_or(true),
                "loop_work": loop_work,
                "loop_call_weight": loop_weights
                    .as_ref()
                    .and_then(|weights| weights.get(index))
                    .copied(),
                "selected": selected[index],
                "instantiation_loop_call_weight": instantiation_weights
                    .as_ref()
                    .and_then(|weights| weights.get(index))
                    .copied(),
                "instantiation_selected": instantiation_selected[index],
                "shared_profile_loop_call_weight": shared_profile_weights
                    .as_ref()
                    .and_then(|weights| weights.get(index))
                    .copied(),
                "shared_profile_selected": shared_profile_selected[index],
            })
        })
        .collect::<Vec<_>>();
    let weighted_candidates = loop_weights.as_ref().map(|weights| {
        let mut rows = (0..program.functions.len())
            .filter_map(|index| {
                let weight = weights[index];
                (weight > 0).then(|| {
                    serde_json::json!({
                        "proto": index,
                        "name": program.functions[index].effective_name_units()
                            .map(|name| name.to_lossy_string()),
                        "instructions": program.functions[index].code.len(),
                        "weight": weight,
                        "selected": selected[index],
                    })
                })
            })
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| std::cmp::Reverse(row["weight"].as_u64().unwrap_or_default()));
        rows.truncate(24);
        rows
    });
    let hot_loop_calls = [156usize, 193]
        .into_iter()
        .map(|caller| {
            let graph = &graphs[caller];
            let definitions = graph
                .blocks
                .iter()
                .flat_map(|block| block.operations.iter())
                .map(|(value, op)| (*value, op))
                .collect::<std::collections::HashMap<_, _>>();
            let calls = graph
                .blocks
                .iter()
                .flat_map(|block| block.operations.iter())
                .filter_map(|(result, op)| {
                    let crate::graph::GraphOp::Call { callee, .. } = op else {
                        return None;
                    };
                    Some(serde_json::json!({
                        "pc": graph.origins.get(result).map(|origin| origin.pc),
                        "callee_value": callee.0,
                        "callee_definition": definitions.get(callee).map(|op| format!("{op:?}")),
                        "known_target": known_plan.functions[caller].known.get(callee),
                        "instantiation_known_target":
                            instantiation_plan.functions[caller].known.get(callee),
                    }))
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "caller": caller,
                "name": program.functions[caller]
                    .effective_name_units()
                    .map(|name| name.to_lossy_string()),
                "calls": calls,
            })
        })
        .collect::<Vec<_>>();
    let selected_protos = selected
        .iter()
        .enumerate()
        .filter_map(|(index, selected)| selected.then_some(index))
        .collect::<Vec<_>>();
    let selected_names = selected_protos
        .iter()
        .map(|&index| {
            serde_json::json!({
                "proto": index,
                "name": program.functions[index]
                    .effective_name_units()
                    .map(|name| name.to_lossy_string()),
                "instructions": program.functions[index].code.len(),
                "loop_work": diagnostic_loop_work(&program.functions[index]),
            })
        })
        .collect::<Vec<_>>();
    let mut loop_candidates = program
        .functions
        .iter()
        .enumerate()
        .filter_map(|(index, proto)| {
            if !(128..=1536).contains(&proto.code.len()) {
                return None;
            }
            diagnostic_loop_work(proto).map(|(span, backedges)| {
                serde_json::json!({
                    "proto": index,
                    "name": proto.effective_name_units().map(|name| name.to_lossy_string()),
                    "instructions": proto.code.len(),
                    "span": span,
                    "backedges": backedges,
                    "loop_call_weight": loop_weights
                        .as_ref()
                        .and_then(|weights| weights.get(index))
                        .copied()
                        .unwrap_or_default(),
                    "selected": selected[index],
                })
            })
        })
        .collect::<Vec<_>>();
    loop_candidates.sort_by(|left, right| {
        let left_span = left["span"].as_u64().unwrap_or(0);
        let right_span = right["span"].as_u64().unwrap_or(0);
        let left_backedges = left["backedges"].as_u64().unwrap_or(0);
        let right_backedges = right["backedges"].as_u64().unwrap_or(0);
        let left_instructions = left["instructions"].as_u64().unwrap_or(1);
        let right_instructions = right["instructions"].as_u64().unwrap_or(1);
        (right_backedges * left_instructions)
            .cmp(&(left_backedges * right_instructions))
            .then_with(|| {
                right_span
                    .cmp(&left_span)
                    .then_with(|| right_backedges.cmp(&left_backedges))
            })
    });
    println!(
        "{}",
        serde_json::json!({
            "source": "packages/effect/src/encoding/SchemaBinary.ts",
            "source_bytes": source.len(),
            "proto_count": program.functions.len(),
            "body_instructions": program.functions.iter().map(|proto| proto.code.len()).sum::<usize>(),
            "entry_instructions": program.code.len(),
            "selected_protos": selected_protos,
            "module_instantiation_end": module.link_facts.module_instantiation_end,
            "instantiation_entry_instructions": instantiation_program.code.len(),
            "instantiation_selected_protos": instantiation_selected.iter().enumerate()
                .filter_map(|(index, selected)| selected.then_some(index)).collect::<Vec<_>>(),
            "shared_profile_selected_protos": shared_profile_selected.iter().enumerate()
                .filter_map(|(index, selected)| selected.then_some(index)).collect::<Vec<_>>(),
            "decode_array_group": decode_array_group,
            "selected_names": selected_names,
        "selected_target_rows": rows,
        "loop_profile_present": loop_weights.is_some(),
        "weighted_candidates": weighted_candidates,
            "hot_loop_calls": hot_loop_calls,
            "top_loop_candidates": loop_candidates.into_iter().take(32).collect::<Vec<_>>(),
        })
    );
    for (index, expected_name) in targets {
        assert_eq!(
            program.functions[index]
                .effective_name_units()
                .map(|name| name.to_lossy_string())
                .as_deref(),
            Some(expected_name),
            "the pinned function IDs must still resolve to the profiled source functions"
        );
    }
    let weights = generic_plan
        .generic_loop_profile_weights()
        .expect("Schema graph fits the explicit bounded loop-call profile caps");
    assert!(
        weights[200] > 0,
        "the in-loop fn193 -> fn199 -> fn200 chain is credited"
    );
    assert!(
        selected[165],
        "the encode callee remains selected under its loop weight"
    );
    assert!(
        !instantiation_selected[165],
        "the declaration-only entry loses the evaluation stores used by local ranking"
    );
    assert!(
        shared_profile_selected[165],
        "the full-module ranking profile restores the bounded decision without changing calls"
    );
    assert!(
        shared_profile_selected[199],
        "decodeValue is reached from decodeArray"
    );
    assert!(
        shared_profile_selected[200],
        "decodeValueUnchecked remains grouped with its hot wrapper"
    );
    assert!(
        shared_profile_selected[193],
        "the repeated decodeArray loop driver is selected with its callee chain"
    );

    // Exercise the same split-module emitter used by native cyclic-module AOT, not only the
    // planner helper above. The instantiation C unit contains the hoisted callback body, while
    // the loop and immutable binding assignment that establish its ranking are in evaluation.
    let emit_started = std::time::Instant::now();
    let emitted = crate::emit_module_with_logical_path_and_policy_and_instantiation(
        &module,
        "packages/effect/src/encoding/SchemaBinary.ts",
        crate::EmissionPolicy {
            generic_only: true,
            ..Default::default()
        },
        true,
    )
    .expect("the pinned SchemaBinary cycle module emits");
    let instantiation = emitted
        .instantiation
        .expect("the cycle module has a separate instantiation unit");
    println!(
        "schema_emission_stats {}",
        serde_json::json!({
            "evaluation_c_bytes": emitted.source.len(),
            "instantiation_c_bytes": instantiation.source.len(),
            "emission_elapsed_ms": emit_started.elapsed().as_millis(),
            "selected_function_count": shared_profile_selected.iter().filter(|selected| **selected).count(),
            "selected_function_instructions": shared_profile_selected.iter().enumerate()
                .filter(|(_, selected)| **selected)
                .map(|(index, _)| program.functions[index].code.len())
                .sum::<usize>(),
            "tier_profile_function_limit": 512,
            "tier_profile_instruction_limit": 32_768,
        })
    );
    assert!(
        instantiation.generic_mixed_optimization,
        "the instantiation unit carries its own mixed-tier compile requirement"
    );
    let marker = "/* frontend function 165:";
    let marker_at = instantiation
        .source
        .find(marker)
        .expect("the pinned encodeValue function is retained in instantiation");
    let header_end = instantiation.source[marker_at..]
        .find("static inline int")
        .map(|offset| marker_at + offset)
        .expect("encodeValue has an emitted C function");
    let function_header = &instantiation.source[marker_at..header_end];
    assert!(
        !function_header.contains("optnone")
            && !function_header.contains("optimize(\"O0\")")
            && (function_header.contains("optimize(\"O1\")")
                || !function_header.contains("KILN_GENERIC_MIXED_TIER")),
        "actual SchemaBinary instantiation emits encodeValue in the bounded O1 tier: {function_header}"
    );
    for index in [199usize, 200] {
        let marker = format!("/* frontend function {index}:");
        let marker_at = instantiation
            .source
            .find(&marker)
            .unwrap_or_else(|| panic!("instantiation retains hot decode proto {index}"));
        let header_end = instantiation.source[marker_at..]
            .find("static inline int")
            .map(|offset| marker_at + offset)
            .unwrap_or_else(|| panic!("decode proto {index} has an emitted C function"));
        let function_header = &instantiation.source[marker_at..header_end];
        assert!(
            !function_header.contains("optnone") && !function_header.contains("optimize(\"O0\")"),
            "actual SchemaBinary instantiation emits hot decode proto {index} in O1: {function_header}"
        );
    }
    let driver_marker = "/* frontend function 193:";
    let driver_marker_at = instantiation
        .source
        .find(driver_marker)
        .expect("the repeated decodeArray loop driver is retained");
    let driver_header_end = instantiation.source[driver_marker_at..]
        .find("static inline int")
        .map(|offset| driver_marker_at + offset)
        .expect("decodeArray has an emitted C function");
    let driver_header = &instantiation.source[driver_marker_at..driver_header_end];
    assert!(
        !driver_header.contains("optnone") && !driver_header.contains("optimize(\"O0\")"),
        "actual SchemaBinary instantiation emits decodeArray in the bounded O1 tier: {driver_header}"
    );
}

#[test]
fn cyclic_instantiation_reuses_full_module_loop_ranking_only() {
    let mut source = String::new();
    for index in 0..70 {
        source.push_str(&format!("function filler{index}() {{ return {index}; }}\n"));
    }
    source.push_str(
        "function run(count: number): number { let sum = 0; for (let i = 0; i < count; i++) sum += invoke(i); return sum; }\nconst invoke = (value: number): number => value + 1;\nexport { run };\n",
    );
    let hir = kiln_frontend_oxc::parse("fixture/cycle-loop-ranking.ts", &source);
    let module = kiln_ir::compile_module(&hir).expect("cycle ranking fixture compiles");
    let find_proto = |name: &str| {
        module
            .program
            .functions
            .iter()
            .position(|proto| {
                proto
                    .effective_name_units()
                    .is_some_and(|value| value.to_lossy_string() == name)
            })
            .unwrap_or_else(|| panic!("missing function proto {name}"))
    };
    let invoke = find_proto("invoke");
    let profile = crate::direct::generic_loop_profile_for_program(&module.program)
        .expect("small module receives bounded loop-ranking facts");
    assert!(profile.weights(None)[invoke] > 0);

    // The production emitter reruns selection after module liveness is known. That pass must
    // preserve the loop-callee grouping rather than reduce it to independent per-proto weights.
    let mut instantiation_program = module.program.clone();
    instantiation_program
        .code
        .truncate(module.link_facts.module_instantiation_end as usize);
    instantiation_program
        .code
        .push(kiln_ir::Instruction::Return);
    let instantiation_graphs = instantiation_program
        .functions
        .iter()
        .map(|proto| {
            crate::graph::build_function_graph(proto, &instantiation_program)
                .expect("build instantiation graph")
        })
        .collect::<Vec<_>>();
    let instantiation_entry = crate::entry_proto(&instantiation_program);
    let instantiation_entry_graph =
        crate::graph::build_function_graph(&instantiation_entry, &instantiation_program)
            .expect("build instantiation entry graph");
    let instantiation_publishes = crate::compute_publishes_frame(&instantiation_program);
    let mut available_plan = crate::direct::DirectPlan::plan_with_loop_profile(
        &instantiation_program,
        &instantiation_graphs,
        &instantiation_entry_graph,
        &instantiation_publishes,
        true,
        false,
        true,
        false,
        None,
        Some(profile.clone()),
    );
    available_plan.set_available(
        &instantiation_program,
        &vec![true; instantiation_program.functions.len()],
    );
    assert!(
        available_plan.functions[invoke].generic_function_optimized,
        "liveness recomputation must retain the bounded loop-callee group"
    );
    let mut unavailable = vec![true; instantiation_program.functions.len()];
    unavailable[invoke] = false;
    let mut unavailable_plan = crate::direct::DirectPlan::plan_with_loop_profile(
        &instantiation_program,
        &instantiation_graphs,
        &instantiation_entry_graph,
        &instantiation_publishes,
        true,
        false,
        true,
        false,
        None,
        Some(profile.clone()),
    );
    unavailable_plan.set_available(&instantiation_program, &unavailable);
    assert!(
        !unavailable_plan.functions[invoke].generic_function_optimized,
        "an unavailable member of a ranked callee group cannot be selected"
    );

    let emitted = crate::emit_module_with_logical_path_and_policy_and_instantiation(
        &module,
        "fixture/cycle-loop-ranking.ts",
        crate::EmissionPolicy {
            compact: false,
            generic_only: true,
            ..Default::default()
        },
        true,
    )
    .expect("cycle module emits");
    let instantiation = emitted.instantiation.expect("cycle has instantiate unit");
    let marker = format!("/* frontend function {invoke}:");
    let marker_at = instantiation
        .source
        .find(&marker)
        .expect("instantiation emits the callback body");
    let header_end = instantiation.source[marker_at..]
        .find("static inline int")
        .map(|offset| marker_at + offset)
        .expect("callback has emitted body signature");
    let function_header = &instantiation.source[marker_at..header_end];
    assert!(
        !function_header.contains("optnone") && !function_header.contains("optimize(\"O0\")"),
        "the callback's boxed body receives the shared O1 ranking decision"
    );
    assert!(
        function_header.contains("optimize(\"O1\")")
            || !function_header.contains("KILN_GENERIC_MIXED_TIER"),
        "the selected callback is not emitted with the mixed-tier O0 attribute"
    );
    assert!(
        instantiation.source.contains("kiln_c99_val_call"),
        "ranking facts must not replace phase-specific calls with direct dispatch"
    );

    let mutable = "function run(count: number): number { let sum = 0; for (let i = 0; i < count; i++) sum += invoke(i); return sum; }\nlet invoke = (value: number): number => value + 1;\ninvoke = (value: number): number => value * 2;\nexport { run };\n";
    let mutable_hir = kiln_frontend_oxc::parse("fixture/mutable-loop-ranking.ts", mutable);
    let mutable_module = kiln_ir::compile_module(&mutable_hir).expect("mutable control compiles");
    let mutable_invoke = mutable_module
        .program
        .functions
        .iter()
        .position(|proto| {
            proto
                .effective_name_units()
                .is_some_and(|value| value.to_lossy_string() == "invoke")
        })
        .expect("mutable callback proto exists");
    let mutable_profile = crate::direct::generic_loop_profile_for_program(&mutable_module.program)
        .expect("mutable control fits profile bounds");
    assert_eq!(
        mutable_profile.weights(None)[mutable_invoke],
        0,
        "reassigned bindings do not contribute speculative ranking edges"
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
