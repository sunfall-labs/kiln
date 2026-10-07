use super::*;

#[test]
fn diagnostic_symbol_retention_changes_only_the_release_link_identity() {
    let options = NativeBuildOptions::default();
    assert_ne!(
        executable_link_options_digest_with_symbols(NativeProfile::Release, &options, false, &[]),
        executable_link_options_digest_with_symbols(NativeProfile::Release, &options, true, &[]),
        "release links with and without local symbols must never share a cache artifact"
    );
    assert_ne!(
        executable_link_options_digest_with_symbols(
            NativeProfile::ReleaseSize,
            &options,
            false,
            &[]
        ),
        executable_link_options_digest_with_symbols(
            NativeProfile::ReleaseSize,
            &options,
            true,
            &[]
        ),
        "size-release links also omit stripping only when explicitly requested"
    );
    assert_eq!(
        executable_link_options_digest_with_symbols(NativeProfile::Dev, &options, false, &[]),
        executable_link_options_digest_with_symbols(NativeProfile::Dev, &options, true, &[]),
        "non-release profiles never strip this symbol table"
    );
    assert_eq!(
        executable_link_options_digest_with_symbols(NativeProfile::Profile, &options, false, &[]),
        executable_link_options_digest_with_symbols(NativeProfile::Profile, &options, true, &[]),
        "the gprof profile already retains symbols"
    );
}

#[test]
fn provider_link_arguments_change_executable_cache_identity() {
    let options = NativeBuildOptions::default();
    let app_object = Digest::hash(b"same retained application object");
    let provider_archive = Digest::hash(b"same provider archive");
    let key = |arguments: &[String]| {
        NativeArtifactFamily::ExecutableLink.key(
            "clang-22",
            "x86_64-unknown-linux-gnu",
            NativeProfile::Release.cache(),
            executable_link_options_digest_with_symbols(
                NativeProfile::Release,
                &options,
                false,
                arguments,
            ),
            [app_object, provider_archive],
        )
    };

    let first = vec!["-lgtk-4".to_owned(), "-lwayland-client".to_owned()];
    let reordered = vec!["-lwayland-client".to_owned(), "-lgtk-4".to_owned()];
    let concatenated = vec!["-lgtk-4-lwayland-client".to_owned()];
    assert_ne!(key(&first), key(&reordered), "argument order is observable");
    assert_ne!(
        key(&first),
        key(&concatenated),
        "argument boundaries matter"
    );
}

#[test]
fn native_http_facade_demand_accounts_for_dynamic_global_access() {
    let mut members = kiln_graph::DemandedMembers::default();
    members.globals.insert("console".into());
    members.names.insert("log".into());
    assert!(
        !needs_native_http_facade(&members),
        "ordinary hello stays lean"
    );

    members.globals.insert("fetch".into());
    assert!(
        needs_native_http_facade(&members),
        "direct global fetch is retained"
    );

    let mut members = kiln_graph::DemandedMembers::default();
    members.globals.insert("globalThis".into());
    members.names.insert("fetch".into());
    assert!(
        needs_native_http_facade(&members),
        "static globalThis.fetch is retained"
    );

    members.names.clear();
    members.has_computed_access = true;
    assert!(
        needs_native_http_facade(&members),
        "computed global access is open"
    );

    members.has_computed_access = false;
    members.globals.insert("Reflect".into());
    members.names.insert("get".into());
    assert!(
        needs_native_http_facade(&members),
        "reflective global access is retained"
    );

    let mut members = kiln_graph::DemandedMembers::default();
    members.has_open_world = true;
    assert!(
        needs_native_http_facade(&members),
        "direct eval/open-world code is retained"
    );
}

fn object_single_flight_key(builder: &NativeBuilder, source: &[u8]) -> ArtifactKey {
    let tool = builder.toolchain().identity();
    NativeArtifactFamily::ModuleObject.key(
        compiler_identity(tool),
        &target_identity(tool),
        NativeProfile::Dev.cache(),
        Digest::hash(b"native-object-single-flight-test-options"),
        [Digest::hash(source), prelude_header_digest()],
    )
}

#[test]
fn release_prelude_compile_only_disables_vectorizers() {
    let mut options = CompileOptions::default();
    configure_release_prelude_compile(&mut options);
    let arguments = options
        .extra_args
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        ["-fno-vectorize", "-fno-slp-vectorize"],
        "scalar inlining is runtime-significant and must stay enabled"
    );
}

#[test]
fn release_prelude_compile_policy_has_a_distinct_object_key() {
    let base = Digest::hash(b"ordinary compiler options");
    assert_ne!(
        prelude_object_options(base, NativeProfile::Release),
        base,
        "release prelude objects must not reuse objects built under the old flags"
    );
    for profile in [
        NativeProfile::Dev,
        NativeProfile::Check,
        NativeProfile::Profile,
    ] {
        assert_eq!(prelude_object_options(base, profile), base);
    }
}

#[test]
fn concurrent_misses_for_one_native_object_compile_once_and_share_the_cached_bytes() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native object single-flight test: no C compiler found");
        return;
    };
    const CALLERS: usize = 4;
    let root = TestDirectory::new("kiln-native-object-single-flight");
    let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
    let source = b"int kiln_single_flight_probe(void) { return 17; }\n";
    let key = object_single_flight_key(&builder, source);
    let barrier = Barrier::new(CALLERS);
    let root_path = &root.path;

    let results = std::thread::scope(|scope| {
        let callers = (0..CALLERS)
            .map(|index| {
                let builder = &builder;
                let barrier = &barrier;
                let key = &key;
                scope.spawn(move || {
                    let caller = root_path.join(format!("caller-{index}"));
                    fs::create_dir_all(&caller).expect("create caller work directory");
                    let source_path = caller.join("probe.c");
                    let object_path = caller.join("probe.o");
                    fs::write(&source_path, source).expect("write identical C source");
                    barrier.wait();
                    builder
                        .object(
                            key,
                            source,
                            &source_path,
                            &object_path,
                            &CompileOptions::default(),
                        )
                        .expect("compile or reuse the shared object")
                })
            })
            .collect::<Vec<_>>();
        callers
            .into_iter()
            .map(|caller| caller.join().expect("object caller exits"))
            .collect::<Vec<_>>()
    });

    assert_eq!(results.iter().filter(|(_, _, hit, _)| !hit).count(), 1);
    assert_eq!(
        results.iter().filter(|(_, _, hit, _)| *hit).count(),
        CALLERS - 1
    );
    let first = &results[0].0;
    assert!(
        results.iter().all(|(bytes, _, _, _)| bytes == first),
        "all callers receive the same cached object bytes"
    );
    assert!(
        results
            .iter()
            .all(|(bytes, digest, _, _)| Digest::hash(bytes) == *digest)
    );
}

#[test]
fn readonly_native_object_hit_does_not_create_compile_locks() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping read-only native object hit test: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-object-readonly-hit");
    let source = b"int kiln_readonly_probe(void) { return 1; }\n";
    let key_builder = NativeBuilder::new(
        ArtifactCache::new(root.path.join("key-cache")),
        toolchain.clone(),
    );
    let key = object_single_flight_key(&key_builder, source);
    let cache_root = root.path.join("readonly-cache");
    let writable = ArtifactCache::new(&cache_root);
    writable
        .put(&key, b"seeded object payload")
        .expect("seed cache record");
    let builder = NativeBuilder::new(
        ArtifactCache::with_layers([kiln_artifact_cache::CacheLayer::read_only(&cache_root)]),
        toolchain,
    );

    let (bytes, digest, hit, _) = builder
        .object(
            &key,
            source,
            &root.path.join("not-needed.c"),
            &root.path.join("not-needed.o"),
            &CompileOptions::default(),
        )
        .expect("read-only object hit avoids compiler and writable layer");
    assert!(hit);
    assert_eq!(bytes, b"seeded object payload");
    assert_eq!(digest, Digest::hash(&bytes));
    assert!(
        !cache_root.join(".compile-locks").exists(),
        "warm read-only hits must not touch compile-lock storage"
    );
}

#[test]
fn failed_native_object_compile_releases_its_bucket_for_retry() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native object retry test: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-object-lock-retry");
    let builder = NativeBuilder::new(ArtifactCache::new(root.path.join("cache")), toolchain);
    let source = b"int kiln_retry_probe(void) { return 23; }\n";
    let key = object_single_flight_key(&builder, source);
    let source_path = root.path.join("probe.c");
    fs::write(&source_path, source).expect("write retry source");
    let failed_object_path = root.path.join("failed.o");
    fs::create_dir(&failed_object_path).expect("make the first compiler output path invalid");

    assert!(
        builder
            .object(
                &key,
                source,
                &source_path,
                &failed_object_path,
                &CompileOptions::default(),
            )
            .is_err()
    );
    assert!(matches!(builder.lookup(&key), Ok(Lookup::Miss)));

    let (_, _, hit, _) = builder
        .object(
            &key,
            source,
            &source_path,
            &root.path.join("retry.o"),
            &CompileOptions::default(),
        )
        .expect("retry acquires the released bucket and stores the object");
    assert!(!hit, "the successful retry is the cache miss producer");
}

#[test]
fn release_large_modules_preserve_throughput_policy() {
    let mut options = CompileOptions::default();
    configure_release_module_compile(&mut options, true, false);
    let arguments = options
        .extra_args
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        [
            "-O0",
            "-fno-sanitize=undefined",
            "-fno-function-sections",
            "-fno-data-sections"
        ]
    );
    assert_eq!(
        release_module_compile_identity(true, false),
        "generic-o0-nosan-nosections"
    );
}

#[test]
fn release_generic_mixed_tier_uses_a_distinct_optimization_identity() {
    let mut options = CompileOptions::default();
    configure_release_module_compile(&mut options, true, true);
    let arguments = options
        .extra_args
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        arguments,
        [
            "-O1",
            "-DKILN_GENERIC_MIXED_TIER=1",
            "-fno-sanitize=undefined",
            "-fno-function-sections",
            "-fno-data-sections"
        ]
    );
    assert_eq!(
        release_module_compile_identity(true, true),
        "generic-mixed-o1-nosan-nosections"
    );
}

#[test]
fn release_module_specialization_budget_counts_all_instructions() {
    let mut program = Program {
        source: String::new().into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return; MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    assert!(!release_module_uses_generic_bodies(
        &budget_module(program.clone()),
        &EmissionPolicy::default()
    ));

    program.functions.push(empty_function());
    assert!(release_module_uses_generic_bodies(
        &budget_module(program.clone()),
        &EmissionPolicy::default()
    ));
}

#[test]
fn release_module_specialization_budget_counts_functions() {
    let mut program = Program {
        source: String::new().into(),
        version: BYTECODE_VERSION,
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![empty_function(); MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS],
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };
    assert!(!release_module_uses_generic_bodies(
        &budget_module(program.clone()),
        &EmissionPolicy::default()
    ));

    program.functions.push(empty_function());
    assert!(release_module_uses_generic_bodies(
        &budget_module(program.clone()),
        &EmissionPolicy::default()
    ));
}

#[test]
fn release_specialization_ignores_dead_functions_but_keeps_live_and_eval_budgets() {
    let mut source = String::from(
        "export function hot(n) { let x = 0; for (let i = 0; i < n; i++) x += i; return x; }\n",
    );
    for index in 0..32 {
        source.push_str(&format!("function unused{index}() {{ return 1; }}\n"));
    }
    source.push_str("class Unused { method() { return 1; } }\n");
    let mut module =
        kiln_ir::compile_module(&kiln_frontend_oxc::parse("main.mjs", &source)).unwrap();
    let demanded = module
        .exports
        .iter()
        .map(|export| export.local_slot)
        .collect();
    let policy = EmissionPolicy {
        live_module_slots: Some(Arc::new(demanded)),
        ..Default::default()
    };
    assert!(!release_module_uses_generic_bodies(&module, &policy));
    assert_eq!(
        kiln_backend_c::retained_module_size(&module, policy.live_module_slots.as_deref()).1,
        1
    );
    assert!(
        release_module_uses_generic_bodies(&module, &EmissionPolicy::default()),
        "shared/open modules retain all bodies"
    );
    let all = EmissionPolicy {
        live_module_slots: Some(Arc::new((0..module.program.local_count).collect())),
        ..Default::default()
    };
    assert!(release_module_uses_generic_bodies(&module, &all));
    // A single dead function can exceed the instruction budget without hitting the function
    // count limit. Its body must not disable specialization either.
    let huge_source = format!(
        "export function hot(n) {{ return n + 1; }} function unused() {{ {} }}",
        "console.log(1);".repeat(MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS)
    );
    let huge =
        kiln_ir::compile_module(&kiln_frontend_oxc::parse("main.mjs", &huge_source)).unwrap();
    assert!(release_module_uses_generic_bodies(
        &huge,
        &EmissionPolicy::default()
    ));
    let huge_policy = EmissionPolicy {
        live_module_slots: Some(Arc::new(
            huge.exports
                .iter()
                .map(|export| export.local_slot)
                .collect(),
        )),
        ..Default::default()
    };
    assert!(!release_module_uses_generic_bodies(&huge, &huge_policy));
    module.link_facts.direct_eval = true;
    assert!(release_module_uses_generic_bodies(&module, &policy));
}

#[test]
fn release_pruning_preserves_superclass_tdz() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-class-tdz");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "class Unused extends Later {} class Later {} console.log('continued');",
    )
    .unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("tdz"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(report.output)
        .output_when_launchable()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("ReferenceError"),
        "{:?}",
        result
    );
}

#[test]
fn executable_module_sizes_reports_retained_and_dropped() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-module-sizes");
    fs::write(
        project.path.join("package.json"),
        r#"{"sideEffects": false}"#,
    )
    .expect("write package.json");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import { a } from './dep1.mjs';\nimport './dep2.mjs';\nconsole.log(a());\n",
    )
    .expect("write entry");
    fs::write(
        project.path.join("dep1.mjs"),
        "export function a() { return 1; }\nexport function b() { return 2; }\nfunction helperUnused() { return 3; }\n",
    )
    .expect("write dep1");
    fs::write(
        project.path.join("dep2.mjs"),
        "globalThis.__sideEffectProbe = 1;\n",
    )
    .expect("write dep2");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("sizes graph");
    let sizes = kiln_liveness::executable_module_sizes(
        &graph,
        &kiln_liveness::SelectionOptions::development(),
    )
    .expect("module sizes");
    assert_eq!(sizes.len(), 3, "entry plus two dependencies");
    let by_name = |name: &str| {
        sizes
            .iter()
            .find(|size| size.path.ends_with(name))
            .unwrap_or_else(|| panic!("{name} reported"))
    };
    let entry_sizes = by_name("main.mjs");
    assert!(entry_sizes.live);
    let dep1 = by_name("dep1.mjs");
    assert!(dep1.live);
    assert!(
        dep1.retained_functions < dep1.total_functions,
        "unused helper is not retained: {dep1:?}"
    );
    assert!(
        dep1.retained_instructions < dep1.total_instructions,
        "unused bodies shrink the module: {dep1:?}"
    );
    let dep2 = by_name("dep2.mjs");
    assert!(
        !dep2.live,
        "side-effect-only import of a side-effect-free package is dropped: {dep2:?}"
    );
    assert_eq!(dep2.retained_instructions, 0);
    assert_eq!(dep2.retained_functions, 0);
}

#[test]
fn executable_module_sizes_prunes_unused_pure_declarations() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-pure-declarations-pruning");
    fs::write(
        project.path.join("package.json"),
        r#"{"sideEffects": false}"#,
    )
    .expect("write package.json");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import { a } from './dep.mjs';\nconsole.log(a());\n",
    )
    .expect("write entry");
    fs::write(
        project.path.join("dep.mjs"),
        "export function a() { return 1; }\nconst UNUSED_ARR = [1, 2, 3];\nconst UNUSED_OBJ = { x: 10, y: 20 };\nconst UNUSED_NUM = 42;\n",
    )
    .expect("write dep");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("sizes graph");
    let sizes = kiln_liveness::executable_module_sizes(
        &graph,
        &kiln_liveness::SelectionOptions::development(),
    )
    .expect("module sizes");
    let dep = sizes
        .iter()
        .find(|size| size.path.ends_with("dep.mjs"))
        .expect("dep reported");
    assert!(
        dep.retained_instructions < dep.total_instructions,
        "unused pure declarations shrink the module: {dep:?}"
    );
}

#[test]
fn executable_demanded_members_names_builtins_and_computed() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-demanded-members");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "const xs = [1];\nxs.push(2);\nconst o = { log(x) { return x; } };\no.log(xs[0]);\nconst key = 'length';\nconst n = xs[key];\nsetTimeout(() => o.log(n), 0);\n",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("members graph");
    let demand =
        kiln_liveness::executable_demand(&graph, &kiln_liveness::SelectionOptions::development())
            .expect("demand");
    let members = kiln_graph::analyze_demanded_members(&graph, &demand.live, &demand.slots);
    assert!(members.names.contains("push"), "{members:?}");
    assert!(members.names.contains("log"), "{members:?}");
    assert!(members.builtins.contains("SetTimeout"), "{members:?}");
    assert!(
        members.has_computed_access,
        "xs[key] names nothing statically: {members:?}"
    );
    assert!(!members.has_open_world, "{members:?}");
}

#[test]
fn retained_method_groups_drops_node_groups_without_demand() {
    use kiln_graph::{CORE_METHOD_GROUPS, PRUNABLE_METHOD_GROUPS, method_groups as groups};
    use kiln_module_resolver::NodeRuntimeResolver;

    fn mask_for(entry_name: &str, files: &[(&str, &str)]) -> u32 {
        let project = TestDirectory::new("kiln-group-mask");
        for (name, source) in files {
            fs::write(project.path.join(name), source).expect("write mask fixture");
        }
        let graph = ModuleGraph::build_node(
            &project.path.join(entry_name),
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("mask graph");
        let demand = kiln_liveness::executable_demand(
            &graph,
            &kiln_liveness::SelectionOptions::development(),
        )
        .expect("demand");
        let members = kiln_graph::analyze_demanded_members(&graph, &demand.live, &demand.slots);
        kiln_liveness::retained_method_groups(&graph, &demand, &members)
    }

    assert_eq!(
        mask_for("main.mjs", &[("main.mjs", "console.log(1 + 2);\n")]),
        CORE_METHOD_GROUPS,
        "a program with no node use keeps core groups only"
    );
    let http_mask = mask_for(
        "main.mjs",
        &[(
            "main.mjs",
            "import http from 'node:http';\nconsole.log(typeof http.createServer);\n",
        )],
    );
    for group in [
        groups::NODE_HTTP,
        groups::NODE_RESPONSE,
        groups::NODE_SERVER,
        groups::NODE_NET,
        groups::NODE_STREAM,
        groups::NODE_EVENT,
        groups::BUFFER,
    ] {
        assert!(
            http_mask & group != 0,
            "node:http keeps its groups: {http_mask:#x}"
        );
    }
    assert_eq!(
        http_mask & PRUNABLE_METHOD_GROUPS,
        PRUNABLE_METHOD_GROUPS,
        "node:http's built-in global-object/computed-access path retains the conservative full table"
    );
    let path_mask = mask_for(
        "main.mjs",
        &[(
            "main.mjs",
            "import path from 'node:path';\nconsole.log(typeof path.join);\n",
        )],
    );
    assert_ne!(
        path_mask & groups::NODE_PATH,
        0,
        "node:path retains the dispatch group for its methods"
    );
    assert_eq!(
        path_mask & groups::NODE_HTTP,
        0,
        "node:path alone keeps no HTTP groups"
    );
    let buffer_mask = mask_for(
        "main.mjs",
        &[("main.mjs", "console.log(Buffer.from('hi').length);\n")],
    );
    assert!(
        buffer_mask & groups::BUFFER != 0,
        "the Buffer global keeps its group"
    );
    assert_eq!(
        buffer_mask & groups::NODE_HTTP,
        0,
        "Buffer alone keeps no HTTP groups"
    );
    let fs_mask = mask_for(
        "main.mjs",
        &[(
            "main.mjs",
            "import fs from 'node:fs';\nconsole.log(typeof fs.readFile);\n",
        )],
    );
    let node_groups = groups::NODE_EVENT
        | groups::NODE_NET
        | groups::NODE_SERVER
        | groups::NODE_HTTP
        | groups::NODE_RESPONSE
        | groups::BUFFER
        | groups::NODE_UTIL
        | groups::NODE_DIAGNOSTICS
        | groups::NODE_STREAM
        | groups::NODE_PATH
        | groups::NODE_CRYPTO;
    assert_eq!(
        fs_mask & node_groups,
        groups::NODE_STREAM | groups::NODE_EVENT | groups::BUFFER,
        "node:fs keeps stream/event and the Buffer methods its reads can return: {fs_mask:#x}"
    );
    let crypto_mask = mask_for(
        "main.mjs",
        &[(
            "main.mjs",
            "import { createHash } from 'node:crypto';\nconsole.log(typeof createHash);\n",
        )],
    );
    assert_ne!(
        crypto_mask & groups::NODE_CRYPTO,
        0,
        "node:crypto keeps its private raw bridge capability"
    );
    assert_eq!(
        CORE_METHOD_GROUPS & groups::NODE_CRYPTO,
        0,
        "crypto bridge is not part of the always-retained core"
    );
    let global_crypto_mask = mask_for(
        "main.mjs",
        &[("main.mjs", "console.log(typeof crypto.subtle.digest);\n")],
    );
    assert_ne!(
        global_crypto_mask & groups::NODE_CRYPTO,
        0,
        "the global WebCrypto/JOSE path keeps its crypto capability"
    );
    let dynamic_mask = mask_for(
        "main.mjs",
        &[
            (
                "main.mjs",
                "import('./dep.mjs').then((m) => console.log(m.a));\n",
            ),
            ("dep.mjs", "export const a = 1;\n"),
        ],
    );
    assert_eq!(
        dynamic_mask,
        CORE_METHOD_GROUPS | PRUNABLE_METHOD_GROUPS,
        "dynamic import keeps the full table"
    );
    let temporal_mask = mask_for(
        "main.mjs",
        &[("main.mjs", "console.log(Temporal.Now.instant());\n")],
    );
    assert_ne!(
        temporal_mask & groups::TEMPORAL,
        0,
        "loading the Temporal global keeps its semantic dispatch"
    );
    let math_mask = mask_for(
        "main.mjs",
        &[("main.mjs", "console.log(Math.floor(1.5));\n")],
    );
    assert_ne!(math_mask & groups::MATH_NUMBER, 0);
    assert_eq!(math_mask & groups::TEMPORAL, 0);
}

#[test]
fn compact_function_context_is_omitted_only_when_unused() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping compact context fixture: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-compact-context");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        include_str!("../../../../../conformance/native/compact-function-context.js"),
    )
    .expect("write context fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("context graph");
    let emitted = emit_native_graph_module_with_policy(
        graph.module(graph.entry()).expect("entry module"),
        "main.mjs",
        &kiln_backend_c::EmissionPolicy {
            compact: true,
            ..kiln_backend_c::EmissionPolicy::default()
        },
    )
    .expect("emit compact context fixture");
    let identity = emitted
        .source
        .split("/* function ")
        .find(|body| {
            body.starts_with("0: identity */")
                || body
                    .lines()
                    .next()
                    .is_some_and(|line| line.ends_with(": identity */"))
        })
        .expect("identity body")
        .split("\n}\n")
        .next()
        .unwrap();
    assert!(!identity.contains("ctx->"), "{identity}");
    assert!(!identity.contains("receiver_ptr ?"), "{identity}");
    assert!(
        !identity.contains("kiln_c99_inheriting_new_target"),
        "{identity}"
    );
    assert!(
        emitted
            .source
            .contains("kiln_generator *frame = ctx->frame;")
    );
    assert!(emitted.source.contains("kiln_value *thrown = ctx->thrown;"));
    assert!(emitted.source.contains("receiver_ptr ? *receiver_ptr"));

    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("context"), NativeProfile::Release),
        )
        .expect("build compact context fixture");
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run context fixture");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "17 undefined\n41\n40 42\n44\n45\n46\n43\n"
    );
}

#[test]
fn dead_phi_pruning_preserves_effects_parallel_copies_and_suspension() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping dead phi fixture: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-dead-phi");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        include_str!("../../../../../conformance/native/dead-phi-copies.js"),
    )
    .expect("write dead phi fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("dead phi graph");
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("phis"), NativeProfile::Release),
        )
        .expect("build dead phi fixture");
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run dead phi fixture");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "123 231 312 123 231\ncoercion\n12\n9\n9\n1 2 208\n208\n"
    );
}

#[test]
fn release_dce_relinks_only_demanded_exports() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("release-demand-dce");
    let dependency = project.path.join("dependency.mjs");
    let entry = project.path.join("main.mjs");
    fs::write(
        &dependency,
        "export function used() { return 42 } export function unused() { return 0 }",
    )
    .expect("write dependency");
    fs::write(
        &entry,
        "import { used } from './dependency.mjs'; console.log(used())",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build source graph");
    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select live modules");
    let demand = selected.demand();
    let (reduced, demand) = reduced_graph(&graph, &demand).expect("reduce graph");
    let (specialized, remapped) = dce_release_graph(&reduced, &demand);
    let dependency = fs::canonicalize(dependency).expect("canonical dependency");
    let node = specialized
        .module(&dependency)
        .expect("specialized dependency");

    assert_eq!(node.program.program.functions.len(), 1);
    assert_eq!(
        node.program
            .exports
            .iter()
            .map(|export| export.exported.as_str())
            .collect::<Vec<_>>(),
        ["used"]
    );
    assert_eq!(remapped.slots[&dependency].len(), 1);
}

#[test]
fn indexed_compiles_return_in_index_order_under_uneven_work() {
    let count = 64;
    let objects = compile_indexed(count, "test compile", |index| {
        // Deliberately uneven: the low indices are the slow ones, so a scheduler that
        // returned completion order rather than index order would say so loudly.
        if index % 8 == 0 {
            std::thread::sleep(Duration::from_millis(10 * (8 - index / 8) as u64));
        }
        Ok(index)
    })
    .expect("every index compiles");
    assert_eq!(objects, (0..count).collect::<Vec<_>>());
}

#[test]
fn an_indexed_compile_reports_the_lowest_failing_index() {
    let failure = compile_indexed(32, "test compile", |index| {
        // The later failure is also the faster one, so whichever worker claims it will
        // usually report first. The lowest failing index must still win.
        if index == 3 {
            std::thread::sleep(Duration::from_millis(20));
        }
        if index == 3 || index == 17 {
            return Err(NativeError::NumberProvider {
                message: format!("index {index}"),
            });
        }
        Ok(index)
    })
    .expect_err("a failing index fails the batch");
    match failure {
        NativeError::NumberProvider { message } => assert_eq!(message, "index 3"),
        other => panic!("unexpected failure: {other}"),
    }
}

#[test]
fn a_panicking_indexed_worker_becomes_a_worker_panic() {
    let failure = compile_indexed(16, "panicking test compile", |index| {
        assert_ne!(index, 9, "deliberate test panic");
        Ok(index)
    })
    .expect_err("a panicking worker fails the batch");
    assert!(matches!(
        failure,
        NativeError::WorkerPanic {
            stage: "panicking test compile"
        }
    ));
}

#[test]
fn live_cache_identity_catalogue_covers_every_stage_and_profile_family() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native cache identity catalogue: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-cache-identities");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let identities = builder
        .live_cache_identities()
        .expect("current native identities are enumerable");

    // The ordinary shared-library object and its final link share one toolchain identity by
    // design; identity digests intentionally omit artifact kind and link inputs. Every other
    // catalogued family/profile contributes a distinct identity.
    let tool = builder.toolchain().identity();
    let target = target_identity(tool);
    let compiler = compiler_identity(tool);
    let provider_identities = number_provider::producer_identities(tool)
        .expect("provider identities without archive builds");
    // Installed providers and capability additions change this count independently of the
    // C families. Count their enumerated variants instead of pinning the old eleven-variant
    // total. One object/link pair aliases in each of the five native profile families.
    let non_provider_count =
        NativeArtifactFamily::SOURCES.len() - 1 + (NativeArtifactFamily::OBJECTS.len() - 1) * 5;
    assert_eq!(
        identities.len(),
        non_provider_count + provider_identities.len()
    );
    for family in NativeArtifactFamily::SOURCES {
        // Providers are checked below against every configured variant. An explicit
        // manifest may select only FullGui or a capability-minimal variant.
        if matches!(family, NativeArtifactFamily::NumberProvider) {
            continue;
        }
        let (backend, abi) = family.backend_abi(
            &module_abi_identity(),
            &runtime_abi_identity(),
            &shared_embedding_abi_identity(),
        );
        assert!(identities.contains(&cache_identity(
            family.compiler_version(),
            backend,
            &abi,
            &target,
            CacheProfile::Debug,
        )));
    }
    for (_variant, provider_identity) in provider_identities {
        assert!(identities.contains(&cache_identity(
            provider_identity,
            Backend::Custom("rust-native-number-provider".to_owned()),
            &native_number_abi_identity(),
            &target,
            CacheProfile::Release,
        )));
    }
    for profile in [
        CacheProfile::Debug,
        CacheProfile::Check,
        CacheProfile::Release,
        CacheProfile::Profile,
    ] {
        for family in NativeArtifactFamily::OBJECTS {
            let (backend, abi) = family.backend_abi(
                &module_abi_identity(),
                &runtime_abi_identity(),
                &shared_embedding_abi_identity(),
            );
            assert!(identities.contains(&cache_identity(
                compiler.clone(),
                backend,
                &abi,
                &target,
                profile.clone()
            )));
        }
    }
}

#[test]
fn compiler_environment_identity_is_allowlisted_and_unambiguously_framed() {
    let clang_baseline = environment_digest(CompilerKind::Clang, &[]);
    for name in C_COMPILER_ENVIRONMENT {
        assert_ne!(
            environment_digest(CompilerKind::Clang, &[(name, "changed")]),
            clang_baseline,
            "{name} must participate in native cache identity"
        );
    }
    for name in GCC_COMPILER_ENVIRONMENT {
        assert_ne!(
            environment_digest(CompilerKind::Gcc, &[(name, "changed")]),
            environment_digest(CompilerKind::Gcc, &[]),
            "{name} must participate in GCC cache identity"
        );
    }
    for kind in [CompilerKind::Clang, CompilerKind::Zig] {
        assert_eq!(
            environment_digest(kind, &[("GCC_EXEC_PREFIX", "ignored")]),
            environment_digest(kind, &[]),
            "GCC-only policy must not invalidate {kind} artifacts"
        );
    }
    assert_ne!(
        environment_digest(CompilerKind::Zig, &[("ZIG_LIB_DIR", "changed")]),
        environment_digest(CompilerKind::Zig, &[]),
        "Zig's library override must participate in Zig cache identity"
    );
    assert_eq!(
        compiler_environment_digest_with(CompilerKind::Clang, |name| {
            (name == "KILN_UNRELATED").then(|| "ignored".into())
        }),
        clang_baseline,
        "variables outside the compiler allowlist do not affect artifacts"
    );
    for name in ["CPLUS_INCLUDE_PATH", "OBJC_INCLUDE_PATH"] {
        assert_eq!(
            environment_digest(CompilerKind::Clang, &[(name, "ignored")]),
            clang_baseline,
            "{name} does not affect a C11 translation unit"
        );
    }
    assert_ne!(
        environment_digest(CompilerKind::Clang, &[("CPATH", "")]),
        clang_baseline,
        "an empty search path is not the same compiler policy as an unset one"
    );
    assert_ne!(
        environment_digest(
            CompilerKind::Clang,
            &[("CPATH", "a"), ("C_INCLUDE_PATH", "bc")]
        ),
        environment_digest(
            CompilerKind::Clang,
            &[("CPATH", "ab"), ("C_INCLUDE_PATH", "c")]
        ),
        "length framing must distinguish adjacent values"
    );
}

#[test]
fn compiler_environment_changes_miss_native_artifact_keys() {
    let root = TestDirectory::new("kiln-native-environment-key");
    let cache = ArtifactCache::new(root.path.join("cache"));
    let options = Digest::hash(b"same options");
    let inputs = [Digest::hash(b"same generated C")];
    let make_key = |environment| {
        key(
            ArtifactKind::NativeObject,
            compiler_identity_from_parts("same tool", environment),
            Backend::C,
            "same ABI",
            "same target",
            CacheProfile::Debug,
            options,
            inputs,
        )
    };
    let first = make_key(environment_digest(
        CompilerKind::Clang,
        &[("CPATH", "/first/include")],
    ));
    let second = make_key(environment_digest(
        CompilerKind::Clang,
        &[("CPATH", "/second/include")],
    ));

    cache.put(&first, b"object").expect("seed first identity");
    assert_eq!(
        cache.lookup(&first).expect("first lookup"),
        Lookup::Hit(kiln_artifact_cache::Artifact {
            bytes: b"object".to_vec(),
            digest: Digest::hash(b"object"),
            key_digest: first.digest(),
            layer_index: 0,
            path: cache.path_for(&first).expect("first path"),
        })
    );
    assert_eq!(cache.lookup(&second).expect("second lookup"), Lookup::Miss);
}

#[test]
fn deterministic_payload_hits_reject_self_consistent_wrong_bytes() {
    let root = TestDirectory::new("kiln-native-expected-payload");
    let cache = ArtifactCache::new(root.path.join("cache"));
    let expected = b"deterministic generated C";
    let payload_key = key(
        ArtifactKind::GeneratedC,
        "kiln-test-generator",
        Backend::C,
        "test ABI",
        "test target",
        CacheProfile::Debug,
        Digest::hash(b"test options"),
        [Digest::hash(expected)],
    );
    cache
        .put(&payload_key, b"different but internally valid bytes")
        .expect("seed a self-consistent wrong record");

    let error = cached_payload(&cache, &payload_key, expected)
        .expect_err("a deterministic payload must match its expected bytes");
    assert!(
        matches!(
            error,
            NativeError::Cache(CacheError::Corrupt { ref reason, .. })
                if reason.contains("does not match deterministic expected digest")
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn materialization_skips_a_stale_temporary_name() {
    let root = TestDirectory::new("kiln-native-stale-temporary");
    let destination = root.path.join("program");
    let stale = materialized_temporary_path(&destination, 7);
    fs::write(&stale, b"crashed writer").expect("seed stale temporary");
    let mut counters = [7, 8].into_iter();

    let temporary = write_materialized_temporary_with(&destination, b"new artifact", false, || {
        counters.next().expect("bounded retry sequence")
    })
    .expect("stale temporary is skipped");

    assert_eq!(temporary, materialized_temporary_path(&destination, 8));
    assert_eq!(
        fs::read(&stale).expect("stale file remains"),
        b"crashed writer"
    );
    assert_eq!(
        fs::read(&temporary).expect("read new temporary"),
        b"new artifact"
    );
    fs::remove_file(temporary).expect("remove new temporary");
}

#[test]
fn immutable_materialization_never_replaces_a_concurrent_winner() {
    let root = TestDirectory::new("kiln-native-immutable-race");
    let destination = root.path.join("content.so");
    let barrier = Arc::new(Barrier::new(3));
    let writers = [b"first payload".as_slice(), b"second payload".as_slice()]
        .into_iter()
        .map(|payload| {
            let destination = destination.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                materialize_immutable(&destination, payload)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = writers
        .into_iter()
        .map(|writer| writer.join().expect("immutable writer exits"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(NativeError::ImmutableArtifactCollision { .. })))
            .count(),
        1
    );
    let installed = fs::read(destination).expect("read concurrent winner");
    assert!(
        installed == b"first payload" || installed == b"second payload",
        "unexpected installed payload: {installed:?}"
    );
}

#[test]
fn native_failed_dynamic_dependency_is_cached_with_original_error() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping dynamic failure integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-failure-cache");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::write(
        root.path.join("dep.mjs"),
        "globalThis.depCount = (globalThis.depCount || 0) + 1; throw 'boom';\n",
    )
    .expect("write throwing dependency");
    fs::write(
        root.path.join("target.mjs"),
        "import './dep.mjs'; globalThis.targetCount = (globalThis.targetCount || 0) + 1;\n",
    )
    .expect("write dependent module");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "const first = await import('./target.mjs').catch(error => error);\n\
         const second = await import('./target.mjs').catch(error => error);\n\
         console.log(globalThis.depCount, globalThis.targetCount || 0, first === second, first);\n",
    )
    .expect("write failure entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("dynamic failure graph compiles");
    for (profile, name) in [
        (NativeProfile::Dev, "dev"),
        (NativeProfile::Release, "release"),
        (NativeProfile::ReleaseSize, "release-size"),
    ] {
        let report = NativeBuilder::new(
            test_cache(root.path.join(format!("cache-{name}"))),
            toolchain.clone(),
        )
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(format!("program-{name}")),
                profile,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("dynamic failure graph builds and runs");
        assert_eq!(report.stdout, b"1 0 true boom\n", "profile {name}");
    }
}

#[test]
fn native_build_with_march_native_opt_in_succeeds() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let toolchain = match Toolchain::discover() {
        Ok(toolchain) => toolchain,
        Err(ToolchainError::NoCompiler { .. }) => return,
        Err(error) => panic!("compiler probe failed: {error}"),
    };
    let project = TestDirectory::new("kiln-native-march-native");
    let entry = project.path.join("main.mjs");
    fs::write(&entry, "console.log(42);").expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let output = project.path.join("program");
    let options = NativeBuildOptions::new(output, NativeProfile::Release).with_march_native(true);
    let report = project
        .builder(toolchain)
        .build_graph(&graph, &options)
        .expect("native build with march=native succeeds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native executable");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
}

#[test]
fn native_build_with_type_profile_records_and_optimizes() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let toolchain = match Toolchain::discover() {
        Ok(toolchain) => toolchain,
        Err(ToolchainError::NoCompiler { .. }) => return,
        Err(error) => panic!("compiler probe failed: {error}"),
    };
    let project = TestDirectory::new("kiln-native-type-profile");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"
function add(a, b) {
    return a + b;
}
console.log(add(10, 20));
"#,
    )
    .expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let output = project.path.join("program_profiled");
    let options = NativeBuildOptions::new(output.clone(), NativeProfile::Profile);
    let report = project
        .builder(toolchain.clone())
        .build_graph(&graph, &options)
        .unwrap_or_else(|e| panic!("native profile build failed: {e:?}"));

    let profile_file = project.path.join("main.typeprofile");
    let run_output = Command::new(&report.output)
        .env("KILN_TYPE_PROFILE_FILE", &profile_file)
        .output_when_launchable()
        .expect("run native executable");
    assert_eq!(String::from_utf8_lossy(&run_output.stdout).trim(), "30");

    assert!(profile_file.is_file(), "type profile file must be written");
    let prof_content = fs::read_to_string(&profile_file).expect("read profile");
    let prof: kiln_backend_c::TypeProfile =
        kiln_backend_c::TypeProfile::parse_json(&prof_content).expect("parse profile JSON");
    let add_fn = prof
        .functions
        .iter()
        .find(|f| f.name.as_deref() == Some("add"))
        .expect("found add fn");
    assert_eq!(add_fn.parameters, vec!["number", "number"]);

    // Next build: Release with profile passed in via options.with_type_profile_path
    let output2 = project.path.join("program_optimized");
    let options2 = NativeBuildOptions::new(output2.clone(), NativeProfile::Release)
        .with_type_profile_path(&profile_file);
    let report2 = project
        .builder(toolchain)
        .build_graph(&graph, &options2)
        .expect("optimized build succeeds");

    let run_output2 = Command::new(&report2.output)
        .output_when_launchable()
        .expect("run optimized native executable");
    assert_eq!(String::from_utf8_lossy(&run_output2.stdout).trim(), "30");

    // Also test under KILN_SPECULATE=exit:
    let run_spec_exit = Command::new(&report2.output)
        .env("KILN_SPECULATE", "exit")
        .output_when_launchable()
        .expect("run optimized native executable under KILN_SPECULATE=exit");
    assert_eq!(String::from_utf8_lossy(&run_spec_exit.stdout).trim(), "30");
}

#[test]
fn parse_worker_count_semantics() {
    assert_eq!(parse_worker_count(None), 1);
    assert_eq!(parse_worker_count(Some("")), 1);
    assert_eq!(parse_worker_count(Some("0")), 1);
    assert_eq!(parse_worker_count(Some("-1")), 1);
    assert_eq!(parse_worker_count(Some("-42")), 1);
    assert_eq!(parse_worker_count(Some("foo")), 1);
    assert_eq!(parse_worker_count(Some("4bad")), 1);
    assert_eq!(parse_worker_count(Some("1")), 1);
    assert_eq!(parse_worker_count(Some("4")), 4);
    assert_eq!(parse_worker_count(Some("16")), 16);
    assert_eq!(parse_worker_count(Some("1024")), 1024);
    assert_eq!(parse_worker_count(Some("2000")), 1024);

    // Case-insensitive "auto"
    assert_eq!(parse_worker_count_with(Some("auto"), || 8), 8);
    assert_eq!(parse_worker_count_with(Some("Auto"), || 8), 8);
    assert_eq!(parse_worker_count_with(Some("AUTO"), || 8), 8);
    assert_eq!(parse_worker_count_with(Some("aUtO"), || 8), 8);
    assert_eq!(parse_worker_count_with(Some("auto"), || 0), 1);
    assert_eq!(parse_worker_count_with(Some("auto"), || 2048), 1024);
    assert_eq!(parse_worker_count_with(Some("auto!"), || 8), 1);

    // Live auto queries actual online cpus
    let live = parse_worker_count(Some("auto"));
    assert!((1..=1024).contains(&live));
}

#[test]
fn kiln_cli_worker_count_c_runtime_matches_contract() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native worker count test: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-worker-count");
    let c_source = r#"
#include <locale.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#ifndef _WIN32
#include <sys/wait.h>
#include <unistd.h>
#endif

static long kiln_cli_worker_count(void) {
    const char *setting = getenv("KILN_WORKERS");
    char *end = NULL;
    long count;
    if (setting == NULL || *setting == '\0') return 1;
    if ((setting[0] == 'a' || setting[0] == 'A') &&
        (setting[1] == 'u' || setting[1] == 'U') &&
        (setting[2] == 't' || setting[2] == 'T') &&
        (setting[3] == 'o' || setting[3] == 'O') &&
        setting[4] == '\0') {
#ifndef _WIN32
        long cpus = sysconf(_SC_NPROCESSORS_ONLN);
        count = cpus > 0 ? cpus : 1;
#else
        count = 1;
#endif
    } else {
        count = strtol(setting, &end, 10);
        if (end == setting || *end != '\0' || count < 1) return 1;
    }
    if (count < 1) count = 1;
    if (count > 1024) count = 1024;
    return count;
}

int main(void) {
    printf("%ld\n", kiln_cli_worker_count());
    return 0;
}
"#;
    let src_file = project.path.join("test_worker.c");
    let obj_file = project.path.join("test_worker.o");
    let bin_file = project.path.join("test_worker");
    fs::write(&src_file, c_source).expect("write C source");

    toolchain
        .compile_c(&src_file, &obj_file, &CompileOptions::default())
        .expect("compile C probe");
    toolchain
        .link_executable(&[obj_file], &bin_file, &LinkOptions::default())
        .expect("link C probe");

    let run_with = |env_val: Option<&str>| -> String {
        let mut cmd = Command::new(&bin_file);
        if let Some(val) = env_val {
            cmd.env("KILN_WORKERS", val);
        } else {
            cmd.env_remove("KILN_WORKERS");
        }
        let output = cmd.output_when_launchable().expect("run probe");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    assert_eq!(run_with(None), "1");
    assert_eq!(run_with(Some("")), "1");
    assert_eq!(run_with(Some("0")), "1");
    assert_eq!(run_with(Some("-5")), "1");
    assert_eq!(run_with(Some("invalid")), "1");
    assert_eq!(run_with(Some("4abc")), "1");
    assert_eq!(run_with(Some("1")), "1");
    assert_eq!(run_with(Some("4")), "4");
    assert_eq!(run_with(Some("2000")), "1024");

    let auto_res = run_with(Some("auto"));
    let auto_count: usize = auto_res.parse().expect("valid integer from auto");
    assert!((1..=1024).contains(&auto_count));
    assert_eq!(run_with(Some("Auto")), auto_res);
    assert_eq!(run_with(Some("AUTO")), auto_res);
}

#[test]
fn native_build_launcher_supports_workers_auto() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native workers auto test: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-workers-auto");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "console.log(`workers-ok ${process.env.KILN_WORKER_INDEX}/${process.env.KILN_WORKER_COUNT}`);",
    )
    .expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let output = project.path.join("program");
    let options = NativeBuildOptions::new(output, NativeProfile::Dev);
    let report = project
        .builder(toolchain)
        .build_graph(&graph, &options)
        .expect("native build succeeds");

    // Run with KILN_WORKERS=auto
    let out = Command::new(&report.output)
        .env("KILN_WORKERS", "auto")
        .output_when_launchable()
        .expect("run native executable with KILN_WORKERS=auto");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("workers-ok"));

    // Every worker learns its index and the count before its realm starts, in both modes: a
    // thread-mode realm through its own `process.env`, a supervised process through its
    // environment (worker 0 is one of them: the supervisor runs no realm). A worker that
    // finishes with 0 is done and is not restarted.
    for mode in ["thread", "process", ""] {
        let out = Command::new(&report.output)
            .env("KILN_WORKERS", "3")
            .env("KILN_WORKERS_MODE", mode)
            .output_when_launchable()
            .expect("run native executable with KILN_WORKERS=3");
        assert!(out.status.success(), "{mode}: {out:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut lines: Vec<&str> = stdout.lines().collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            ["workers-ok 0/3", "workers-ok 1/3", "workers-ok 2/3"],
            "{mode}"
        );
    }

    // A single realm sets neither variable, so the program sees the environment node would.
    let out = Command::new(&report.output)
        .env_remove("KILN_WORKERS")
        .env_remove("KILN_WORKER_INDEX")
        .env_remove("KILN_WORKER_COUNT")
        .output_when_launchable()
        .expect("run native executable without KILN_WORKERS");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "workers-ok undefined/undefined"
    );
}

#[test]
fn native_build_records_a_worker_count_and_surfaces_a_worker_exit_status() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping recorded workers test: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-recorded-workers");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "console.log(`worker ${process.env.KILN_WORKER_INDEX}/${process.env.KILN_WORKER_COUNT}`);\n\
         if (process.env.KILN_WORKER_INDEX === '1') process.exit(7);",
    )
    .expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let options = NativeBuildOptions::new(project.path.join("program"), NativeProfile::Dev)
        .with_realm_policy(
            NativeRealmPolicy::default().with_workers(Some(NativeWorkers::Count(3))),
        );
    let report = project
        .builder(toolchain)
        .build_graph(&graph, &options)
        .expect("native build succeeds");

    // The recorded count applies with KILN_WORKERS unset. Worker 1 exits 7 on purpose: that is an
    // answer, not a crash, so it is not restarted and becomes the supervisor's status. Workers 0
    // and 2 may be stopped before they print, which is that same rule, so only worker 1 is read.
    let out = Command::new(&report.output)
        .env_remove("KILN_WORKERS")
        .env("KILN_WORKERS_MODE", "process")
        .output_when_launchable()
        .expect("run native executable with its recorded worker count");
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|line| line == "worker 1/3"),
        "{out:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("exited with status 7; stopping the other workers"),
        "{stderr}"
    );
    assert!(!stderr.contains("restarting"), "{stderr}");

    // Thread mode: `process.exit(7)` in realm 1 ends the process with 7.
    let out = Command::new(&report.output)
        .env_remove("KILN_WORKERS")
        .env("KILN_WORKERS_MODE", "thread")
        .output_when_launchable()
        .expect("run native executable in thread mode");
    assert_eq!(out.status.code(), Some(7), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|line| line == "worker 1/3"),
        "{out:?}"
    );

    // KILN_WORKERS overrides the recorded count: one realm, neither variable set.
    let out = Command::new(&report.output)
        .env("KILN_WORKERS", "1")
        .output_when_launchable()
        .expect("run native executable with KILN_WORKERS=1");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "worker undefined/undefined"
    );
}

#[test]
fn parse_http3_enabled_semantics() {
    assert!(!parse_http3_enabled(None, &[] as &[&str]));
    assert!(!parse_http3_enabled(Some(""), &[] as &[&str]));
    assert!(!parse_http3_enabled(Some("0"), &[] as &[&str]));
    assert!(parse_http3_enabled(Some("1"), &[] as &[&str]));
    assert!(parse_http3_enabled(Some("true"), &[] as &[&str]));

    // Argv flag "--http3"
    assert!(parse_http3_enabled(None, ["--http3"]));
    assert!(parse_http3_enabled(Some("0"), ["--http3"]));
    assert!(!parse_http3_enabled(None, ["--other", "arg"]));
}

#[test]
fn kiln_cli_http3_enabled_c_runtime_matches_contract() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native http3 test: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http3-c");
    let c_source = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static int kiln_cli_http3_enabled(int argc, char **argv) {
    const char *setting = getenv("KILN_HTTP3");
    int i;
    if (setting != NULL && *setting != '\0' && strcmp(setting, "0") != 0) return 1;
    for (i = 1; i < argc; ++i) {
        if (strcmp(argv[i], "--http3") == 0) return 1;
    }
    return 0;
}

int main(int argc, char **argv) {
    printf("%d\n", kiln_cli_http3_enabled(argc, argv));
    return 0;
}
"#;
    let src_file = project.path.join("test_http3.c");
    let obj_file = project.path.join("test_http3.o");
    let bin_file = project.path.join("test_http3");
    fs::write(&src_file, c_source).expect("write C source");

    toolchain
        .compile_c(&src_file, &obj_file, &CompileOptions::default())
        .expect("compile C probe");
    toolchain
        .link_executable(&[obj_file], &bin_file, &LinkOptions::default())
        .expect("link C probe");

    let run_with = |env_val: Option<&str>, args: &[&str]| -> String {
        let mut cmd = Command::new(&bin_file);
        for arg in args {
            cmd.arg(arg);
        }
        if let Some(val) = env_val {
            cmd.env("KILN_HTTP3", val);
        } else {
            cmd.env_remove("KILN_HTTP3");
        }
        let output = cmd.output_when_launchable().expect("run probe");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    assert_eq!(run_with(None, &[]), "0");
    assert_eq!(run_with(Some(""), &[]), "0");
    assert_eq!(run_with(Some("0"), &[]), "0");
    assert_eq!(run_with(Some("1"), &[]), "1");
    assert_eq!(run_with(Some("true"), &[]), "1");
    assert_eq!(run_with(None, &["--http3"]), "1");
    assert_eq!(run_with(Some("0"), &["--http3"]), "1");
    assert_eq!(run_with(None, &["--other"]), "0");
}

#[test]
fn native_build_launcher_supports_http3_flag() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native http3 test: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http3");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "console.log(process.argv.includes('--http3') ? 'http3-active' : 'no-http3');",
    )
    .expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let output = project.path.join("program");
    let options = NativeBuildOptions::new(output, NativeProfile::Dev);
    let report = project
        .builder(toolchain)
        .build_graph(&graph, &options)
        .expect("native build succeeds");

    // Run without flag
    let out_default = Command::new(&report.output)
        .output_when_launchable()
        .expect("run default");
    assert!(out_default.status.success());
    assert!(String::from_utf8_lossy(&out_default.stdout).contains("no-http3"));

    // Run with --http3
    let out_flag = Command::new(&report.output)
        .arg("--http3")
        .output_when_launchable()
        .expect("run with --http3");
    assert!(out_flag.status.success());
    assert!(String::from_utf8_lossy(&out_flag.stdout).contains("http3-active"));
}

#[test]
fn executable_host_refusal_preserves_bounded_diagnostics() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let toolchain = match Toolchain::discover() {
        Ok(toolchain) => toolchain,
        Err(ToolchainError::NoCompiler { .. }) => return,
        Err(error) => panic!("compiler probe failed: {error}"),
    };
    let project = TestDirectory::new("kiln-native-host-refusal-fs");
    let entry = project.path.join("main.mjs");
    fs::write(&entry, "console.log('ok');").expect("write fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let output = project.path.join("program");
    let options = NativeBuildOptions::new(output, NativeProfile::Dev);
    let report = project
        .builder(toolchain)
        .build_graph(&graph, &options)
        .expect("native build succeeds");

    let bad_result_path = project.path.join("nonexistent_dir").join("result.bin");
    let out = Command::new(&report.output)
        .env(
            kiln_native_runtime::NATIVE_RESULT_PATH_ENV,
            &bad_result_path,
        )
        .output_when_launchable()
        .expect("run executable with bad result path");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr, "kiln: the host refused an operation: filesystem access\n",
        "stderr should preserve filesystem access refusal diagnostic"
    );

    if Path::new("/dev/full").exists() {
        if let Ok(dev_full) = fs::OpenOptions::new().write(true).open("/dev/full") {
            let out_io = Command::new(&report.output)
                .stdout(dev_full)
                .output_when_launchable()
                .expect("run executable with full stdout");
            assert!(!out_io.status.success());
            let stderr_io = String::from_utf8_lossy(&out_io.stderr);
            assert_eq!(
                stderr_io, "kiln: the host refused an operation: terminal I/O\n",
                "stderr should preserve terminal I/O refusal diagnostic"
            );
        }
    }
}

#[test]
fn provider_capability_demand_is_typed_and_conservative() {
    use kiln_module_resolver::NodeRuntimeResolver;

    fn selection(entry_name: &str, files: &[(&str, &str)]) -> ProviderCapabilities {
        let project = TestDirectory::new("kiln-provider-variant-selection");
        for (name, source) in files {
            fs::write(project.path.join(name), source).expect("write selection fixture");
        }
        let graph = ModuleGraph::build_node(
            &project.path.join(entry_name),
            NodeRuntimeResolver::new(&project.path).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("selection graph");
        let demand = kiln_liveness::executable_demand(
            &graph,
            &kiln_liveness::SelectionOptions::development(),
        )
        .expect("selection demand");
        let members = kiln_graph::analyze_demanded_members(&graph, &demand.live, &demand.slots);
        kiln_liveness::retained_provider_capabilities(&graph, &demand, &members)
    }

    assert!(
        selection("main.mjs", &[("main.mjs", "console.log(1 + 2);\n")])
            == ProviderCapabilities::REALM,
        "a plain ESM entry needs only Realm"
    );
    assert!(
        selection(
            "main.mjs",
            &[
                (
                    "main.mjs",
                    "import { a } from './dep.mjs';\nconsole.log(a);\n"
                ),
                ("dep.mjs", "export const a = 1;\n"),
            ]
        ) == ProviderCapabilities::REALM,
        "an engine-free static graph needs only Realm"
    );
    assert_eq!(
        selection(
            "main.mjs",
            &[
                ("package.json", "{\"sideEffects\":false}\n"),
                (
                    "main.mjs",
                    "import { unused } from './dead.mjs';\nconsole.log(1);\n",
                ),
                ("dead.mjs", "export const unused = typeof Temporal;\n",),
            ],
        ),
        ProviderCapabilities::REALM,
        "a dead module does not enlarge provider requirements"
    );
    assert_eq!(
        selection(
            "main.mjs",
            &[
                (
                    "main.mjs",
                    "import { used } from './dep.mjs';\nconsole.log(used);\n",
                ),
                (
                    "dep.mjs",
                    "export const used = 1;\nexport function dead() { return [/x/, 1n, JSON.parse('1')]; }\n",
                ),
            ],
        ),
        ProviderCapabilities::REALM,
        "an unreachable function in a live module does not enlarge provider requirements"
    );
    assert_eq!(
        selection(
            "main.mjs",
            &[(
                "main.mjs",
                "import http from 'node:http';\nconsole.log(typeof http.createServer);\n",
            )]
        ),
        ProviderCapabilities::REALM.with(ProviderCapability::Http),
        "node:http declares HTTP demand"
    );
    assert!(
        selection(
            "main.mjs",
            &[(
                "main.mjs",
                "import https from 'node:https';\nconsole.log(typeof https);\n",
            )]
        ) == ProviderCapabilities::REALM.with(ProviderCapability::Http),
        "node:https declares HTTP demand"
    );
    assert!(
        selection(
            "main.mjs",
            &[(
                "main.mjs",
                "import { dlopen } from 'node:ffi';\nconsole.log(typeof dlopen);\n",
            )]
        ) == ProviderCapabilities::REALM.with(ProviderCapability::Ffi),
        "node:ffi declares FFI demand"
    );
    assert!(
        selection(
            "main.mjs",
            &[
                (
                    "main.mjs",
                    "import('./dep.mjs').then((m) => console.log(m.a));\n",
                ),
                ("dep.mjs", "export const a = 1;\n"),
            ]
        ) == ProviderCapabilities::FULL,
        "a dynamic import conservatively demands every capability"
    );
    assert!(
        selection(
            "main.cjs",
            &[
                (
                    "main.cjs",
                    "const dep = require('./dep.cjs');\nmodule.exports = dep;\n",
                ),
                ("dep.cjs", "module.exports = 1;\n"),
            ]
        ) == ProviderCapabilities::FULL,
        "a CommonJS graph conservatively demands every capability"
    );
    assert!(
        selection(
            "main.mjs",
            &[("main.mjs", "console.log(JSON.parse('1'));\n")]
        ) == ProviderCapabilities::REALM.with(ProviderCapability::Json),
        "JSON is independently demandable"
    );
    // The host GUI bridge is demanded by importing `kiln:gui`, and only then: `full` stops short
    // of it, so the conservative CommonJS answer has to add it back when the graph imports it
    // (ADR 0074).
    let gui = selection(
        "main.mjs",
        &[(
            "main.mjs",
            "import { currentHost } from 'kiln:gui';\nconsole.log(typeof currentHost);\n",
        )],
    );
    assert!(gui.contains(ProviderCapability::Gui), "{:?}", gui.names());
    assert_eq!(
        ProviderVariant::smallest_covering(gui),
        Some(ProviderVariant::FullGui),
        "a GUI program's engines are covered by the full-gui package"
    );
    assert_eq!(
        selection(
            "main.cjs",
            &[(
                "main.cjs",
                "const gui = require('kiln:gui');\nconsole.log(typeof gui);\n",
            )],
        ),
        ProviderCapabilities::FULL.with(ProviderCapability::Gui),
        "a CommonJS GUI program conservatively demands every capability, the bridge included"
    );
    for (source, capability) in [
        ("console.log(/a/.test('a'));\n", ProviderCapability::RegExp),
        ("console.log(1n + 2n);\n", ProviderCapability::BigInt),
        (
            "console.log(typeof Temporal);\n",
            ProviderCapability::Temporal,
        ),
        ("console.log(typeof Intl);\n", ProviderCapability::Intl),
        (
            "console.log([1].parallelMap);\n",
            ProviderCapability::Parallel,
        ),
    ] {
        assert_eq!(
            selection("main.mjs", &[("main.mjs", source)]),
            ProviderCapabilities::REALM.with(capability),
            "{} is represented as its own capability",
            capability.as_str()
        );
    }
}

/// Realms issue 08: `{ m() {} }.m` -- how the bootstrap writes a built-in that is a method -- is
/// the method alone, with no literal and no home-object cell to become garbage, unless the
/// method could reach the literal as its [[HomeObject]]: through `super` (in its body or an arrow
/// inside it) or a direct `eval`. A read of another key keeps the literal too.
#[test]
fn a_lone_method_read_back_from_its_literal_makes_no_object() {
    let count_objects = |source: &str| {
        let module =
            kiln_ir::compile_module(&kiln_frontend_oxc::parse("main.mjs", source)).unwrap();
        let literals = module
            .program
            .code
            .iter()
            .filter(|instruction| matches!(instruction, kiln_ir::Instruction::NewObject { .. }))
            .count();
        let methods = module
            .program
            .functions
            .iter()
            .filter(|function| function.is_method)
            .count();
        (literals, methods)
    };
    assert_eq!(
        count_objects("export const f = { getTime(x) { return x; } }.getTime;"),
        (0, 1)
    );
    assert_eq!(
        count_objects("export const f = { *g() { yield 1; } }.g;"),
        (0, 1)
    );
    assert_eq!(
        count_objects("export const f = { m() { return super.x; } }.m;"),
        (1, 1)
    );
    assert_eq!(
        count_objects("export const f = { m() { return () => super.x; } }.m;"),
        (1, 1)
    );
    assert_eq!(
        count_objects("export const f = { m() { return eval(\"1\"); } }.m;"),
        (1, 1)
    );
    assert_eq!(
        count_objects("export const f = { m() { return 1; } }.n;"),
        (1, 1)
    );
    assert_eq!(
        count_objects("export const f = { m() {}, n() {} }.m;"),
        (1, 2)
    );
    assert_eq!(
        count_objects("export const f = { m: function () {} }.m;"),
        (1, 0)
    );
}
