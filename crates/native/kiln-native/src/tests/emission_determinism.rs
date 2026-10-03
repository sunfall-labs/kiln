#[test]
fn instantiation_emission_is_stable_for_the_same_typescript_module() {
    let source = include_str!("../../tests/fixtures/multipart-headers.ts");
    let hir = kiln_frontend_oxc::parse("headers.ts", source);
    let module = kiln_ir::compile_module(&hir).expect("upstream TypeScript compiles");
    assert!(module.link_facts.module_instantiation_end > 0);
    let proto = &module.program.functions[3];
    let graph = kiln_backend_c::build_function_graph(proto, &module.program)
        .expect("header parser graph builds");
    for run in 0..20 {
        let again = kiln_backend_c::build_function_graph(proto, &module.program)
            .expect("same header parser graph builds");
        assert_eq!(graph, again, "SSA graph changed on run {run}");
    }

    let policy = kiln_backend_c::EmissionPolicy::default();
    let first = kiln_backend_c::emit_module_with_logical_path_and_policy_and_instantiation(
        &module,
        "source/packages/effect/src/http/MultipartParser/internal/headers.ts",
        policy.clone(),
        true,
    )
    .expect("split module emits");
    let expected = first
        .instantiation
        .as_ref()
        .expect("instantiation unit")
        .source
        .clone();

    for run in 1..50 {
        let emitted = kiln_backend_c::emit_module_with_logical_path_and_policy_and_instantiation(
            &module,
            "source/packages/effect/src/http/MultipartParser/internal/headers.ts",
            policy.clone(),
            true,
        )
        .expect("same module emits repeatedly");
        let actual = &emitted
            .instantiation
            .as_ref()
            .expect("instantiation unit")
            .source;
        if actual != &expected {
            std::fs::write("/tmp/kiln-instantiation-expected.c", &expected).unwrap();
            std::fs::write("/tmp/kiln-instantiation-actual.c", actual).unwrap();
            panic!("instantiation source changed on run {run}");
        }
    }

    std::thread::scope(|scope| {
        let workers = (0..6)
            .map(|_| {
                scope.spawn(|| {
                    kiln_backend_c::emit_module_with_logical_path_and_policy_and_instantiation(
                        &module,
                        "source/packages/effect/src/http/MultipartParser/internal/headers.ts",
                        policy.clone(),
                        true,
                    )
                    .expect("concurrent same-module emission")
                    .instantiation
                    .expect("instantiation unit")
                    .source
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            assert_eq!(worker.join().expect("emitter thread completes"), expected);
        }
    });
}
