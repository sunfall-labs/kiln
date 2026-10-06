use super::*;
use crate::graph::build_function_graph;
use serde_json::json;

fn generic_plan(source: &str) -> (Program, DirectPlan) {
    let hir = kiln_frontend_oxc::parse("direct-plan-test.ts", source);
    let program = kiln_ir::compile_module(&hir)
        .expect("compile test module")
        .program;
    let graphs = program
        .functions
        .iter()
        .map(|proto| build_function_graph(proto, &program).expect("build function graph"))
        .collect::<Vec<_>>();
    let entry_proto = crate::entry_proto(&program);
    let entry_graph = build_function_graph(&entry_proto, &program).expect("build entry graph");
    let publishes = crate::compute_publishes_frame(&program);
    let plan = DirectPlan::plan(
        &program,
        &graphs,
        &entry_graph,
        &publishes,
        true,
        false,
        true,
        true,
        None,
    );
    (program, plan)
}

#[test]
fn bounded_generic_plan_keeps_safe_known_callees_without_typed_entries() {
    let (program, mut plan) = generic_plan(
        "function add(a, b) { return a + b; }\n\
         function invoke(callback) { return callback(1, 2); }\n\
         function run() { return add(1, 2); }\n\
         run();",
    );
    let add = program
        .functions
        .iter()
        .position(|proto| {
            proto
                .effective_name_units()
                .is_some_and(|name| name.to_lossy_string() == "add")
        })
        .expect("add proto") as u32;
    let run = program
        .functions
        .iter()
        .position(|proto| {
            proto
                .effective_name_units()
                .is_some_and(|name| name.to_lossy_string() == "run")
        })
        .expect("run proto");
    let invoke = program
        .functions
        .iter()
        .position(|proto| {
            proto
                .effective_name_units()
                .is_some_and(|name| name.to_lossy_string() == "invoke")
        })
        .expect("invoke proto");
    assert!(
        plan.functions[run]
            .known
            .values()
            .any(|&target| target == add as u32)
    );
    assert!(plan.functions.iter().all(|body| body.generic_only));
    assert!(plan.typed.iter().all(Option::is_none));
    assert!(
        plan.functions[invoke].known.is_empty(),
        "the indirect callback parameter must remain a generic call"
    );
    let mut unavailable = vec![true; program.functions.len()];
    unavailable[add as usize] = false;
    plan.set_available(&program, &unavailable);
    assert!(
        plan.functions[run]
            .known
            .values()
            .all(|&target| target != add as u32),
        "a target omitted from the emitted unit must use runtime fallback"
    );
}

#[test]
fn generic_plan_skips_known_callee_analysis_above_module_budget() {
    let mut source = String::new();
    for index in 0..MAX_GENERIC_DIRECT_ANALYSIS_FUNCTIONS + 1 {
        source.push_str(&format!("function f{index}() {{ return {index}; }}\n"));
    }
    source.push_str("f0();\n");
    let (_, plan) = generic_plan(&source);
    assert!(plan.functions.iter().all(|body| body.known.is_empty()));
    assert!(plan.entry.known.is_empty());
}

/// Read-only diagnostic for a pinned Effect source checkout. Run with
/// `KILN_EFFECT_SOURCE_ROOT=/path/to/source mbx test -p kiln-backend-c --lib
/// diagnose_effect_hash_direct_eligibility -- --ignored --nocapture --test-threads=1`.
/// This lowers and analyzes the two source modules, but does not emit or compile C.
#[test]
#[ignore = "one-off native Hash direct-call eligibility diagnostic"]
fn diagnose_effect_hash_direct_eligibility() {
    let root = std::path::PathBuf::from(
        std::env::var_os("KILN_EFFECT_SOURCE_ROOT")
            .expect("set KILN_EFFECT_SOURCE_ROOT to the pinned Effect source root"),
    );

    for relative in [
        "packages/effect/src/Equal.ts",
        "packages/effect/src/internal/hashMap.ts",
    ] {
        let path = root.join(relative);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let hir = kiln_frontend_oxc::parse(&path.to_string_lossy(), &source);
        let program = kiln_ir::compile_module(&hir)
            .unwrap_or_else(|error| panic!("compile {}: {error:?}", path.display()))
            .program;
        let graphs = program
            .functions
            .iter()
            .map(|proto| {
                build_function_graph(proto, &program)
                    .unwrap_or_else(|error| panic!("graph {}: {error}", path.display()))
            })
            .collect::<Vec<_>>();
        let entry_proto = crate::entry_proto(&program);
        let entry_graph = build_function_graph(&entry_proto, &program)
            .unwrap_or_else(|error| panic!("entry graph {}: {error}", path.display()));
        let publishes = crate::compute_publishes_frame(&program);
        let generic = DirectPlan::plan(
            &program,
            &graphs,
            &entry_graph,
            &publishes,
            true,
            false,
            true,
            true,
            None,
        );
        let analyzed = DirectPlan::plan(
            &program,
            &graphs,
            &entry_graph,
            &publishes,
            true,
            false,
            false,
            true,
            None,
        );
        let functions = program
            .functions
            .iter()
            .enumerate()
            .map(|(index, proto)| {
                let known = analyzed.functions[index]
                    .known
                    .iter()
                    .map(|(site, target)| json!({ "site_value": site.0, "target_proto": target }))
                    .collect::<Vec<_>>();
                let publishes_frame = publishes.get(index).copied().unwrap_or(true);
                json!({
                    "proto": index,
                    "name": proto.effective_name_units().map(|units| units.to_lossy_string()),
                    "instructions": proto.code.len(),
                    "arity": proto.parameter_count,
                    "method": proto.is_method,
                    "generator": proto.generator,
                    "async": proto.asynchronous,
                    "publishes_frame": publishes_frame,
                    "direct_callable": direct_callable(proto, publishes_frame),
                    "generic_known_callees": generic.functions[index].known.len(),
                    "counterfactual_known_callees": known,
                    "typed_entry": analyzed.typed.get(index).is_some_and(Option::is_some),
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            json!({
                "source": relative,
                "function_count": program.functions.len(),
                "function_instructions": program.functions.iter().map(|p| p.code.len()).sum::<usize>(),
                "entry_instructions": program.code.len(),
                "total_instructions": program.functions.iter().map(|p| p.code.len()).chain(std::iter::once(program.code.len())).sum::<usize>(),
                "generic_plan_direct_sites": generic.functions.iter().map(|p| p.known.len()).sum::<usize>(),
                "non_generic_plan_direct_sites": analyzed.functions.iter().map(|p| p.known.len()).sum::<usize>(),
                "functions": functions,
            })
        );
    }
}
