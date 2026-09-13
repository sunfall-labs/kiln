use super::*;
use kiln_ir::{
    BYTECODE_VERSION, Constant, ExportBinding, FunctionProto, HostOp, ImportBinding, Instruction,
    ModuleProgram,
};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn empty_function() -> FunctionProto {
    FunctionProto {
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        parameter_count: 0,
        declared_length: 0,
        mapped_arguments: false,
        local_count: 0,
        code: vec![Instruction::Return],
        upvalues: Vec::new(),
        type_annotations: None,
    }
}

#[test]
fn release_module_specialization_budget_counts_all_instructions() {
    let mut program = Program {
        source: String::new().into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![Instruction::Return; MAX_RELEASE_SPECIALIZED_MODULE_INSTRUCTIONS],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
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
        constants: Vec::new(),
        code: vec![Instruction::Return],
        local_count: 0,
        functions: vec![empty_function(); MAX_RELEASE_SPECIALIZED_MODULE_FUNCTIONS],
        template_sites: Vec::new(),
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

fn budget_module(program: Program) -> ModuleProgram {
    let mut module = kiln_ir::compile_module(&frontend_oxc::parse("main.mjs", "")).unwrap();
    module.program = program;
    module
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
    let mut module = kiln_ir::compile_module(&frontend_oxc::parse("main.mjs", &source)).unwrap();
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
    let huge = kiln_ir::compile_module(&frontend_oxc::parse("main.mjs", &huge_source)).unwrap();
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
fn inherited_classes_are_not_removable_declarations() {
    for source in [
        "class Unused extends Later {} class Later {}",
        "class Base {} Base = 3; class Unused extends Base {}",
        "class Base {} const Unused = class extends Base {};",
    ] {
        let module = kiln_ir::compile_module(&frontend_oxc::parse("main.mjs", source)).unwrap();
        assert_eq!(
            module.link_facts.removable_declarations.len(),
            1,
            "only the class without heritage is removable: {source}"
        );
    }
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
    let result = Command::new(report.output).output().unwrap();
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
    let sizes = executable_module_sizes(&graph, NativeProfile::Dev).expect("module sizes");
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
    let demand = executable_demand(&graph, NativeProfile::Dev).expect("demand");
    let members =
        kiln_backend_c::facts::analyze_demanded_members(&graph, &demand.live, &demand.slots);
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
    use kiln_backend_c::facts::{
        CORE_METHOD_GROUPS, PRUNABLE_METHOD_GROUPS, method_groups as groups,
    };
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
        let demand = executable_demand(&graph, NativeProfile::Dev).expect("demand");
        let members =
            kiln_backend_c::facts::analyze_demanded_members(&graph, &demand.live, &demand.slots);
        retained_method_groups(&graph, &demand, &members)
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
    ] {
        assert!(
            http_mask & group != 0,
            "node:http keeps its groups: {http_mask:#x}"
        );
    }
    assert_eq!(
        http_mask & groups::NODE_PATH,
        0,
        "unrelated groups still drop"
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
    assert_eq!(
        fs_mask & PRUNABLE_METHOD_GROUPS,
        groups::NODE_STREAM | groups::NODE_EVENT,
        "node:fs keeps only stream/event groups: {fs_mask:#x}"
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
}

struct OxcFrontend;

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
        include_str!("../../../conformance/native/compact-function-context.js"),
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
        .output()
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
        include_str!("../../../conformance/native/dead-phi-copies.js"),
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
        .output()
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

impl kiln_module_graph::Frontend for OxcFrontend {
    fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
        frontend_oxc::parse(path, source)
    }

    fn parse_commonjs(&mut self, path: &str, source: &str) -> kiln_module_graph::CommonJsParse {
        let (module, static_requires) = frontend_oxc::parse_commonjs_with_requires(path, source);
        kiln_module_graph::CommonJsParse {
            module,
            static_requires,
        }
    }
}

fn shared_test_cache_root() -> PathBuf {
    static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
    CACHE_DIR
        .get_or_init(|| {
            if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
                let target_cache = Path::new(&manifest_dir).join("../../target/kiln-test-cache");
                if fs::create_dir_all(&target_cache).is_ok() {
                    return target_cache;
                }
            }
            let fallback =
                std::env::temp_dir().join(format!("kiln-test-shared-cache-{}", std::process::id()));
            let _ = fs::create_dir_all(&fallback);
            fallback
        })
        .clone()
}

fn test_cache(local_root: impl Into<PathBuf>) -> ArtifactCache {
    let local = local_root.into();
    let shared = shared_test_cache_root();
    ArtifactCache::with_layers([
        kiln_artifact_cache::CacheLayer::writable(shared),
        kiln_artifact_cache::CacheLayer::writable(local),
    ])
}

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create native test directory");
        Self { path }
    }

    fn cache(&self) -> ArtifactCache {
        test_cache(self.path.join("cache"))
    }

    fn builder(&self, toolchain: Toolchain) -> NativeBuilder {
        NativeBuilder::new(self.cache(), toolchain)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn run_native_http_fixture(name: &str, source: &str) -> Option<String> {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:http fixture: no C compiler found");
        return None;
    };
    let project = TestDirectory::new(name);
    let entry = project.path.join("main.mjs");
    fs::write(&entry, source).expect("write node:http fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native node:http graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native node:http fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8(output.stdout).expect("fixture output is UTF-8"))
}

fn run_native_failure_fixture(name: &str, source: &str) -> Option<String> {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native diagnostic fixture: no C compiler found");
        return None;
    };
    let project = TestDirectory::new(name);
    let entry = project.path.join("main.mjs");
    fs::write(&entry, source).expect("write native diagnostic fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("diagnostic source graph compiles");
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native diagnostic graph builds");
    let output = Command::new(&report.output)
        .output()
        .expect("run native diagnostic fixture");
    assert!(!output.status.success(), "fixture unexpectedly succeeded");
    Some(String::from_utf8(output.stderr).expect("diagnostic output is UTF-8"))
}

#[test]
fn native_non_callable_diagnostic_keeps_call_site_and_callee_context() {
    let Some(stderr) = run_native_failure_fixture(
        "kiln-native-non-callable-diagnostic",
        "function invoke() { const value = {}; value(); }\ninvoke();\n",
    ) else {
        return;
    };
    assert!(
        stderr.contains("module=main.mjs"),
        "missing module: {stderr}"
    );
    assert!(
        stderr.contains("operation=Call"),
        "missing operation: {stderr}"
    );
    assert!(
        stderr.contains("detail=non-callable callee kind"),
        "missing value kind: {stderr}"
    );
    assert!(
        stderr.contains("function=invoke"),
        "missing owning function: {stderr}"
    );
    assert!(
        stderr.contains("receiver_tag=9"),
        "missing offending value tag: {stderr}"
    );
}

#[test]
fn native_non_callable_caught_error_does_not_leak_into_later_throw() {
    let Some(stderr) = run_native_failure_fixture(
        "kiln-native-non-callable-diagnostic-reset",
        "function caught() { try { const value = {}; value(); } catch (_) {} }\nfunction boom() { throw new Error('boom'); }\ncaught();\nboom();\n",
    ) else {
        return;
    };
    assert!(
        stderr.contains("detail=Call invocation threw"),
        "stale non-callable detail: {stderr}"
    );
    assert!(
        !stderr.contains("function=caught"),
        "caught context leaked: {stderr}"
    );
}

#[test]
fn owned_result_wrapper_source_is_deterministic_and_has_no_static_result_state() {
    let source = owned_result_wrapper_source(
        "kiln_probe",
        "scalar",
        "kiln_probe(&proxy_host, &value)",
        "",
    );
    assert_eq!(
        source,
        owned_result_wrapper_source(
            "kiln_probe",
            "scalar",
            "kiln_probe(&proxy_host, &value)",
            "",
        )
    );
    for fragment in [
        "kiln_probe_api_v2",
        "kiln_module_api_v2",
        "kiln_result_v2",
        "kiln_v2_scalar_kiln_probe_run",
        "kiln_v2_scalar_kiln_probe_read",
        "kiln_v2_scalar_kiln_probe_dispose",
        "failure_status",
        "*out = NULL",
        "memcpy(copy, value.ptr, value.len)",
    ] {
        assert!(
            source.contains(fragment),
            "missing generated fragment: {fragment}"
        );
    }
    assert!(!source.contains("static kiln_result_v2"));
    assert!(!source.contains("static kiln_value result"));
}

fn environment_digest(kind: CompilerKind, entries: &[(&str, &str)]) -> Digest {
    let values = entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), std::ffi::OsString::from(value)))
        .collect::<std::collections::HashMap<_, _>>();
    compiler_environment_digest_with(kind, |name| values.get(name).cloned())
}

#[test]
fn largest_first_compilation_preserves_results_and_error_precedence() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let observed = parking_lot::Mutex::new(Vec::new());
    let result = pool
        .install(|| {
            compile_largest_first(&[1, 20, 3, 20], "priority test", |index| {
                observed.lock().push(index);
                Ok(index)
            })
        })
        .unwrap();
    assert_eq!(*observed.lock(), vec![1, 3, 2, 0]);
    assert_eq!(result, vec![0, 1, 2, 3]);
    let failure = pool
        .install(|| {
            compile_largest_first(&[1, 20, 3, 20], "priority test", |index| {
                if index == 0 || index == 3 {
                    return Err(NativeError::NumberProvider {
                        message: format!("index {index}"),
                    });
                }
                Ok(index)
            })
        })
        .unwrap_err();
    assert!(matches!(failure, NativeError::NumberProvider { message } if message == "index 0"));
    let failure = compile_largest_first(&[1, 20, 3], "priority panic", |index| {
        assert_ne!(index, 1, "deliberate test panic");
        Ok(index)
    })
    .unwrap_err();
    assert!(matches!(
        failure,
        NativeError::WorkerPanic {
            stage: "priority panic"
        }
    ));
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
fn one_slow_index_does_not_hold_the_indices_behind_it() {
    let workers = rayon::current_num_threads();
    if workers < 2 {
        // Nothing to steal from on a single-core machine.
        return;
    }
    let count = workers * 4;
    // Index zero blocks until every other index has finished. Under a static partition its
    // chunk-mates could never run, so this would time out; claiming indices one at a time lets
    // the other workers drain the whole rest of the space while zero waits.
    let progress = (std::sync::Mutex::new(0usize), std::sync::Condvar::new());
    let progress_ref = &progress;
    let observed = compile_indexed(count, "test compile", move |index| {
        let (finished, signal) = progress_ref;
        if index == 0 {
            let mut finished = finished.lock().expect("progress lock");
            while *finished < count - 1 {
                let (guard, timeout) = signal
                    .wait_timeout(finished, Duration::from_secs(20))
                    .expect("progress lock");
                finished = guard;
                if timeout.timed_out() {
                    break;
                }
            }
            return Ok(*finished);
        }
        *finished.lock().expect("progress lock") += 1;
        signal.notify_all();
        Ok(0)
    })
    .expect("every index compiles");
    assert_eq!(
        observed[0],
        count - 1,
        "every other index should finish while index zero is blocked"
    );
}

#[test]
fn async_realm_contract_is_in_runtime_not_application_identity() {
    // Shape, not value. This was a pinned literal, and on 2026-08-31 alone it went 3 -> 5
    // -> 15 -> 17: three intended bumps, three red baselines, and not one accidental bump
    // caught. A tripwire that fires only on the change it is meant to permit is friction,
    // and the repair -- editing the literal -- teaches nothing and is indistinguishable
    // from rubber-stamping a mistake.
    //
    // What actually protects the caches is that the identity *has* every component, since a
    // missing one silently makes two different ABIs look alike. A bump is then visible in
    // the diff of the constant it comes from, where a reviewer can see what moved.
    let identity = module_abi_identity();
    for component in ["host-", "c-", "generator-"] {
        assert!(
            identity.contains(component),
            "module ABI identity {identity:?} has no `{component}` component; a missing one \
                 makes two different ABIs hash alike and every cached artifact is then suspect"
        );
    }
    assert!(runtime_abi_identity().contains("async-6"));
    // Same reasoning as above, and the same history: this pin was 14, then 15, then 18, and
    // is 23 as this is written -- all intended, none caught by being pinned.
    // What matters is that the provider's version reaches the identity at all: if it did not,
    // an archive built before an ABI change would keep being linked against emitted C that
    // expects the change, which is exactly the "undefined symbols from a stale archive"
    // failure this repository loses time to.
    let provider = native_number_abi_identity();
    let version = provider
        .rsplit_once('-')
        .map(|(_, version)| version)
        .unwrap_or_default();
    assert!(
        provider.starts_with("kiln-native-number-provider-") && version.parse::<u32>().is_ok(),
        "provider identity {provider:?} must end in the ABI version, or a stale archive and \
             a fresh one hash alike"
    );
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

    assert_eq!(
        identities.len(),
        52,
        "seven generated-source families, one number provider, and eleven stage families across four profiles"
    );

    let tool = builder.toolchain().identity();
    let target = target_identity(tool);
    let compiler = compiler_identity(tool);
    for profile in [
        CacheProfile::Debug,
        CacheProfile::Check,
        CacheProfile::Release,
        CacheProfile::Profile,
    ] {
        for identity in [
            cache_identity(
                compiler.clone(),
                Backend::C,
                &module_abi_identity(),
                &target,
                profile.clone(),
            ),
            cache_identity(
                compiler.clone(),
                Backend::Custom("native-runtime".to_owned()),
                &runtime_abi_identity(),
                &target,
                profile.clone(),
            ),
            cache_identity(
                compiler.clone(),
                Backend::Custom("native-shared-graph".to_owned()),
                &runtime_abi_identity(),
                &target,
                profile,
            ),
        ] {
            assert!(identities.contains(&identity));
        }
    }
}

#[test]
fn typed_node_querystring_has_a_native_materializer() {
    use kiln_module_resolver::{NodeBuiltin, NodeRequestKind, PinnedPackageResolver};

    let project = TestDirectory::new("native-node-builtin-gate");
    let entry = project.path.join("main.mjs");
    fs::write(&entry, "export const marker = 1;\n").expect("write entry");
    let builtin_path = NodeBuiltin::QueryString.virtual_path(&project.path);
    let entry_program = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(1_f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["node:querystring".to_owned()],
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "marker".to_owned(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let provider_entry = fs::canonicalize(&entry).expect("canonical entry");
    let graph = ModuleGraph::build_with_typed_policy(
        &entry,
        PinnedPackageResolver::new_with_node_builtins_requests_and_formats(
            &project.path,
            [(
                entry.clone(),
                "node:querystring".to_owned(),
                NodeRequestKind::Import,
                builtin_path,
            )],
            [],
        )
        .expect("node resolver"),
        move |path, source| {
            assert_eq!(path, provider_entry);
            assert_eq!(source, ModuleSource::Source);
            Ok(kiln_module_graph::ModulePayload::Program(
                entry_program.clone(),
            ))
        },
    )
    .expect("typed node graph");

    validate_native_node_builtins(&graph).expect("querystring materializes");
    let body = native_builtin_body(NodeBuiltin::QueryString).expect("materializer body");
    assert!(body.contains("KILN_C99_METHOD_NODE_QUERYSTRING_PARSE"));
    for export in ["decode", "default", "parse"] {
        assert!(native_builtin_supports_export(
            NodeBuiltin::QueryString,
            export
        ));
    }
}

#[test]
fn native_node_querystring_parses_form_data_and_preserves_namespace_identity() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-querystring");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let entry = root.path.join("main.mjs");
    fs::write(
            &entry,
            r#"import querystring, { decode, parse } from "node:querystring";
const query = parse("hello=world&foo=b==ar&repeat=1&repeat=2&a+b=c+d&check=%E2%9C%93&bad=%ZZ&__proto__=safe");
const edges = decode("&&missing&=empty-key&empty-value=&short=%&half=%A&invalid=%C0&broken=%E2%28%A1");
console.log(parse === decode, querystring.parse === parse, query.hello, query.foo, query.repeat.join(","), query["a b"], query.check, query.bad, query.__proto__);
console.log(edges.missing === "", edges[""], edges["empty-value"] === "", edges.short, edges.half, edges.invalid, edges.broken);"#,
        )
        .expect("write querystring consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build querystring graph");

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native querystring integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let run = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native querystring graph builds and runs");
    assert_eq!(
        run.stdout,
        "true true world b==ar 1,2 c d ✓ %ZZ safe\ntrue empty-key true % %A � �(�\n".as_bytes()
    );
}

#[test]
fn node_events_has_a_native_runtime_module_materializer() {
    use kiln_module_resolver::NodeBuiltin;

    let module = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::Return],
            local_count: 2,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![
            ExportBinding {
                exported: "EventEmitter".to_owned(),
                local_slot: 0,
            },
            ExportBinding {
                exported: "default".to_owned(),
                local_slot: 1,
            },
        ],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let node = kiln_module_graph::ModuleNode {
        path: PathBuf::from("/.kiln/builtins/events.builtin"),
        source: ModuleSource::NodeBuiltin(NodeBuiltin::Events),
        program: module,
        data: None,
        dependencies: Vec::new(),
    };
    let emitted = emit_native_graph_module(
        &node,
        ".kiln/builtins/events.builtin",
        &NativeRealmPolicy::default(),
    )
    .expect("events materializes");

    assert!(
        emitted
            .source
            .contains("KILN_C99_METHOD_NODE_EVENT_EMITTER")
    );
    assert!(emitted.source.contains("exports[0] = constructor;"));
    assert!(emitted.source.contains("exports[1] = constructor;"));
    assert!(native_builtin_body(NodeBuiltin::Https).is_some());
    assert!(kiln_builtins::is_ts_builtin(NodeBuiltin::Path));
    assert!(native_builtin_supports_export(NodeBuiltin::Path, "join"));
    assert!(kiln_builtins::is_ts_builtin(NodeBuiltin::Assert));
    assert!(native_builtin_supports_export(NodeBuiltin::Assert, "ok"));
}

#[test]
fn node_net_has_one_native_runtime_module_materializer() {
    use kiln_module_resolver::NodeBuiltin;

    let body = native_builtin_body(NodeBuiltin::Net).expect("net materializes");
    assert!(body.contains("KILN_C99_METHOD_NODE_NET_CREATE_SERVER"));
    assert!(body.contains("exports[0] = connect;"));
    assert!(body.contains("exports[1] = connect;"));
    assert!(body.contains("exports[2] = create_server;"));
    assert!(native_builtin_body(NodeBuiltin::Https).is_some());
    assert!(kiln_builtins::is_ts_builtin(NodeBuiltin::Path));
    assert!(native_builtin_supports_export(NodeBuiltin::Path, "resolve"));
}

#[test]
fn node_stream_and_perf_hooks_have_native_runtime_module_materializers() {
    use kiln_module_resolver::NodeBuiltin;

    let stream = native_builtin_body(NodeBuiltin::Stream).expect("stream materializes");
    assert!(stream.contains("KILN_C99_METHOD_NODE_READABLE"));
    assert!(stream.contains("KILN_C99_METHOD_NODE_WRITABLE"));
    assert!(stream.contains("KILN_C99_METHOD_NODE_STREAM_FINISHED"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Stream,
        "addAbortSignal"
    ));

    // The realm's own `performance` builtin rather than a second object built here: node makes
    // `require("node:perf_hooks").performance` and `globalThis.performance` one value, and two
    // objects made a library's "have I already installed my shim" check answer wrong. `now`
    // comes off that builtin's table, so the method code is no longer named in this body.
    let performance = native_builtin_body(NodeBuiltin::PerfHooks).expect("perf_hooks materializes");
    assert!(performance.contains("kiln_c99_builtin(KILN_C99_BUILTIN_PERFORMANCE)"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::PerfHooks,
        "performance"
    ));
}

#[test]
fn native_process_next_tick_forwards_variadic_arguments() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native process.nextTick integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-process-next-tick");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"process.nextTick((left, right) => console.log(left + right), 19, 23);
"#,
    )
    .expect("write process.nextTick fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native process.nextTick graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native process.nextTick fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"42\n");
}

#[test]
fn native_node_fs_promises_read_write_stat_and_encoding() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:fs/promises integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-fs-promises");
    let entry = project.path.join("main.mjs");
    let data = project.path.join("data.txt");
    let malformed = project.path.join("malformed.bin");
    let missing = project.path.join("missing.txt");
    let path = data.to_string_lossy();
    let malformed_path = malformed.to_string_lossy();
    let missing_path = missing.to_string_lossy();
    fs::write(
        &entry,
        format!(
            r#"import {{ readFile, stat, writeFile }} from 'node:fs/promises';
writeFile({path:?}, 'héllo').then(() => readFile({path:?}, 'utf8')).then((text) => {{
  console.log(text);
  return readFile({path:?});
}}).then((bytes) => {{
  console.log(Buffer.isBuffer(bytes));
  console.log(bytes.length);
  return stat({path:?});
}}).then((info) => {{
  console.log(info.size);
  console.log(info.isFile());
  console.log(info.isDirectory());
  console.log(info['@@kiln:node:is-file'] === undefined);
  Object.defineProperty(info, '@@kiln:node:is-file', {{ value: false }});
  console.log(info.isFile());
  delete info['@@kiln:node:is-file'];
  console.log(info.isFile());
  console.log(Object.hasOwn(info, '@@kiln:node:is-file') === false);
  console.log(('@@kiln:node:is-file' in info) === false);
  console.log(Object.getOwnPropertyDescriptor(info, '@@kiln:node:is-file') === undefined);
  return writeFile({malformed_path:?}, new Uint8Array([0xe2, 0x82]));
}}).then(() => readFile({malformed_path:?}, 'utf8')).then((text) => {{
  console.log(text === '\ufffd');
  return readFile({missing_path:?});
}}).then(() => console.log('unexpected'), (error) => {{
  console.log(error.code);
  console.log(error.syscall);
  console.log(error.path === {missing_path:?});
}});
"#,
            path = path,
            malformed_path = malformed_path,
            missing_path = missing_path
        ),
    )
    .expect("write node:fs/promises fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native node:fs/promises graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native node:fs/promises fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
            output.stdout,
            "héllo\ntrue\n6\n6\ntrue\nfalse\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\nENOENT\nopen\ntrue\n".as_bytes()
        );
}

#[test]
fn node_http_native_materializer_matches_the_authoritative_export_order() {
    use kiln_module_resolver::NodeBuiltin;

    // These indices were six slots wide and started at `Server`, which is the layout that
    // shipped and did not work: the authoritative list is eight names beginning
    // `IncomingMessage, METHODS, Server`, so every slot sat two low. `default` resolved to
    // `request` and `typeof http` answered "function" where node answers "object";
    // `createServer` resolved to `get`, so the first real server refused with "a function is
    // not a request options object". Node is the arbiter on both and the corrected layout
    // Slot 1 held `undefined` too until `METHODS` was materialized. It is a constant array,
    // so the alignment slot it used to be became a wrong answer once the CommonJS projection
    // started publishing a stand-in callable for every unbuilt name: `typeof http.METHODS`
    // answered "function" against the interpreter's "object".
    let body = native_builtin_body(NodeBuiltin::Http).expect("http materializes");
    assert!(body.contains("exports[0] = kiln_c99_undefined();"));
    assert!(body.contains("exports[1] = methods;"));
    assert!(body.contains("exports[2] = server;"));
    assert!(body.contains("exports[3] = server_response;"));
    assert!(body.contains("exports[4] = create_server;"));
    assert!(body.contains("exports[5] = namespace;"));
    assert!(body.contains("exports[6] = get;"));
    assert!(body.contains("exports[7] = request;"));
    assert!(body.contains("KILN_C99_METHOD_NODE_HTTP_SERVER"));
    assert!(body.contains("KILN_C99_METHOD_NODE_HTTP_SERVER_RESPONSE"));
    assert!(body.contains("KILN_C99_METHOD_NODE_HTTP_CREATE_SERVER"));
    assert!(native_builtin_supports_export(NodeBuiltin::Http, "Server"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Http,
        "ServerResponse"
    ));
}

#[test]
fn native_node_http_exports_distinct_server_and_create_server_names() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-callable-shape",
        r#"import http from "node:http";
if (typeof http.Server !== "function") throw new Error("Server is not callable");
if (typeof http.createServer !== "function") throw new Error("createServer is not callable");
if (http.Server === http.createServer) throw new Error("HTTP callables share identity");
if (http.Server.name !== "Server") throw new Error("wrong Server name");
if (http.createServer.name !== "createServer") throw new Error("wrong createServer name");
console.log("pass");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "pass\n");
}

#[test]
fn native_node_http_server_accepts_options_overload_without_listener() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-options-overload",
        r#"import http from "node:http";
const optionOnly = new http.Server({ allowHalfOpen: true });
const nullOptions = new http.Server(null);
const withListener = new http.Server({}, () => {});
const factoryOptionOnly = http.createServer({});
const factoryWithListener = http.createServer({}, () => {});
for (const server of [optionOnly, nullOptions, withListener, factoryOptionOnly, factoryWithListener]) {
  if (typeof server.close !== "function") throw new Error("missing close");
  server.close();
}
console.log("pass");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "pass\n");
}

#[test]
fn native_node_http_server_exposes_timeout_policy_used_by_frameworks() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-server-timeouts",
        r#"import http from "node:http";
const server = http.createServer(() => {});
console.log(server.timeout, server.keepAliveTimeout, server.requestTimeout);
server.setTimeout(12.9);
console.log(server.timeout);
console.log("pass");
"#,
    ) else {
        return;
    };
    assert_eq!(output, "0 5000 300000\n12\npass\n");
}

#[test]
fn native_node_http_response_exposes_node_lifecycle_state() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-response-state",
        r#"import http from "node:http";
const server = http.createServer((request, response) => {
  console.log(response.statusCode, response.headersSent, response.writableEnded, response.destroyed);
  response.statusCode = 201;
  response.writeHead(response.statusCode, { "content-type": "text/plain" });
  console.log(response.statusCode, response.headersSent, response.writableEnded);
  response.end("ok");
  console.log(response.headersSent, response.writableEnded);
  server.close();
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
    ) else {
        return;
    };
    assert_eq!(output, "200 false false false\n201 true false\ntrue true\n");
}

#[test]
fn native_node_http_response_headers_are_case_insensitive_and_snapshot() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-response-headers",
        r#"import http from "node:http";
const server = http.createServer((_request, response) => {
  response.setHeader("Content-Type", "text/plain");
  console.log(response.getHeader("content-type"), response.hasHeader("CONTENT-TYPE"));
  const headers = response.getHeaders();
  headers["content-type"] = "changed";
  console.log(response.getHeader("content-type"));
  response.removeHeader("CONTENT-TYPE");
  console.log(response.hasHeader("content-type"), response.getHeader("content-type"));
  response.end("ok");
  server.close();
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
    ) else {
        return;
    };
    assert_eq!(output, "text/plain true\ntext/plain\nfalse undefined\n");
}

#[test]
fn native_node_http_incoming_message_exposes_request_lifecycle_state() {
    let Some(output) = run_native_http_fixture(
        "kiln-native-node-http-request-state",
        r#"import http from "node:http";
const server = http.createServer((request, response) => {
  console.log(request.httpVersion, request.httpVersionMajor, request.httpVersionMinor);
  console.log(request.aborted, request.complete, request.destroyed);
  request.on("end", () => {
    console.log(request.complete);
    response.end("ok");
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => {
  const request = http.request({ host: "127.0.0.1", port: server.address().port, path: "/" }, (response) => {
    response.on("data", () => {});
    response.on("end", () => {});
  });
  request.end();
});
"#,
    ) else {
        return;
    };
    assert_eq!(output, "1.1 1 1\nfalse false false\ntrue\n");
}

#[test]
fn native_node_http_server_rejects_invalid_options_with_a_node_code() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:http option validation: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http-invalid-options");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import http from "node:http";
for (const option of ["foo", 42, true, []]) {
  let thrown;
  try { new http.Server(option); } catch (error) { thrown = error; }
  if (!(thrown instanceof TypeError)) throw new Error("missing TypeError");
  if (thrown.code !== "ERR_INVALID_ARG_TYPE") throw new Error("missing code");
}
console.log("pass");
"#,
    )
    .expect("write invalid-options fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native invalid-options graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native invalid-options fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "pass\n");
}

#[test]
fn native_node_net_source_graph_builds_through_the_production_frontend() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:net integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-net-source");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { createServer } from "node:net";
const server = createServer((socket) => {
  socket.on("data", (chunk) => {
    console.log(chunk);
    socket.end();
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
    )
    .expect("write node:net fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    assert!(graph.modules().any(|node| matches!(
        node.source,
        ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Net)
    )));
    let builder = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain);
    let report = builder
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native node:net graph builds");
    assert!(report.output.exists());

    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("start native node:net server");
    let stdout = child.stdout.take().expect("capture server stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read ephemeral listening port");
    let port: u16 = listening
        .trim()
        .parse()
        .expect("server prints numeric port");
    assert_ne!(port, 0);
    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect loopback client");
    client
        .write_all(b"native loopback")
        .expect("write client bytes");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("finish client write");
    let mut ignored = Vec::new();
    client
        .read_to_end(&mut ignored)
        .expect("server closes client");
    let status = child.wait().expect("server exits after close");
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(status.success(), "server failed: {stderr}");
    assert_eq!(rest, "native loopback\n");
}

#[test]
fn native_node_net_client_sends_final_bytes_and_reads_after_half_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:net client integration: no C compiler found");
        return;
    };
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback server");
    let port = listener.local_addr().expect("read loopback address").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept native client");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read through client EOF");
        assert_eq!(request, "final bytes");
        stream
            .write_all(b"reply")
            .expect("write reply after half-close");
    });
    let project = TestDirectory::new("kiln-native-node-net-client");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        format!(
            r#"import {{ connect }} from "node:net";
const socket = connect({port}, "127.0.0.1", () => {{
  if (socket.readyState !== "open") throw new Error("socket was not open");
  socket.end("final bytes");
  if (socket.readyState !== "readOnly") throw new Error("socket was not read-only");
}});
let reply = "";
socket.setEncoding("utf8");
socket.on("data", chunk => reply += chunk);
socket.on("end", () => {{}});
socket.on("close", () => {{
  if (socket.readyState !== "closed") throw new Error("socket was not closed");
  console.log(reply);
}});
"#
        ),
    )
    .expect("write node:net client fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("client"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native node:net client builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native node:net client");
    server.join().expect("loopback server completes");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"reply\n");
}

#[test]
fn native_node_http_server_responds_after_node_net_client_half_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP half-close integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-half-close");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        include_str!("../../../conformance/node/upstream-adapted/http-server-allow-half-open.mjs"),
    )
    .expect("write HTTP half-close fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native HTTP half-close fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native HTTP half-close fixture");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll native fixture") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill stuck native fixture");
            break child.wait().expect("reap stuck native fixture");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("capture stdout")
        .read_to_string(&mut stdout)
        .expect("read stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(
        status.success(),
        "status={status} stdout={stdout:?} stderr={stderr:?}"
    );
    assert_eq!(stdout, "pass\n");
}

#[test]
fn native_node_http_server_handles_two_pipelined_requests_on_one_socket() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP pipelining integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-pipelining");
    let reservation = TcpListener::bind(("127.0.0.1", 0)).expect("reserve loopback port");
    let port = reservation.local_addr().expect("read reserved port").port();
    drop(reservation);
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        format!(
            r#"import {{ createServer }} from "node:http";
let requests = 0;
const server = createServer((request, response) => {{
  console.log(request.url);
  response.end(request.url);
  requests += 1;
  if (requests === 2) server.close();
}});
server.listen({port}, "127.0.0.1");
"#
        ),
    )
    .expect("write HTTP pipelining fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native HTTP pipelining fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native HTTP pipelining fixture");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut client = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(client) => break client,
            Err(error) if Instant::now() < deadline => {
                if let Some(status) = child.try_wait().expect("poll native server") {
                    let mut stderr = String::new();
                    child
                        .stderr
                        .take()
                        .expect("capture stderr")
                        .read_to_string(&mut stderr)
                        .expect("read stderr");
                    panic!("server exited before listen: {status}; stderr={stderr:?}");
                }
                let _ = error;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                child.kill().expect("kill server stuck before listen");
                child.wait().expect("reap server stuck before listen");
                panic!("server did not listen before deadline: {error}");
            }
        }
    };
    client
        .set_nonblocking(true)
        .expect("make HTTP client nonblocking");
    client
            .write_all(
                b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\nGET /second HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .expect("write pipelined requests");
    let mut response = String::new();
    let mut chunk = [0_u8; 4096];
    let response_deadline = Instant::now() + Duration::from_secs(10);
    while !response.contains("/second") {
        match client.read(&mut chunk) {
            Ok(0) => break,
            Ok(length) => {
                response.push_str(&String::from_utf8_lossy(&chunk[..length]));
                if response.len() > 64 * 1024 {
                    child.kill().expect("kill server repeating response bytes");
                    child.wait().expect("reap server repeating response bytes");
                    let prefix = &response[..response.floor_char_boundary(512)];
                    panic!(
                        "HTTP bridge repeated response bytes; length={} prefix={prefix:?}",
                        response.len()
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= response_deadline {
                    child.kill().expect("kill server stuck during responses");
                    child.wait().expect("reap server stuck during responses");
                    panic!("timed out reading pipelined responses; partial={response:?}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                panic!("read both pipelined responses: {error}; partial response={response:?}")
            }
        }
    }
    let first = response.find("/first").unwrap_or_else(|| {
        let status = child
            .try_wait()
            .expect("poll server missing first response")
            .unwrap_or_else(|| {
                child.kill().expect("kill server missing first response");
                child.wait().expect("reap server missing first response")
            });
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        panic!("first response body missing: {response:?}; status={status}; stderr={stderr:?}")
    });
    let second = response.find("/second").unwrap_or_else(|| {
        child.kill().expect("kill server missing second response");
        child.wait().expect("reap server missing second response");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        panic!("second response body missing: {response:?}; stderr={stderr:?}")
    });
    assert!(
        first < second,
        "responses preserve request order: {response:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll native server exit") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill server stuck after responses");
            break child.wait().expect("reap server stuck after responses");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut rest = String::new();
    child
        .stdout
        .take()
        .expect("capture stdout")
        .read_to_string(&mut rest)
        .expect("read stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(
        status.success(),
        "status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert_eq!(rest, "/first\n/second\n");
}

#[test]
fn native_node_http_source_graph_serves_one_live_tokio_request() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native node:http integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-node-http-source");
    let entry = project.path.join("main.mjs");
    fs::write(
            &entry,
            r#"import { createServer } from "node:http";
const server = createServer((request, response) => {
  console.log(request.method, request.url, request.httpVersion);
  if (!request.socket || request.connection !== request.socket) throw new Error("request socket");
  if (response.req !== request) throw new Error("response request");
  if (response.socket !== request.socket || response.connection !== request.socket) throw new Error("response socket");
  let body = "";
  request.on("data", chunk => body += chunk);
  request.on("end", () => {
    console.log(body);
    response.setHeader("Content-Type", "text/plain");
    response.writeHead(201);
    response.write("echo:");
    response.end(body);
    server.close();
  });
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
        )
        .expect("write node:http fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    assert!(graph.modules().any(|node| matches!(
        node.source,
        ModuleSource::NodeBuiltin(kiln_module_resolver::NodeBuiltin::Http)
    )));
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native node:http graph builds");

    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("start native node:http server");
    let stdout = child.stdout.take().expect("capture server stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read ephemeral listening port");
    let port: u16 = listening
        .trim()
        .parse()
        .expect("server prints numeric port");
    assert_ne!(port, 0);

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .expect("set HTTP client timeout");
    client
            .write_all(
                b"POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: 6\r\nConnection: close\r\n\r\nabc",
            )
            .expect("write first HTTP request fragment");
    client
        .write_all(b"123")
        .expect("write second HTTP request fragment");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("finish HTTP request");
    let mut response = String::new();
    let read_result = client.read_to_string(&mut response);
    if let Err(error) = read_result {
        let status = child.wait().expect("server exits after failed HTTP read");
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .expect("capture stderr")
            .read_to_string(&mut stderr)
            .expect("read stderr");
        panic!(
            "HTTP read failed: {error}; status={status}; response={response:?}; stderr={stderr:?}"
        );
    }
    assert!(
        response.starts_with("HTTP/1.1 201 Created\r\n"),
        "{response:?}"
    );
    let response_lower = response.to_ascii_lowercase();
    assert!(
        response_lower.contains("content-type: text/plain\r\n"),
        "{response:?}"
    );
    assert!(
        response_lower.contains("connection: close\r\n"),
        "{response:?}"
    );
    assert!(
        response.ends_with("\r\n5\r\necho:\r\n6\r\nabc123\r\n0\r\n\r\n"),
        "{response:?}"
    );

    let status = child
        .wait()
        .expect("server exits after listener and socket close");
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read request callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");
    assert!(status.success(), "server failed: {stderr}");
    assert_eq!(rest, "POST /echo 1.1\nabc123\n");
}

#[test]
#[ignore = "native Fastify constructor still reaches an unowned Call refusal"]
fn native_fastify_server_case_runs_through_the_production_graph_when_installed() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native Fastify integration: no C compiler found");
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/server/packages");
    if !root.join("node_modules/fastify/fastify.js").is_file() {
        eprintln!("skipping native Fastify integration: packages are not installed");
        return;
    }
    let entry = root.join("s04-fastify.mjs");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root).expect("Node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("Fastify source graph compiles");
    let project = TestDirectory::new("kiln-native-fastify");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native Fastify graph builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native Fastify graph");
    let deadline = Instant::now() + Duration::from_secs(15);
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().expect("poll native Fastify graph") {
            break (status, false);
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill stuck native Fastify graph");
            break (child.wait().expect("reap stuck native Fastify graph"), true);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .expect("capture Fastify stdout")
        .read_to_string(&mut stdout)
        .expect("read Fastify stdout");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture Fastify stderr")
        .read_to_string(&mut stderr)
        .expect("read Fastify stderr");
    assert!(
        !timed_out,
        "native Fastify graph timed out; stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        status.success(),
        "status={status}; stdout={stdout:?}; stderr={stderr:?}"
    );
    assert_eq!(
        stdout,
        "200 {\"ok\":true}\n200 {\"id\":\"7\"}\n200 {\"got\":{\"hello\":\"kiln\"}}\n404 {\"error\":\"not found\"}\n"
    );
    assert!(stderr.is_empty(), "stderr={stderr:?}");
}

/// Serving a request and *reading the reply* are two different claims, and the corpus only
/// covered the first. `setEncoding` is the seam: it is the one method name a `net.Socket`, an
/// `http.IncomingMessage` and a `stream.Readable` all own, so the compiled dispatch has to
/// pick a handler from the receiver rather than from a category. When it picked by category
/// instead, every `res.setEncoding("utf8")` threw `a object is not a Socket` -- with the whole
/// live-request suite still green, because none of it ever read a response as text.
///
/// The assertion is node's observable, not the absence of a throw: after `setEncoding("utf8")`
/// a `data` chunk is a string, and `typeof` says so.
#[test]
fn native_node_http_client_response_set_encoding_delivers_text_chunks() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native setEncoding integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-client-set-encoding");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import http from "node:http";
const server = http.createServer((request, response) => {
  response.writeHead(200, { "content-type": "text/plain" });
  response.end("kiln");
});
server.listen(0, "127.0.0.1", () => {
  const { port } = server.address();
  http.get({ host: "127.0.0.1", port, path: "/" }, (response) => {
    response.setEncoding("utf8");
    let body = "";
    let kinds = "";
    response.on("data", chunk => { kinds += typeof chunk; body += chunk; });
    response.on("end", () => {
      console.log(response.statusCode, response.headers["content-type"], kinds, body);
      server.close();
    });
  });
});
"#,
    )
    .expect("write node:http setEncoding fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("client"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native node:http setEncoding graph builds");
    let output = Command::new(&report.output)
        .output_when_launchable()
        .expect("run native node:http setEncoding program");
    assert!(
        output.status.success(),
        "status={} stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Recorded from node v25: `200 text/plain string kiln`.
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "200 text/plain string kiln\n"
    );
}

#[test]
fn native_node_http_server_close_waits_for_delayed_response_after_half_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP delayed-close integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-delayed-close");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { createServer } from "node:http";
let socket_closes = 0;
const server = createServer((request, response) => {
  console.log("request");
  request.socket.on("close", () => {
    socket_closes += 1;
    console.log(`socket-close:${socket_closes}`);
  });
  setTimeout(() => { console.log("timer"); response.end("delayed"); }, 0);
  console.log("before-close");
  server.close(() => console.log("server-close"));
  console.log("after-close", request.socket.readyState);
});
server.httpAllowHalfOpen = true;
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
    )
    .expect("write delayed-close fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native delayed-close fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native delayed-close fixture");
    let stdout = child.stdout.take().expect("capture server stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read delayed-close listening port");
    let port: u16 = listening
        .trim()
        .parse()
        .expect("server prints numeric port");
    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set HTTP client timeout");
    client
        .write_all(b"GET /delayed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write HTTP request");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close HTTP request");
    let mut response = String::new();
    if let Err(error) = client.read_to_string(&mut response) {
        let _ = child.kill();
        let status = child
            .wait()
            .expect("reap delayed-close server after read failure");
        panic!("read delayed HTTP response: {error}; status={status}; response={response:?}");
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll delayed-close server exit") {
            break status;
        }
        if Instant::now() >= deadline {
            child
                .kill()
                .expect("kill delayed-close server after timeout");
            break child
                .wait()
                .expect("reap delayed-close server after timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read delayed-close callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture server stderr")
        .read_to_string(&mut stderr)
        .expect("read server stderr");
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
    );
    // The body arrives whole with `end`, so it is length-declared rather than chunked. What
    // this test pins is the ordering - the response completes before the close - not the
    // framing, so both facts are asserted separately.
    assert!(
        response.contains("\r\ncontent-length: 7\r\n"),
        "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert!(
        response.ends_with("\r\n\r\ndelayed"),
        "response={response:?} status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert!(status.success(), "server failed: {stderr}");
    assert_eq!(rest.matches("socket-close:").count(), 1, "stdout={rest:?}");
    assert_eq!(
        rest,
        "request\nbefore-close\nafter-close open\ntimer\nsocket-close:1\nserver-close\n"
    );
}

#[test]
fn native_node_http_socket_destroy_uses_abort_close() {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::io::{BufRead, Read, Write};
    use std::net::TcpStream;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native HTTP destroy integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-http-destroy");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { createServer } from "node:http";
const server = createServer((request, response) => {
  console.log("destroy");
  request.socket.destroy();
  server.close(() => console.log("closed"));
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#,
    )
    .expect("write HTTP destroy fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let report = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("server"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native HTTP destroy fixture builds");
    let mut child = Command::new(&report.output)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .expect("run native HTTP destroy fixture");
    let stdout = child.stdout.take().expect("capture destroy stdout");
    let mut stdout = std::io::BufReader::new(stdout);
    let mut listening = String::new();
    stdout
        .read_line(&mut listening)
        .expect("read destroy listening port");
    let port: u16 = listening.trim().parse().expect("numeric destroy port");
    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect HTTP client");
    client
        .write_all(b"GET /destroy HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write destroy request");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll destroy server") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill destroy server after timeout");
            break child.wait().expect("reap destroy server after timeout");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut rest = String::new();
    stdout
        .read_to_string(&mut rest)
        .expect("read destroy callback output");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("capture destroy stderr")
        .read_to_string(&mut stderr)
        .expect("read destroy stderr");
    assert!(
        status.success(),
        "status={status} stdout={rest:?} stderr={stderr:?}"
    );
    assert_eq!(rest, "destroy\nclosed\n");
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
fn successful_private_runs_require_one_well_formed_scalar_result() {
    let root = TestDirectory::new("kiln-native-result");
    let result_path = root.path.join("result.bin");

    // An absent file after a successful run is `None`, not an error. The result carries only
    // the five boundary scalar tags, and a module's completion value is under no obligation to
    // be one of them -- a `--node` entry completes with its `module.exports`, an object. The
    // launcher declines to write what it cannot represent, and declining is not failing: it
    // used to charge that refusal to the program's exit status, so a program that printed
    // correctly exited 1. Unrepresentable is absent, which is what the `Option` already meant.
    assert_eq!(decode_run_result(&result_path, true).unwrap(), None);
    assert_eq!(decode_run_result(&result_path, false).unwrap(), None);

    fs::write(&result_path, b"not-a-result").expect("write malformed result");
    assert!(matches!(
        decode_run_result(&result_path, true),
        Err(NativeError::RunResult { .. })
    ));

    let expected = HostValue::String("line one\nline two\0".to_owned());
    fs::write(
        &result_path,
        kiln_native_runtime::encode_native_result(&expected),
    )
    .expect("write valid result");
    assert_eq!(
        decode_run_result(&result_path, true).expect("decode valid result"),
        Some(expected)
    );
}

#[test]
fn shared_publication_serializes_each_header_library_pair() {
    let root = TestDirectory::new("kiln-native-shared-publication");
    let output = root.path.join("program.so");
    let loader = root.path.join("kiln-content.so");
    let header = shared_library_header_path(&output);
    materialize(&header, b"old header", false).expect("publish old header");
    materialize(&output, b"old library", false).expect("publish old library");

    let process_lock = SHARED_PUBLICATION_LOCK.lock();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let writer_output = output.clone();
    let writer_loader = loader.clone();
    let writer = std::thread::spawn(move || {
        entered_tx.send(()).expect("announce waiting writer");
        let result = publish_shared_artifacts(
            &writer_output,
            &writer_loader,
            b"new header",
            b"new library",
        );
        finished_tx.send(result).expect("report publication");
    });
    entered_rx.recv().expect("writer starts");
    assert!(
        finished_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "a second publisher must wait for the pair lock"
    );
    assert_eq!(fs::read(&header).expect("read old header"), b"old header");
    assert_eq!(fs::read(&output).expect("read old library"), b"old library");

    drop(process_lock);
    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("writer completes after unlock")
        .expect("publication succeeds");
    writer.join().expect("writer exits");
    assert_eq!(fs::read(&header).expect("read new header"), b"new header");
    assert_eq!(fs::read(&output).expect("read new library"), b"new library");
    assert_eq!(
        fs::read(&loader).expect("read loader library"),
        b"new library"
    );

    let collision = publish_shared_artifacts(&output, &loader, b"other header", b"other library")
        .expect_err("a canonical content name is immutable");
    assert!(matches!(
        collision,
        NativeError::ImmutableArtifactCollision { .. }
    ));
    assert_eq!(fs::read(header).unwrap(), b"new header");
    assert_eq!(fs::read(&output).unwrap(), b"new library");

    #[cfg(unix)]
    {
        fs::remove_file(&loader).expect("remove regular loader");
        std::os::unix::fs::symlink(&output, &loader).expect("create matching loader symlink");
        let error = publish_shared_artifacts(&output, &loader, b"new header", b"new library")
            .expect_err("a symlink cannot stand in for an immutable loader");
        assert!(matches!(
            error,
            NativeError::ImmutableArtifactCollision { .. }
        ));
        assert!(
            fs::symlink_metadata(&loader)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
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
fn shared_loader_paths_canonicalize_and_reserve_the_loader_namespace() {
    let root = TestDirectory::new("kiln-native-loader-path");
    let real = root.path.join("real");
    fs::create_dir(&real).expect("create real output directory");
    let identity = Digest::hash(b"loader-path-test");

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&real, root.path.join("alias"))
            .expect("create output-directory symlink");
        let loader = shared_library_loader_path(&root.path.join("alias/program.so"), identity)
            .expect("derive loader through symlink");
        let canonical_real = fs::canonicalize(&real).expect("canonical real directory");
        assert_eq!(
            loader.parent(),
            Some(canonical_real.join(".kiln-loaders").as_path())
        );
    }

    let reserved = real.join(".kiln-loaders");
    fs::create_dir(&reserved).expect("create reserved loader directory");
    assert!(matches!(
        shared_library_loader_path(&reserved.join("program.so"), identity),
        Err(NativeError::ArtifactPathCollision { .. })
    ));
    let nested = reserved.join("nested");
    fs::create_dir(&nested).expect("create nested reserved directory");
    assert!(matches!(
        shared_library_loader_path(&nested.join("program.so"), identity),
        Err(NativeError::ArtifactPathCollision { .. })
    ));
    assert!(matches!(
        shared_library_loader_path(&real.join(".kiln-loaders"), identity),
        Err(NativeError::ArtifactPathCollision { .. })
    ));
}

#[test]
fn graph_emission_cache_hits_persist_and_all_input_classes_invalidate() {
    let project = TestDirectory::new("kiln-native-emission-cache");
    let graph = one_module_graph(&project.path, 1.0, "value");
    let node = graph.module(graph.entry()).unwrap();
    let policy = EmissionPolicy {
        decoupled_prelude: true,
        compact: true,
        ..Default::default()
    };
    let cache_path = project.path.join("emission-cache");
    let cache = ArtifactCache::new(&cache_path);
    // Release selection can change generic_only after the input cache key is computed.
    let selected_policy = EmissionPolicy {
        generic_only: !policy.generic_only,
        ..policy.clone()
    };
    let expected = emit_native_graph_module_with_policy(node, "main.js", &selected_policy).unwrap();
    let (first, first_hit) =
        cached_graph_module(&cache, "test-target", node, "main.js", &policy, || {
            Ok(expected.clone())
        })
        .unwrap();
    assert!(!first_hit);
    assert_eq!(first, expected);
    // A fresh cache object must hit without entering the emitter at all.
    let reopened = ArtifactCache::new(&cache_path);
    let (cached, cache_hit) =
        cached_graph_module(&reopened, "test-target", node, "main.js", &policy, || {
            panic!("unchanged module was emitted again")
        })
        .unwrap();
    assert!(cache_hit);
    assert_eq!(cached, expected);

    let mut cases = Vec::new();
    let mut changed = node.clone();
    changed.program.program.constants[0] = Constant::Number(2.0_f64.to_bits());
    cases.push((changed, policy.clone(), "main.js", "test-target"));
    let mut changed = node.clone();
    changed.program.program.source = "// changed observable source".into();
    cases.push((changed, policy.clone(), "main.js", "test-target"));
    let mut changed = node.clone();
    changed.program.strictness.body = !changed.program.strictness.body;
    cases.push((changed, policy.clone(), "main.js", "test-target"));
    let mut changed = node.clone();
    changed.program.exports[0].exported = "renamed".into();
    cases.push((changed, policy.clone(), "main.js", "test-target"));
    let mut changed = node.clone();
    changed
        .program
        .link_facts
        .immutable_bindings
        .push("value".into());
    cases.push((changed, policy.clone(), "main.js", "test-target"));
    let mut changed = node.clone();
    changed.source = ModuleSource::CommonJs;
    cases.push((changed, policy.clone(), "main.js", "test-target"));
    cases.push((node.clone(), policy.clone(), "other.js", "test-target"));
    cases.push((node.clone(), policy.clone(), "main.js", "other-target"));
    for change in 0..10 {
        let mut changed = policy.clone();
        match change {
            0 => changed.compact = false,
            1 => changed.host_clock = true,
            2 => changed.entropy = kiln_backend_c::EmissionEntropyPolicy::Seeded(3),
            3 => changed.timezone = kiln_backend_c::EmissionStringPolicy::Named("UTC".into()),
            4 => changed.locale = kiln_backend_c::EmissionStringPolicy::Host,
            5 => changed.type_profile_record = true,
            6 => changed.type_profile = Some(Arc::new(Default::default())),
            7 => changed.live_module_slots = Some(Arc::new(BTreeSet::new())),
            8 => changed.live_module_slots = Some(Arc::new(BTreeSet::from([0]))),
            9 => changed.generic_only = true,
            _ => unreachable!(),
        }
        cases.push((node.clone(), changed, "main.js", "test-target"));
    }
    for (index, (node, policy, path, target)) in cases.iter().enumerate() {
        let calls = std::cell::Cell::new(0);
        let (actual, cache_hit) = cached_graph_module(&cache, target, node, path, policy, || {
            calls.set(calls.get() + 1);
            Ok(expected.clone())
        })
        .unwrap();
        assert!(!cache_hit, "input case {index} reported a cache hit");
        assert_eq!(calls.get(), 1, "input case {index} reused stale output");
        assert_eq!(actual, expected);
    }
    let mut embedded = policy.clone();
    embedded.decoupled_prelude = false;
    let calls = std::cell::Cell::new(0);
    for _ in 0..2 {
        cached_graph_module(&cache, "test-target", node, "main.js", &embedded, || {
            calls.set(calls.get() + 1);
            Ok(expected.clone())
        })
        .unwrap();
    }
    assert_eq!(
        calls.get(),
        2,
        "embedded runtime sources must bypass this cache"
    );
}

fn one_module_graph(project: &Path, value: f64, export: &str) -> ModuleGraph {
    fs::create_dir_all(project).expect("create one-module graph project");
    fs::write(
        project.join("main.js"),
        format!("export const {export} = {value};\n"),
    )
    .expect("write one-module graph entry");
    let module = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(value.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::LoadLocal(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: export.to_owned(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    ModuleGraph::build_with_modules(project.join("main.js"), project, move |_| module.clone())
        .expect("construct one-module graph")
}

fn realm_scheduling_graph(project: &Path) -> ModuleGraph {
    fs::create_dir_all(project).expect("create scheduling graph project");
    for name in ["dep.js", "main.js"] {
        fs::write(project.join(name), "").expect("write scheduling graph module");
    }
    let dependency = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::String("dependency microtask".to_owned()),
                Constant::String("dependency timer".to_owned()),
                Constant::Number(0.0_f64.to_bits()),
                Constant::Undefined,
            ],
            code: vec![
                Instruction::LoadBuiltin(kiln_ir::Builtin::QueueMicrotask),
                Instruction::MakeFunction(1),
                Instruction::Call { argc: 1 },
                Instruction::Pop,
                Instruction::LoadBuiltin(kiln_ir::Builtin::SetTimeout),
                Instruction::MakeFunction(0),
                Instruction::Const(2),
                Instruction::Call { argc: 2 },
                Instruction::Pop,
                Instruction::Const(3),
                Instruction::Return,
            ],
            local_count: 0,
            functions: vec![
                FunctionProto {
                    span: kiln_ir::Span::default(),
                    prologue_end: 0,
                    asynchronous: false,
                    name: None,
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    is_method: false,
                    parameter_count: 0,
                    declared_length: 0,
                    mapped_arguments: false,
                    local_count: 0,
                    code: vec![
                        Instruction::Const(1),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 1,
                        },
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                    type_annotations: None,
                },
                FunctionProto {
                    span: kiln_ir::Span::default(),
                    prologue_end: 0,
                    asynchronous: false,
                    name: None,
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    is_method: false,
                    parameter_count: 0,
                    declared_length: 0,
                    mapped_arguments: false,
                    local_count: 0,
                    code: vec![
                        Instruction::Const(0),
                        Instruction::HostCall {
                            op: HostOp::WriteStdout,
                            argc: 1,
                        },
                        Instruction::Return,
                    ],
                    upvalues: Vec::new(),
                    type_annotations: None,
                },
            ],
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let entry = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![
                Constant::String("entry body".to_owned()),
                Constant::Undefined,
            ],
            code: vec![
                Instruction::Const(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Pop,
                Instruction::Const(1),
                Instruction::Return,
            ],
            local_count: 0,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["./dep.js".to_owned()],
        imports: Vec::new(),
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let dependency_path = fs::canonicalize(project.join("dep.js")).expect("canonical dependency");
    ModuleGraph::build_with_modules(project.join("main.js"), project, move |path| {
        if path == dependency_path {
            dependency.clone()
        } else {
            entry.clone()
        }
    })
    .expect("construct scheduling graph")
}

#[test]
fn one_realm_defers_dependency_work_until_the_complete_native_graph_finishes() {
    let root = TestDirectory::new("kiln-native-realm-graph");
    let graph = realm_scheduling_graph(&root.path.join("project"));
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native Realm graph integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let run = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native scheduling graph runs");
    assert_eq!(
        run.stdout,
        b"entry body\ndependency microtask\ndependency timer\n"
    );
}

/// A compiled binary is a single-executable application in node's sense, so it reports the
/// command line the way node does for one: `argv[0]` and `argv[1]` both name the executable,
/// and the program's own arguments start at index 2. That is not a cosmetic choice -- every
/// argument-reading program, and `const [, , first] = process.argv`, counts from there, and a
/// one-element argv would silently hand each of them the wrong argument.
///
/// Run through `Command` rather than `run_graph`, because the whole question is what the OS
/// put in `main`'s `argv` and there is no other way to put anything there.
#[test]
fn native_process_argv_reports_the_single_executable_command_line() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native process.argv integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-process-argv");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"console.log(JSON.stringify([
  process.argv.length,
  process.argv[0] === process.argv[1],
  process.execPath === process.argv[0],
  process.argv.slice(2),
]));
"#,
    )
    .expect("write process.argv fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("process.argv source compiles");
    let output = project.path.join("program");
    NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: output.clone(),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("process.argv graph builds natively");

    let bare = std::process::Command::new(&output)
        .output_when_launchable()
        .expect("run the compiled program");
    assert_eq!(bare.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&bare.stdout),
        "[2,true,true,[]]\n",
        "stderr: {}",
        String::from_utf8_lossy(&bare.stderr)
    );

    let passed = std::process::Command::new(&output)
        .args(["a", "b", "c"])
        .output_when_launchable()
        .expect("run the compiled program with arguments");
    assert_eq!(passed.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&passed.stdout),
        "[5,true,true,[\"a\",\"b\",\"c\"]]\n",
        "stderr: {}",
        String::from_utf8_lossy(&passed.stderr)
    );
}

#[test]
fn native_process_next_tick_uses_the_realm_lane_before_microtasks() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native process.nextTick integration: no C compiler found");
        return;
    };
    let project = TestDirectory::new("kiln-native-process-next-tick");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        r#"const order = [];
const nextTick = process.nextTick;
order.push(`${delete process.nextTick}:${process.nextTick === undefined}`);
Promise.resolve().then(function () { order.push("promise"); });
nextTick(function (first, second) {
  order.push(`${first}:${second}:${this === undefined}`);
  nextTick(function () { order.push("nested"); });
  queueMicrotask(function () { order.push("micro-from-tick"); });
}, "a", 2);
queueMicrotask(function () { order.push("micro"); });
setTimeout(function () { console.log(order.join(",")); }, 0);
"#,
    )
    .expect("write process.nextTick fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("process.nextTick source compiles");
    let run = NativeBuilder::new(test_cache(project.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("process.nextTick graph runs natively");
    assert_eq!(run.exit_code, Some(0));
    assert_eq!(
        run.stdout,
        b"true:true,a:2:true,nested,promise,micro,micro-from-tick\n"
    );
    assert!(run.stderr.is_empty());
}

#[test]
fn embedded_graph_owns_one_realm_through_async_quiescence() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping embedded Realm graph integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping embedded Realm graph integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-embedded-realm-graph");
    let graph = realm_scheduling_graph(&root.path.join("project"));
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone());
    let library = builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(if cfg!(target_os = "macos") {
                    "program.dylib"
                } else {
                    "program.so"
                }),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build embedded scheduling graph");
    let package = shared_publication::read_shared_library_package(&library.output)
        .expect("read graph shared-library package");
    assert_eq!(
        package.manifest().legacy_entry_symbol,
        library.module_symbol
    );
    assert_eq!(package.manifest().v2_api_symbol, library.module_api_symbol);
    let header_name = library
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("shared graph header name");
    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <stdio.h>

static int write_host_call(void *context, uint16_t operation, uint16_t argc,
                           const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != KILN_HOST_OP_WRITE_STDOUT || argc != 1 || arguments == NULL ||
        result == NULL || arguments[0].tag != KILN_VALUE_STRING ||
        (arguments[0].len != 0 && arguments[0].ptr == NULL)) return KILN_C99_HOST_ERROR;
    if (fwrite(arguments[0].ptr, 1, arguments[0].len, stdout) != arguments[0].len ||
        fputc('\n', stdout) == EOF) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{ write_host_call, NULL }};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 1;
}}
"#,
            symbol = library.module_symbol,
        ),
    )
    .expect("write embedded scheduling host");
    let host_object = root.path.join("host.o");
    let mut compile = CompileOptions::default();
    compile.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile)
        .expect("compile embedded scheduling host");
    let host_executable = root.path.join("host");
    let loader_directory = library.loader.parent().expect("loader directory");
    let mut link = LinkOptions::default();
    link.runtime_libraries.push(library.output.clone());
    link.extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link)
        .expect("link embedded scheduling host");
    let mut command = Command::new(host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run embedded scheduling graph");
    assert!(
        output.status.success(),
        "embedded scheduling graph failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"entry body\ndependency microtask\ndependency timer\n"
    );
}

fn star_reexport_graph(project: &Path) -> ModuleGraph {
    fs::create_dir_all(project).expect("create star graph project");
    for name in ["origin.js", "barrel.js", "main.js"] {
        fs::write(project.join(name), "").expect("write star graph module");
    }
    let origin = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(42.0_f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "answer".into(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let barrel = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Undefined],
            code: vec![Instruction::Const(0), Instruction::Return],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["./origin.js".into()],
        imports: vec![ImportBinding {
            source: "./origin.js".into(),
            imported: kiln_ir::NAMESPACE_IMPORT.into(),
            local: "*reexport0*".into(),
            local_slot: 0,
        }],
        exports: vec![ExportBinding {
            exported: format!("{}./origin.js", kiln_ir::STAR_REEXPORT_PREFIX),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let entry = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Undefined],
            code: vec![
                Instruction::LoadLocal(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Pop,
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["./barrel.js".into()],
        imports: vec![ImportBinding {
            source: "./barrel.js".into(),
            imported: "answer".into(),
            local: "answer".into(),
            local_slot: 0,
        }],
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let origin_path = fs::canonicalize(project.join("origin.js")).expect("canonical origin");
    let barrel_path = fs::canonicalize(project.join("barrel.js")).expect("canonical barrel");
    ModuleGraph::build_with_modules(project.join("main.js"), project, move |path| {
        if path == origin_path {
            origin.clone()
        } else if path == barrel_path {
            barrel.clone()
        } else {
            entry.clone()
        }
    })
    .expect("construct star re-export graph")
}

fn namespace_import_graph(project: &Path) -> ModuleGraph {
    fs::create_dir_all(project).expect("create namespace graph project");
    for name in ["origin.js", "main.js"] {
        fs::write(project.join(name), "").expect("write namespace graph module");
    }
    let origin = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(42.0_f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "answer".into(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let entry = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::String("answer".into())],
            code: vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["./origin.js".into()],
        imports: vec![ImportBinding {
            source: "./origin.js".into(),
            imported: kiln_ir::NAMESPACE_IMPORT.into(),
            local: "namespace".into(),
            local_slot: 0,
        }],
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let origin_path = fs::canonicalize(project.join("origin.js")).expect("canonical origin");
    ModuleGraph::build_with_modules(project.join("main.js"), project, move |path| {
        if path == origin_path {
            origin.clone()
        } else {
            entry.clone()
        }
    })
    .expect("construct namespace import graph")
}

#[test]
fn native_graph_wiring_uses_resolved_star_export_origins() {
    let root = TestDirectory::new("kiln-native-star-graph");
    let graph = star_reexport_graph(&root.path.join("project"));
    let modules = graph
        .modules()
        .map(|node| {
            (
                node,
                kiln_backend_c::emit_module(&node.program).expect("module emits"),
            )
        })
        .collect::<Vec<_>>();

    let launcher = graph_launcher_source(&graph, &modules, &NativeRealmPolicy::default())
        .expect("launcher emits");
    assert!(launcher.contains("kiln_imports_2[0] = kiln_exports_0[0];"));
    assert!(!launcher.contains("kiln_imports_1[0] ="));
    assert_eq!(launcher.matches("kiln_realm_create(&realm)").count(), 1);
    assert_eq!(launcher.matches("kiln_realm_destroy(realm)").count(), 1);
    assert_eq!(
        launcher
            .matches("kiln_realm_enter(realm, kiln_graph_enter, &graph)")
            .count(),
        1
    );
    assert!(!launcher.contains("kiln_graph_enter_module"));
    let bootstrap = native_builtin_bootstrap_module(&NativeRealmPolicy::default().emission())
        .expect("builtin bootstrap emits");
    let bootstrapped = graph_launcher_source_with_imports(
        &graph,
        &modules,
        &NativeRealmPolicy::default(),
        None,
        None,
        None,
        Some(&bootstrap),
    )
    .expect("bootstrapped launcher emits");
    let bootstrap_call = bootstrapped
        .find(&format!("status = {}(", bootstrap.symbol))
        .expect("launcher calls the bootstrap");
    let reset = bootstrapped
        .find("kiln_c99_builtin_store_epoch = 0;")
        .expect("launcher seals bootstrap writes before user code");
    assert!(bootstrap_call < reset);
    // ADR 0044: only a host-clock launcher switches the realm to wall time, and it does so
    // between creating the realm and entering it.
    assert!(!launcher.contains("kiln_realm_use_wall_clock(realm)"));
    let wall = graph_launcher_source(&graph, &modules, &NativeRealmPolicy::host_clock())
        .expect("wall-clock launcher emits");
    let switch = wall
        .find("kiln_realm_use_wall_clock(realm)")
        .expect("the host-clock launcher switches the clock");
    assert!(wall.find("kiln_realm_create(&realm)").unwrap() < switch);
    assert!(
        switch
            < wall
                .find("kiln_realm_enter(realm, kiln_graph_enter, &graph)")
                .unwrap()
    );
    let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_star_graph")
        .expect("shared wrapper emits");
    assert!(wrapper.contains("import_arrays[2][0] = export_arrays[0][0];"));
    assert!(!wrapper.contains("import_arrays[1][0] ="));
    assert!(wrapper.contains("import_arrays[1] = (kiln_value *)calloc"));

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native star integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let run = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native star graph runs");
    assert_eq!(run.stdout, b"42\n");
    builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native shared star graph links");
}

#[test]
fn native_graph_reserves_commonjs_wrapper_bindings_for_the_launcher() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-wrapper");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir(root.path.join("pkg")).expect("create package directory");
    fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
        .expect("write CommonJS package");
    fs::write(
        root.path.join("pkg/index.js"),
        "console.log(__filename, __dirname); module.exports = 42;",
    )
    .expect("write CommonJS module");
    fs::write(root.path.join("pkg/dep.js"), "module.exports = 41;")
        .expect("write CommonJS dependency");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import answer from './pkg/index.js'; console.log(answer);",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build mixed graph");
    let modules = graph
        .modules()
        .map(|node| {
            (
                node,
                emit_native_graph_module(
                    node,
                    &logical_module_path(graph.project_root(), &node.path).expect("logical path"),
                    &NativeRealmPolicy::default(),
                )
                .expect("module emits"),
            )
        })
        .collect::<Vec<_>>();

    let launcher = graph_launcher_source(&graph, &modules, &NativeRealmPolicy::default())
        .expect("launcher reserves wrappers");
    let commonjs_index = modules
        .iter()
        .position(|(node, _)| node.source == ModuleSource::CommonJs)
        .expect("CommonJS module");
    let commonjs_path = modules[commonjs_index].0.path.to_string_lossy();
    let commonjs_dir = modules[commonjs_index]
        .0
        .path
        .parent()
        .expect("CommonJS module has a parent")
        .to_string_lossy();
    let logical_commonjs =
        logical_module_path(graph.project_root(), &modules[commonjs_index].0.path)
            .expect("logical CommonJS path");
    assert!(launcher.contains("static kiln_value kiln_imports_0[5];"));
    assert!(launcher.contains("_commonjs_wrapper(kiln_imports_0, (size_t)5, "));
    assert!(launcher.contains(&format!(
            "{}_commonjs_wrapper(kiln_imports_{commonjs_index}, (size_t)5, {}, (size_t){}, {}, (size_t){}",
            modules[commonjs_index].1.symbol,
            native_c_string(&commonjs_path),
            commonjs_path.len(),
            native_c_string(&commonjs_dir),
            commonjs_dir.len(),
        )));
    assert!(launcher.contains(&format!(
        "kiln_graph_diagnostic(\"commonjs-wrapper\", {})",
        native_c_string(&logical_commonjs)
    )));
    assert_eq!(
        launcher.matches("_commonjs_wrapper(kiln_imports_").count(),
        1
    );
    let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_commonjs_graph")
        .expect("shared wrapper emits CommonJS support");
    let context = wrapper
        .find("typedef struct kiln_commonjs_context")
        .expect("shared wrapper declares CommonJS context");
    let loader = wrapper
        .find("static int kiln_graph_load_commonjs")
        .expect("shared wrapper declares CommonJS loader");
    assert!(context < loader, "context must precede its first use");
    assert!(wrapper.contains(&native_c_string(&commonjs_path)));
    assert!(wrapper.contains(&native_c_string(&commonjs_dir)));

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS execution: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let report = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native mixed graph builds and runs");
    assert_eq!(
        report.stdout,
        format!("{commonjs_path} {commonjs_dir}\n42\n").as_bytes()
    );
    builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native shared CommonJS graph builds and links");

    fs::write(
        root.path.join("pkg/index.js"),
        "module.exports = require('./dep.js') + 1;",
    )
    .expect("rewrite CommonJS module with require");
    let require_graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS require graph");
    let required = builder
        .run_graph(
            &require_graph,
            &NativeBuildOptions {
                output: root.path.join("required-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native CommonJS require graph builds and runs");
    assert_eq!(required.stdout, b"42\n");

    fs::write(
            root.path.join("pkg/index.js"),
            "const first = require('./dep.js'); const second = require('./dep.js'); console.log(first === second, first.loads); module.exports = 42;",
        )
        .expect("rewrite CommonJS module with repeated require");
    fs::write(
            root.path.join("pkg/dep.js"),
            "globalThis.__kilnLoads = (globalThis.__kilnLoads || 0) + 1; module.exports = { loads: globalThis.__kilnLoads };",
        )
        .expect("write stateful CommonJS dependency");
    let cached_graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build repeated CommonJS require graph");
    let cached = builder
        .run_graph(
            &cached_graph,
            &NativeBuildOptions {
                output: root.path.join("cached-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native repeated CommonJS require graph builds and runs");
    assert_eq!(cached.stdout, b"true 1\n42\n");
}

#[test]
fn native_commonjs_require_receives_the_builtin_projection_on_both_graph_paths() {
    use kiln_module_resolver::{NodeBuiltin, NodeRuntimeResolver};

    let root = TestDirectory::new("kiln-native-commonjs-builtin-projection");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir(root.path.join("pkg")).expect("create package directory");
    fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
        .expect("write CommonJS package");
    fs::write(
        root.path.join("pkg/index.js"),
        "const { format } = require('node:util'); module.exports = format('%s:%d', 'ok', 7);",
    )
    .expect("write CommonJS builtin consumer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import answer from './pkg/index.js'; console.log(answer);",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS builtin graph");
    let modules = graph
        .modules()
        .map(|node| {
            (
                node,
                emit_native_graph_module(
                    node,
                    &logical_module_path(graph.project_root(), &node.path).expect("logical path"),
                    &NativeRealmPolicy::default(),
                )
                .expect("module emits"),
            )
        })
        .collect::<Vec<_>>();
    let util_index = modules
        .iter()
        .position(|(node, _)| node.source == ModuleSource::NodeBuiltin(NodeBuiltin::Util))
        .expect("util module");
    let commonjs_index = modules
        .iter()
        .position(|(node, _)| node.source == ModuleSource::CommonJs)
        .expect("CommonJS module");
    let default_index = native_commonjs_projection_index(modules[util_index].0)
        .expect("builtin CommonJS projection");
    let expected_standalone =
        format!("*result = kiln_exports_{util_index}[{default_index}]; return 0;");
    let expected_shared =
        format!("*result = graph->exports[{util_index}][{default_index}]; return KILN_C99_OK;");
    let launcher = graph_launcher_source(&graph, &modules, &NativeRealmPolicy::default())
        .expect("launcher emits");
    assert!(launcher.contains(&expected_standalone));
    let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_builtin_projection")
        .expect("shared wrapper emits");
    assert!(wrapper.contains(&expected_shared));
    assert!(launcher.contains(&format!("kiln_commonjs_require_{commonjs_index}")));

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native builtin projection integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let run = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("standalone graph runs");
    assert_eq!(run.stdout, b"ok:7\n");
    builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("shared graph builds");
}

/// A CommonJS entry completes with `module.exports`, which is an object unless the module
/// assigns a scalar to it. The result file carries only the five boundary scalars, so the
/// launcher declines to write an object -- and used to charge that refusal to the program,
/// which exited 1 after printing exactly the right bytes and saying nothing on stderr. The
/// run succeeded and simply has no scalar to report.
#[test]
fn native_commonjs_object_exports_finish_successfully_with_no_scalar_result() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-object-exports-exit");
    fs::write(root.path.join("package.json"), r#"{"type":"commonjs"}"#)
        .expect("write package metadata");
    let entry = root.path.join("main.js");
    fs::write(&entry, "console.log(\"ok\");").expect("write CommonJS entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS exit-status integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let report = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native CommonJS graph builds and runs");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"ok\n");
    assert_eq!(
        report.result, None,
        "an object result is an absent one here"
    );
}

#[test]
fn native_commonjs_callable_exports_cross_module_boundaries() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-callable-export");
    fs::write(root.path.join("package.json"), r#"{"type":"commonjs"}"#)
        .expect("write package metadata");
    fs::write(
        root.path.join("callee.js"),
        "module.exports = function answer(value) { return value + 1 }",
    )
    .expect("write callable CommonJS provider");
    let entry = root.path.join("main.js");
    fs::write(
            &entry,
            "const answer = require('./callee.js'); console.log(answer(41)); module.exports = answer(1);",
        )
        .expect("write callable CommonJS consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS callable graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS callable integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let report = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native CommonJS callable graph builds and runs");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"42\n");
}

#[test]
fn native_runtime_callable_properties_are_realm_wide_across_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-realm-callable-properties");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    fs::write(
        root.path.join("writer.mjs"),
        r#"import { Buffer } from "node:buffer";
Buffer.realmMarker = { answer: 42 };
export { Buffer };"#,
    )
    .expect("write callable-property producer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { Buffer as written } from "./writer.mjs";
import { Buffer as read } from "node:buffer";
console.log(written === read, read.realmMarker.answer);"#,
    )
    .expect("write callable-property consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build realm callable-property graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping realm callable-property integration: no C compiler found");
        return;
    };
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native realm callable-property graph builds and runs");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&report.stdout),
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"true 42\n");
}

#[test]
fn native_realm_singletons_are_shared_across_generated_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-realm-singletons");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    fs::write(
        root.path.join("writer.mjs"),
        r#"globalThis.realmMarker = 41;
process.realmMarker = 42;
Atomics.realmMarker = 43;
export const ready = true;"#,
    )
    .expect("write singleton producer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { ready } from "./writer.mjs";
console.log(ready, globalThis.realmMarker, process.realmMarker, Atomics.realmMarker);"#,
    )
    .expect("write singleton consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build realm-singleton graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping realm-singleton integration: no C compiler found");
        return;
    };
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native realm-singleton graph builds and runs");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&report.stdout),
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"true 41 42 43\n");
}

#[test]
fn native_commonjs_real_package_callable_executes_after_require() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let package_root = workspace.join("conformance/server/packages");
    // The probe imports a real package off disk, so it needs the conformance server's
    // dependencies installed. A fresh clone has no node_modules there and the resolver
    // raises a bare "cannot resolve" that reads like an engine bug; say what is missing.
    if !package_root.join("node_modules/safe-regex2").is_dir() {
        eprintln!(
            "skipping real CommonJS package integration: {} is missing. \
                 Run `npm ci` in {} to enable it.",
            package_root.join("node_modules/safe-regex2").display(),
            package_root.display()
        );
        return;
    }
    let entry = package_root.join(format!(
        ".kiln-native-safe-regex2-{}.mjs",
        std::process::id()
    ));
    fs::write(
        &entry,
        "import safe from 'safe-regex2'; console.log(safe(/a+/));",
    )
    .expect("write package probe");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&package_root).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build real package graph");
    let result = (|| {
        let toolchain = Toolchain::discover().ok()?;
        let cache = std::env::temp_dir().join(format!(
            "kiln-native-safe-regex-cache-{}",
            std::process::id()
        ));
        let builder = NativeBuilder::new(ArtifactCache::new(cache), toolchain);
        Some(
            builder.run_graph(
                &graph,
                &NativeBuildOptions {
                    output: std::env::temp_dir()
                        .join(format!("kiln-native-safe-regex-{}", std::process::id())),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                    march_native: false,
                },
            ),
        )
    })();
    fs::remove_file(&entry).expect("remove package probe");
    let Some(report) = result else {
        eprintln!("skipping real CommonJS package integration: no C compiler found");
        return;
    };
    let report = report.expect("real CommonJS package runs natively");
    assert_eq!(report.stdout, b"true\n");
}

#[test]
#[ignore = "diagnostic probe for the native Fastify constructor"]
fn native_fastify_constructor_probe() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let package_root = workspace.join("conformance/server/packages");
    if !package_root
        .join("node_modules/fastify/fastify.js")
        .is_file()
    {
        return;
    }
    let entry = package_root.join(format!(
        ".kiln-native-fastify-constructor-{}.mjs",
        std::process::id()
    ));
    fs::write(
        &entry,
        "import Fastify from 'fastify'; const app = Fastify(); console.log(typeof app.get);",
    )
    .expect("write Fastify constructor probe");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&package_root).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build Fastify constructor graph");
    let result = Toolchain::discover().ok().map(|toolchain| {
        NativeBuilder::new(
            ArtifactCache::new(std::env::temp_dir().join("kiln-native-fastify-probe-cache")),
            toolchain,
        )
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: std::env::temp_dir().join("kiln-native-fastify-probe"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
    });
    fs::remove_file(&entry).expect("remove Fastify constructor probe");
    let Some(report) = result else { return };
    let report = report.expect("Fastify constructor runs natively");
    assert_eq!(
        report.exit_code,
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&report.stderr)
    );
    assert_eq!(report.stdout, b"function\n");
}

/// `require("node:buffer")` is the module, and `default` is the module namespace rather than
/// the constructor.
///
/// This used to assert `buffer.Buffer === buffer.default`, which was true here and false in
/// the interpreter -- `default` is the namespace object there, and `typeof` says "object"
/// rather than "function". That is the class of divergence this backend is not allowed to
/// have, and the assertion was encoding it as the contract. What the two engines really share
/// is the identity every `Buffer.isBuffer` shim depends on: whichever way you reach the
/// constructor, it is the same value.
#[test]
fn native_commonjs_builtin_namespace_is_not_its_default_export() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-builtin-namespace");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir(root.path.join("pkg")).expect("create package directory");
    fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
        .expect("write CommonJS package");
    fs::write(
        root.path.join("pkg/index.js"),
        "const buffer = require('node:buffer'); module.exports = typeof buffer.constants.MAX_STRING_LENGTH === 'number' && buffer.Buffer !== buffer.default && buffer.default.Buffer === buffer.Buffer && typeof buffer.from === 'function';",
    )
    .expect("write namespace consumer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import answer from './pkg/index.js'; console.log(answer);",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS namespace graph");

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native builtin namespace integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let run = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("standalone graph runs");
    assert_eq!(run.stdout, b"true\n");
}

#[test]
fn native_commonjs_require_materializes_captured_json_once() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-json");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir(root.path.join("pkg")).expect("create package directory");
    fs::write(
        root.path.join("pkg/package.json"),
        r#"{"type":"commonjs","version":"7.4.2"}"#,
    )
    .expect("write CommonJS package JSON");
    fs::write(
            root.path.join("pkg/index.js"),
            "const first = require('./package.json'); const { version } = first; const second = require('./package.json'); console.log(first === second, version); module.exports = version;",
        )
        .expect("write CommonJS JSON consumer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import version from './pkg/index.js'; console.log(version);",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS JSON graph");
    assert!(graph.modules().any(|node| {
        node.source == ModuleSource::Json
            && node
                .data
                .as_deref()
                .is_some_and(|text| text.contains("7.4.2"))
    }));
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS JSON execution: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let report = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("native CommonJS JSON graph builds and runs");
    assert_eq!(report.stdout, b"true 7.4.2\n7.4.2\n");
}

#[test]
fn native_graphs_materialize_one_snapshot_namespace_per_provider() {
    let root = TestDirectory::new("kiln-native-namespace-graph");
    let graph = namespace_import_graph(&root.path.join("project"));
    let modules = graph
        .modules()
        .map(|node| {
            (
                node,
                kiln_backend_c::emit_module(&node.program).expect("module emits"),
            )
        })
        .collect::<Vec<_>>();

    let launcher = graph_launcher_source(&graph, &modules, &NativeRealmPolicy::default())
        .expect("launcher emits");
    assert!(launcher.contains("kiln_imports_1[0] = kiln_namespaces[0];"));
    assert_eq!(launcher.matches("kiln_namespaces[0]").count(), 2);
    assert!(launcher.contains("_namespace_wrapper(&kiln_namespaces[0]"));
    let wrapper = graph_shared_wrapper_source(&graph, &modules, "kiln_namespace_graph")
        .expect("shared wrapper emits");
    assert!(wrapper.contains("import_arrays[1][0] = namespaces[0];"));
    assert!(wrapper.contains("_namespace_wrapper(&namespaces[0]"));

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native namespace integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let report = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("namespace graph builds and runs");
    assert_eq!(report.stdout, b"42\n");

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let shared = builder
            .build_shared_graph(
                &graph,
                &NativeBuildOptions {
                    output: root.path.join("namespace").with_extension(
                        if cfg!(target_os = "macos") {
                            "dylib"
                        } else {
                            "so"
                        },
                    ),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                    march_native: false,
                },
            )
            .expect("namespace shared graph links");
        assert!(shared.output.is_file());
    }
}

#[test]
fn native_namespace_reexport_reuses_the_provider_namespace_identity() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native namespace re-export integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-namespace-reexport");
    let origin = root.path.join("origin.mjs");
    let barrel = root.path.join("barrel.mjs");
    let entry = root.path.join("main.mjs");
    fs::write(&origin, "export const answer = 42;\n").expect("write namespace origin");
    fs::write(&barrel, "export * as origin from './origin.mjs';\n")
        .expect("write namespace barrel");
    fs::write(
        &entry,
        r#"import * as direct from "./origin.mjs";
import { origin as via } from "./barrel.mjs";
console.log(direct === via, via.answer);
"#,
    )
    .expect("write namespace consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("namespace re-export graph compiles");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("namespace re-export graph builds and runs");
    assert_eq!(report.stdout, b"true 42\n");
}

#[test]
fn native_namespace_refuses_reserved_string_exports_before_emission() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-namespace-reserved-export");
    let origin = root.path.join("origin.mjs");
    let entry = root.path.join("main.mjs");
    fs::write(
        &origin,
        "const value = 1; export { value as \"@@toStringTag\" };\n",
    )
    .expect("write reserved namespace export");
    fs::write(
        &entry,
        "import * as namespace from './origin.mjs'; console.log(namespace);\n",
    )
    .expect("write namespace consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("reserved export graph compiles");
    let modules = graph
        .modules()
        .map(|node| {
            (
                node,
                kiln_backend_c::emit_module(&node.program).expect("module emits"),
            )
        })
        .collect::<Vec<_>>();
    let error = graph_launcher_source(&graph, &modules, &NativeRealmPolicy::default())
        .expect_err("reserved namespace export must refuse before C emission");
    assert!(matches!(
        error,
        NativeError::Emit(EmitError::Unsupported { feature })
            if feature == "@@-prefixed exports in native module namespaces"
    ));
}

#[test]
fn native_named_reexport_of_commonjs_default_keeps_slot_zero() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping CommonJS re-export integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-commonjs-named-reexport");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir(root.path.join("pkg")).expect("create CommonJS package");
    fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
        .expect("write CommonJS package metadata");
    fs::write(root.path.join("pkg/index.js"), "module.exports = 42;\n")
        .expect("write CommonJS provider");
    fs::write(
        root.path.join("barrel.mjs"),
        "export { default as answer } from './pkg/index.js';\n",
    )
    .expect("write ESM barrel");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import { answer } from './barrel.mjs'; console.log(answer);\n",
    )
    .expect("write ESM consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("CommonJS re-export graph compiles");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("CommonJS re-export graph builds and runs");
    assert_eq!(report.stdout, b"42\n");
}

#[test]
fn native_named_import_and_reexport_of_commonjs_properties_resolve_property_values() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping CommonJS named import integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-commonjs-named-import");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir(root.path.join("pkg")).expect("create CommonJS package");
    fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
        .expect("write CommonJS package metadata");
    fs::write(
        root.path.join("pkg/index.js"),
        "module.exports = { answer: 42, greet: () => 'hello' };\n",
    )
    .expect("write CommonJS provider");
    fs::write(
        root.path.join("barrel.mjs"),
        "export { answer, greet } from './pkg/index.js';\n",
    )
    .expect("write ESM barrel");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import { answer, greet } from './barrel.mjs'; console.log(answer, greet());\n",
    )
    .expect("write ESM consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("CommonJS named import graph compiles");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("CommonJS named import graph builds and runs");
    assert_eq!(report.stdout, b"42 hello\n");
}

#[test]
fn native_namespace_import_of_commonjs_resolves_property_values() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping CommonJS namespace import integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-commonjs-namespace-import");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir(root.path.join("pkg")).expect("create CommonJS package");
    fs::write(root.path.join("pkg/package.json"), r#"{"type":"commonjs"}"#)
        .expect("write CommonJS package metadata");
    fs::write(
        root.path.join("pkg/index.js"),
        "module.exports = { answer: 42, greet: () => 'hello' };\n",
    )
    .expect("write CommonJS provider");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import * as pkg from './pkg/index.js'; console.log(pkg.answer, pkg.greet());\n",
    )
    .expect("write ESM consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("CommonJS namespace import graph compiles");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("CommonJS namespace import graph builds and runs");
    assert_eq!(report.stdout, b"42 hello\n");
}

#[test]
fn native_dynamic_import_resolves_opentui_and_in_graph_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping dynamic import integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-import");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::create_dir_all(root.path.join("node_modules/@opentui/core-linux-x64"))
        .expect("create opentui dir");
    fs::write(
        root.path
            .join("node_modules/@opentui/core-linux-x64/libopentui.so"),
        "fake-opentui-so",
    )
    .expect("write mock libopentui");
    fs::write(root.path.join("helper.mjs"), "export const answer = 99;\n").expect("write helper");
    fs::write(root.path.join("asset.wasm"), [0_u8, 159, 146, 150]).expect("write asset");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import * as h from './helper.mjs';\n\
         const otui = await import('@opentui/core-linux-x64');\n\
         const helper = await import('./helper.mjs');\n\
         const asset = await import('./asset.wasm', { with: { type: 'file' } });\n\
         const ffi = await import('node:ffi');\n\
         console.log(otui.default.includes('libopentui.so'), helper.answer, asset.default.endsWith('asset.wasm'), typeof ffi.dlopen, typeof ffi.getRawPointer);\n",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("dynamic import graph compiles");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("dynamic import graph builds and runs");
    if report.stdout != b"true 99 true function function\n" {
        eprintln!("exit_code: {:?}", report.exit_code);
        eprintln!("result: {:?}", report.result);
        eprintln!("stderr: {}", String::from_utf8_lossy(&report.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&report.stdout));
    }
    assert_eq!(report.stdout, b"true 99 true function function\n");
}

#[test]
fn native_dynamic_import_uses_importer_identity_for_relative_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping importer identity integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-import-importer-identity");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    for (dir, value) in [("a", "A"), ("b", "B")] {
        fs::create_dir_all(root.path.join(dir)).expect("create loader directory");
        fs::write(
            root.path.join(format!("{dir}/loader.mjs")),
            "export async function load() { return (await import('./same.mjs')).value; }\n",
        )
        .expect("write loader");
        fs::write(
            root.path.join(format!("{dir}/same.mjs")),
            format!("export const value = '{value}';\n"),
        )
        .expect("write relative module");
    }
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import { load as loadA } from './a/loader.mjs';\n\
         import { load as loadB } from './b/loader.mjs';\n\
         import * as staticAssert from 'node:assert';\n\
         const dynamicAssert = await import('node:assert');\n\
         console.log(await loadA(), await loadB(), staticAssert === dynamicAssert);\n",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("dynamic importer graph compiles");
    for (profile, name) in [
        (NativeProfile::Dev, "dev"),
        (NativeProfile::Release, "release"),
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
            },
        )
        .expect("dynamic importer graph builds and runs");
        assert_eq!(report.stdout, b"A B true\n", "profile {name}");
    }
}

#[test]
fn a_second_build_reuses_every_material_artifact_and_runs() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native integration: no C compiler found");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "kiln-native-test-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let cache = root.join("cache");
    let first_output = root.join("first");
    let second_output = root.join("second");
    let builder = NativeBuilder::new(ArtifactCache::new(cache), toolchain);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::String("native".to_owned()),
            Constant::Number(42.0_f64.to_bits()),
        ],
        code: vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 2,
            },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let first = builder
        .build(
            &program,
            &NativeBuildOptions {
                output: first_output,
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("first native build succeeds");
    assert!(!first.cache_hits.application_object);
    assert!(!first.cache_hits.executable);

    let second = builder
        .run(
            &program,
            &NativeBuildOptions {
                output: second_output,
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("cached native build and run succeeds");
    assert!(second.build.cache_hits.generated_c);
    assert!(second.build.cache_hits.application_object);
    assert!(second.build.cache_hits.runtime_object);
    assert!(second.build.cache_hits.launcher_object);
    assert!(second.build.cache_hits.executable);
    assert_eq!(second.build.timings.compile, Duration::ZERO);
    assert_eq!(second.build.timings.link, Duration::ZERO);
    assert_eq!(second.exit_code, Some(0));
    assert_eq!(second.stdout, b"native 42\n");
    assert!(second.stderr.is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn embedded_shared_library_calls_host_without_cli_runtime_and_reuses_cache() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping embedded native integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping embedded native integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-embedded-test");
    let cache = root.path.join("cache");
    let library_path = root
        .path
        .join("embedded")
        .with_extension(if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        });
    let builder = NativeBuilder::new(ArtifactCache::new(cache), toolchain.clone());
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::String("embedded\0tail=".to_owned()),
            Constant::Number(0.1_f64.to_bits()),
        ],
        code: vec![
            Instruction::Const(0),
            Instruction::Const(1),
            Instruction::Binary(kiln_ir::BinaryOp::Add),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };
    let colliding_output = root.path.join("embedded-header.H");
    let collision = builder
        .build_shared_library(
            &program,
            &NativeBuildOptions {
                output: colliding_output.clone(),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect_err("a library output cannot alias its derived header");
    assert!(matches!(
        collision,
        NativeError::ArtifactPathCollision { .. }
    ));
    assert!(!colliding_output.exists());
    let options = NativeBuildOptions {
        output: library_path.clone(),
        profile: NativeProfile::Dev,
        realm: NativeRealmPolicy::default(),
        march_native: false,
    };
    let first = builder
        .build_shared_library(&program, &options)
        .expect("first embedded shared-library build succeeds");
    assert!(!first.cache_hits.generated_c);
    assert!(!first.cache_hits.object);
    assert!(!first.cache_hits.library);
    let executable = builder
        .build(
            &program,
            &NativeBuildOptions {
                output: root.path.join("ordinary-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("ordinary executable build succeeds after PIC build");
    assert!(executable.cache_hits.generated_c);
    assert!(
        !executable.cache_hits.application_object,
        "a shared PIC object must not satisfy an executable-object lookup"
    );
    let second = builder
        .build_shared_library(&program, &options)
        .expect("second embedded shared-library build succeeds");
    assert!(second.cache_hits.generated_c);
    assert!(second.cache_hits.object);
    assert!(second.cache_hits.library);
    assert_eq!(first.module_symbol, second.module_symbol);
    assert_eq!(second.output, library_path);
    assert!(second.loader.is_file());
    assert_eq!(
        fs::read(&second.output).unwrap(),
        fs::read(&second.loader).unwrap()
    );
    assert_eq!(second.loader, first.loader);
    assert_eq!(first.header, library_path.with_extension("h"));
    assert_eq!(second.header, first.header);
    let package = shared_publication::read_shared_library_package(&second.output)
        .expect("read scalar shared-library package");
    assert_eq!(package.manifest().legacy_entry_symbol, second.module_symbol);
    assert_eq!(package.manifest().v2_api_symbol, second.module_api_symbol);
    assert_eq!(
        fs::read(package.header_path()).unwrap(),
        fs::read(&second.header).unwrap()
    );
    assert_eq!(
        fs::read(package.loader_path()).unwrap(),
        fs::read(&second.loader).unwrap()
    );
    let published_header = fs::read_to_string(&second.header).expect("read embedding header");
    assert!(published_header.contains(&format!("int {}(", second.module_symbol)));

    let renamed_path = root
        .path
        .join("renamed")
        .with_extension(if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        });
    let renamed = builder
        .build_shared_library(
            &program,
            &NativeBuildOptions {
                output: renamed_path,
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("renamed shared-library build succeeds");
    assert!(renamed.cache_hits.generated_c);
    assert!(renamed.cache_hits.object);
    assert!(
        renamed.cache_hits.library,
        "a content-named loader is reusable across requested output aliases"
    );
    assert_eq!(renamed.loader, first.loader);
    assert_eq!(renamed.header, renamed.output.with_extension("h"));

    let host_source = root.path.join("host.c");
    let host_object = root.path.join("host.o");
    let host_executable = root.path.join("host");
    let mut source = String::from("#include \"");
    source.push_str(
        second
            .header
            .file_name()
            .and_then(|name| name.to_str())
            .expect("embedding header has a UTF-8 file name"),
    );
    source.push_str(
        r#""
#include <stdio.h>
#include <string.h>

struct capture {
    char bytes[256];
    size_t length;
    int calls;
};

"#,
    );
    source.push_str(
        r#"static int capture_host_call(void *context, uint16_t operation, uint16_t argc,
                             const kiln_value *arguments, kiln_value *result) {
    struct capture *capture = (struct capture *)context;
    if (capture == NULL || result == NULL || operation != KILN_HOST_OP_WRITE_STDOUT
        || argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING
        || arguments[0].ptr == NULL || arguments[0].len >= sizeof(capture->bytes)) {
        return KILN_C99_HOST_ERROR;
    }
    memcpy(capture->bytes, arguments[0].ptr, arguments[0].len);
    capture->length = arguments[0].len;
    capture->bytes[capture->length++] = '\n';
    capture->calls += 1;
    *result = (kiln_value){ KILN_VALUE_UNDEFINED, 0, {0} };
    return KILN_C99_OK;
}

int main(void) {
    struct capture capture = {{0}, 0, 0};
    kiln_host host = {capture_host_call, &capture};
    kiln_value result;
    int status = "#,
    );
    source.push_str(&second.module_symbol);
    source.push_str(
        r#"(&host, &result);
    if (status != KILN_C99_OK || capture.calls != 1) {
        return 2;
    }
    if (fwrite(capture.bytes, 1, capture.length, stdout) != capture.length) {
        return 3;
    }
    return 0;
}
"#,
    );
    fs::write(&host_source, source).expect("write embedding host");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile embedding host");
    let mut link_options = LinkOptions::default();
    link_options.runtime_libraries.push(library_path.clone());
    let loader_directory = second.loader.parent().expect("loader has directory");
    link_options
        .extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(
            std::slice::from_ref(&host_object),
            &host_executable,
            &link_options,
        )
        .expect("link embedding host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run embedding host");
    assert!(
        output.status.success(),
        "embedding host failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"embedded\0tail=0.1\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn owned_result_scalar_api_copies_strings_and_is_idempotent() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping owned-result native integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping owned-result native integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-owned-result-test");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone());
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![Constant::String("left\0right".to_owned())],
        code: vec![Instruction::Const(0), Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };
    let report = builder
        .build_shared_library(
            &program,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build owned-result scalar library");
    assert_eq!(
        report.module_api_symbol,
        format!("{}_api_v2", report.module_symbol)
    );
    let header_name = report
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("owned-result header file name");
    let host_source = root.path.join("host.c");
    let host_object = root.path.join("host.o");
    let host_executable = root.path.join("host");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <string.h>

int main(void) {{
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    kiln_value value;
    kiln_value legacy;
    int index;
    if (api == NULL || api->abi_version != KILN_MODULE_API_VERSION ||
        api->struct_size < sizeof(*api) || api->run == NULL || api->read == NULL ||
        api->dispose == NULL) return 1;
    if ({module_symbol}(NULL, &legacy) != KILN_C99_OK ||
        legacy.tag != KILN_VALUE_STRING || legacy.len != 10 ||
        memcmp(legacy.ptr, "left\0right", 10) != 0) return 2;
    for (index = 0; index < 64; ++index) {{
        if (api->run(NULL, &owned) != KILN_C99_OK || owned == NULL) return 3;
        if (api->read(owned, &value) != KILN_C99_OK || value.tag != KILN_VALUE_STRING ||
            value.len != 10 || value.ptr == NULL || memcmp(value.ptr, "left\0right", 10) != 0)
            return 4;
        api->dispose(&owned);
        if (owned != NULL) return 5;
        api->dispose(&owned);
    }}
    return 0;
}}
"#,
            api_symbol = report.module_api_symbol,
            module_symbol = report.module_symbol,
        ),
    )
    .expect("write owned-result host");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile owned-result host");
    let mut link_options = LinkOptions::default();
    link_options.runtime_libraries.push(report.output.clone());
    let loader_directory = report.loader.parent().expect("loader has directory");
    link_options
        .extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link owned-result host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run owned-result host");
    assert!(
        output.status.success(),
        "owned-result host failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    #[cfg(target_os = "linux")]
    {
        let symbols = Command::new("nm")
            .args(["-D", "--defined-only"])
            .arg(&report.loader)
            .output()
            .expect("nm is available");
        assert!(symbols.status.success());
        let mut exported = String::from_utf8_lossy(&symbols.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().last())
            .filter(|symbol| symbol.starts_with("kiln_"))
            .map(|symbol| symbol.split('@').next().unwrap_or(symbol).to_owned())
            .collect::<Vec<_>>();
        exported.sort_unstable();
        let mut expected = vec![report.module_symbol, report.module_api_symbol];
        expected.sort_unstable();
        assert_eq!(exported, expected);
    }
}

#[test]
fn owned_result_scalar_api_rejects_malformed_host_strings() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!(
            "skipping owned-result scalar malformed-host integration on unsupported platform"
        );
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping owned-result scalar malformed-host integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-owned-result-malformed-scalar-test");
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: Vec::new(),
        code: vec![
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 0,
            },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone())
        .build_shared_library(
            &program,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build malformed-host scalar library");
    let header_name = report
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("malformed-host scalar header file name");
    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"

static int malformed_call(void *context, uint16_t operation, uint16_t argc,
                          const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    (void)operation;
    (void)argc;
    (void)arguments;
    if (result == NULL) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ KILN_VALUE_STRING, 3, {{0}} }};
    return KILN_C99_OK;
}}

int main(void) {{
    kiln_host host = {{malformed_call, NULL}};
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    if (api == NULL) return 1;
    if (api->run(&host, &owned) != KILN_C99_INVALID_ARGUMENT || owned != NULL)
        return 2;
    api->dispose(&owned);
    return 0;
}}
"#,
            api_symbol = report.module_api_symbol,
        ),
    )
    .expect("write malformed-host scalar host");
    let host_object = root.path.join("host.o");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile malformed-host scalar host");
    let host_executable = root.path.join("host");
    let mut link_options = LinkOptions::default();
    link_options.runtime_libraries.push(report.output.clone());
    let loader_directory = report.loader.parent().expect("loader has directory");
    link_options
        .extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link malformed-host scalar host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run malformed-host scalar host");
    assert!(
        output.status.success(),
        "malformed-host scalar failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn owned_result_graph_api_preserves_uncaught_thrown_scalars() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping owned-result graph throw integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping owned-result graph throw integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-owned-result-throw-test");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create throw graph project");
    fs::write(project.join("main.js"), "").expect("write throw graph entry");
    let thrown = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::String("thrown\0value".to_owned())],
            code: vec![Instruction::Const(0), Instruction::Throw],
            local_count: 0,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let graph =
        ModuleGraph::build_with_modules(project.join("main.js"), &project, move |_| thrown.clone())
            .expect("construct throw graph");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone())
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build throw graph");
    let header_name = report
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("throw graph header file name");
    let host_source = root.path.join("host.c");
    let host_object = root.path.join("host.o");
    let host_executable = root.path.join("host");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <string.h>

int main(void) {{
    const kiln_module_api_v2 *api = {api_symbol}();
    kiln_result_v2 *owned = NULL;
    kiln_value value;
    int status;
    if (api == NULL) return 1;
    status = api->run(NULL, &owned);
    if (status != KILN_C99_THROWN || owned == NULL) return 2;
    if (api->read(owned, &value) != KILN_C99_OK || value.tag != KILN_VALUE_STRING ||
        value.len < 12 || value.ptr == NULL || memcmp(value.ptr, "thrown\0value", 12) != 0)
        return 3;
    api->dispose(&owned);
    if (owned != NULL) return 4;
    api->dispose(&owned);
    return 0;
}}
"#,
            api_symbol = report.module_api_symbol,
        ),
    )
    .expect("write throw graph host");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile throw graph host");
    let mut link_options = LinkOptions::default();
    link_options.runtime_libraries.push(report.output.clone());
    let loader_directory = report.loader.parent().expect("loader has directory");
    link_options
        .extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link throw graph host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run throw graph host");
    assert!(
        output.status.success(),
        "throw graph host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn concurrent_runs_with_one_public_output_launch_their_private_program() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native concurrency integration: no C compiler found");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "kiln-native-race-test-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let shared_output = root.join("shared-program");
    let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);
    let programs = [stdout_program("alpha", 1.0), stdout_program("beta", 2.0)];
    for program in &programs {
        builder
            .build(
                program,
                &NativeBuildOptions {
                    output: shared_output.clone(),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                    march_native: false,
                },
            )
            .expect("prime each native executable");
    }

    let barrier = Arc::new(Barrier::new(12));
    let mut threads = Vec::new();
    for index in 0..12 {
        let builder = builder.clone();
        let barrier = Arc::clone(&barrier);
        let program = programs[index % programs.len()].clone();
        let output = shared_output.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            builder
                .run(
                    &program,
                    &NativeBuildOptions {
                        output,
                        profile: NativeProfile::Dev,
                        realm: NativeRealmPolicy::default(),
                        march_native: false,
                    },
                )
                .expect("concurrent native run succeeds")
        }));
    }
    for (index, thread) in threads.into_iter().enumerate() {
        let expected: &[u8] = if index % 2 == 0 {
            b"alpha\n"
        } else {
            b"beta\n"
        };
        let report = thread.join().expect("run thread does not panic");
        assert_eq!(report.stdout, expected);
        let expected_result = if index % 2 == 0 { 1.0_f64 } else { 2.0_f64 };
        assert_eq!(
            report.result,
            Some(HostValue::Number(expected_result.to_bits()))
        );
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn native_and_interpreter_agree_across_the_scalar_control_flow_slice() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native equivalence integration: no C compiler found");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "kiln-native-equivalence-test-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);

    for (name, program) in scalar_equivalence_programs() {
        let mut host = kiln_runtime::CaptureHost::new();
        let interpreted = kiln_interpreter::execute(
            std::rc::Rc::new(program.clone()),
            &mut host,
            Default::default(),
            kiln_interpreter::ExecutionCapabilities::default(),
        )
        .unwrap_or_else(|error| panic!("interpreter fixture {name} failed: {error}"));
        let expected = host.into_stdout().into_bytes();
        let expected_result = HostValue::from(interpreted.value);
        let native = builder
            .run(
                &program,
                &NativeBuildOptions {
                    output: root.join(name),
                    profile: NativeProfile::Dev,
                    realm: NativeRealmPolicy::default(),
                    march_native: false,
                },
            )
            .unwrap_or_else(|error| panic!("native fixture {name} failed: {error}"));
        assert_eq!(native.exit_code, Some(0), "fixture {name}");
        assert_eq!(native.stdout, expected, "fixture {name}");
        assert!(native.stderr.is_empty(), "fixture {name}");
        assert_eq!(native.result, Some(expected_result), "fixture {name}");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn native_json_errors_are_catchable_and_uncaught_errors_are_reported() {
    use kiln_module_resolver::ModuleResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native JSON exception integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-json-exceptions");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create JSON exception project");
    let entry = project.join("main.js");
    fs::write(
            &entry,
            "try { JSON.parse('{oops'); } catch (error) { console.log('parse', error instanceof SyntaxError, error.constructor.name); }\nconsole.log('after-parse');\nconst cycle = {}; cycle.self = cycle; try { JSON.stringify(cycle); } catch (error) { console.log('stringify', error instanceof TypeError, error.constructor.name); }\nconsole.log('after-stringify');\n",
        )
        .expect("write catchable JSON exception source");
    let mut graph = ModuleGraph::build_node(
        &entry,
        ModuleResolver::new(&project).expect("module resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("catchable JSON exception source compiles");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let caught = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("caught"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("catchable JSON exceptions run natively");
    assert_eq!(caught.exit_code, Some(0));
    assert_eq!(
        caught.stdout,
        b"parse true SyntaxError\nafter-parse\nstringify true TypeError\nafter-stringify\n"
    );
    assert!(caught.stderr.is_empty());

    fs::write(&entry, "JSON.parse('{oops');\n").expect("write uncaught JSON exception source");
    graph = ModuleGraph::build_node(
        &entry,
        ModuleResolver::new(&project).expect("module resolver for uncaught source"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("uncaught JSON exception source compiles");
    let uncaught = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("uncaught"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("uncaught JSON exception run reports a native result");
    assert_eq!(uncaught.exit_code, Some(1));
    assert!(uncaught.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&uncaught.stderr)
            .contains("uncaught SyntaxError: Unexpected token in JSON")
    );

    fs::write(
        &entry,
        "const cycle = {}; cycle.self = cycle; JSON.stringify(cycle);\n",
    )
    .expect("write uncaught cyclic JSON source");
    graph = ModuleGraph::build_node(
        &entry,
        ModuleResolver::new(&project).expect("module resolver for cyclic source"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("uncaught cyclic JSON source compiles");
    let uncaught_cycle = builder
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("uncaught-cycle"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("uncaught cyclic JSON run reports a native result");
    assert_eq!(uncaught_cycle.exit_code, Some(1));
    assert!(uncaught_cycle.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&uncaught_cycle.stderr)
            .contains("uncaught TypeError: Converting circular structure to JSON")
    );
}

#[test]
fn graph_build_reuses_sibling_objects_across_different_roots() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native graph integration: no C compiler found");
        return;
    };
    let root = std::env::temp_dir().join(format!(
        "kiln-native-graph-test-{}-{}",
        std::process::id(),
        WORK_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let first_project = root.join("first");
    let second_project = root.join("second");
    for project in [&first_project, &second_project] {
        fs::create_dir_all(project).expect("create graph project");
        fs::write(project.join("main.js"), "").expect("write graph entry");
        fs::write(project.join("dep.js"), "").expect("write graph dependency");
    }
    let dependency = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(42.0_f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "value".into(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let entry = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(99.0_f64.to_bits())],
            code: vec![
                Instruction::LoadLocal(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Pop,
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["./dep.js".into()],
        imports: vec![ImportBinding {
            source: "./dep.js".into(),
            imported: "value".into(),
            local: "value".into(),
            local_slot: 0,
        }],
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let make_graph = |project: &Path| {
        let entry_path = project.join("main.js");
        let dependency_path =
            fs::canonicalize(project.join("dep.js")).expect("canonical dependency");
        let dependency = dependency.clone();
        let entry = entry.clone();
        ModuleGraph::build_with_modules(entry_path, project, move |path| {
            if path == dependency_path {
                dependency.clone()
            } else {
                entry.clone()
            }
        })
        .expect("construct graph")
    };
    let first_graph = make_graph(&first_project);
    let second_graph = make_graph(&second_project);
    let first_modules = first_graph
        .modules()
        .map(|node| {
            (
                node,
                kiln_backend_c::emit_module(&node.program).expect("module emits"),
            )
        })
        .collect::<Vec<_>>();
    let launcher =
        graph_launcher_source(&first_graph, &first_modules, &NativeRealmPolicy::default())
            .expect("launcher emits");
    let main = launcher
        .find("int main(int argc, char **argv)")
        .expect("launcher has main");
    assert!(launcher[..main].contains("static kiln_value kiln_exports_"));
    assert!(launcher[..main].contains("static kiln_value kiln_imports_"));
    assert!(!launcher[main..].contains("kiln_value kiln_exports_"));
    assert!(!launcher[main..].contains("kiln_value kiln_imports_"));
    let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);
    let first = builder
        .build_graph(
            &first_graph,
            &NativeBuildOptions {
                output: root.join("first-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("first graph build succeeds");
    assert!(!first.cache_hits.application_object);
    let second = builder
        .run_graph(
            &second_graph,
            &NativeBuildOptions {
                output: root.join("second-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("second graph build succeeds");
    assert!(second.build.cache_hits.generated_c);
    assert!(second.build.cache_hits.application_object);
    // Diagnostics use logical project-relative paths, so sibling roots reuse launchers too.
    assert!(second.build.cache_hits.launcher_object);
    assert!(second.build.cache_hits.executable);
    assert_eq!(second.build.timings.compile, Duration::ZERO);
    assert_eq!(second.build.timings.link, Duration::ZERO);
    assert_eq!(second.stdout, b"42\n");
    assert_eq!(second.result, Some(HostValue::Number(99.0_f64.to_bits())));

    assert_eq!(
        second
            .build
            .module_artifacts
            .iter()
            .map(|module| module.logical_path.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("dep.js"), Some("main.js")]
    );
    assert!(second.build.runtime_generated_c_cache_hit);
    assert!(second.build.launcher_generated_c_cache_hit);

    let edited_dependency = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(43.0_f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "value".into(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let edited_graph = {
        let entry_path = first_project.join("main.js");
        let dependency_path =
            fs::canonicalize(first_project.join("dep.js")).expect("canonical dependency");
        let edited_dependency = edited_dependency.clone();
        let entry = entry.clone();
        ModuleGraph::build_with_modules(entry_path, &first_project, move |path| {
            if path == dependency_path {
                edited_dependency.clone()
            } else {
                entry.clone()
            }
        })
        .expect("construct edited graph")
    };
    let edited = builder
        .build_graph(
            &edited_graph,
            &NativeBuildOptions {
                output: root.join("edited-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("edited graph build succeeds");
    let generated_c_misses = edited
        .module_artifacts
        .iter()
        .filter(|module| !module.generated_c_cache_hit)
        .count();
    let object_misses = edited
        .module_artifacts
        .iter()
        .filter(|module| !module.object_cache_hit)
        .count();
    assert_eq!(generated_c_misses, 1);
    assert_eq!(object_misses, 1);
    assert_eq!(
        edited.module_artifacts[0].logical_path.as_deref(),
        Some("dep.js")
    );
    assert!(!edited.module_artifacts[0].generated_c_cache_hit);
    assert!(!edited.module_artifacts[0].object_cache_hit);
    assert!(edited.module_artifacts[1].generated_c_cache_hit);
    assert!(edited.module_artifacts[1].object_cache_hit);
    assert!(edited.cache_hits.runtime_object);
    assert!(edited.cache_hits.launcher_object);
    assert!(edited.runtime_generated_c_cache_hit);
    assert!(edited.launcher_generated_c_cache_hit);
    assert!(!edited.cache_hits.executable);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn commonjs_shared_identity_includes_observable_absolute_path() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-shared-path-identity");
    let first = root.path.join("first");
    let second = root.path.join("second");
    for project in [&first, &second] {
        fs::create_dir_all(project).expect("create CommonJS project");
        fs::write(project.join("package.json"), r#"{"type":"commonjs"}"#)
            .expect("write CommonJS package metadata");
        fs::write(project.join("main.js"), "module.exports = __filename;")
            .expect("write CommonJS entry");
    }

    let identity = |project: &Path| {
        let entry = project.join("main.js");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(project).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build CommonJS graph");
        let modules = graph
            .modules()
            .map(|node| {
                let logical = logical_module_path(graph.project_root(), &node.path)
                    .expect("logical module path");
                (
                    node,
                    emit_native_graph_module(node, &logical, &NativeRealmPolicy::default())
                        .expect("module emits"),
                )
            })
            .collect::<Vec<_>>();
        let topology = graph_shared_topology_digest(&graph, &modules);
        let public = graph_shared_public_digest(topology, &modules)
            .expect("CommonJS graph has absolute module paths");
        let module_sources = modules
            .iter()
            .map(|(_, module)| Digest::hash(module.source.as_bytes()))
            .collect::<Vec<_>>();
        let symbol = format!("kiln_shared_graph_{}", public.to_hex());
        let wrapper =
            graph_shared_wrapper_source(&graph, &modules, &symbol).expect("shared wrapper emits");
        (topology, public, module_sources, wrapper)
    };

    let first_identity = identity(&first);
    let second_identity = identity(&second);
    assert_eq!(first_identity.0, second_identity.0);
    assert_eq!(first_identity.2, second_identity.2);
    assert_ne!(first_identity.1, second_identity.1);
    assert!(
        first_identity.3.contains(&native_c_string(
            &fs::canonicalize(first.join("main.js"))
                .expect("canonical first entry")
                .to_string_lossy()
        ))
    );
    assert!(
        second_identity.3.contains(&native_c_string(
            &fs::canonicalize(second.join("main.js"))
                .expect("canonical second entry")
                .to_string_lossy()
        ))
    );
}

#[test]
fn shared_graph_wrapper_emits_and_links_without_static_value_arrays() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping native shared graph integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native shared graph integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-shared-graph-test");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create shared graph project");
    fs::write(project.join("main.js"), "").expect("write shared graph entry");
    fs::write(project.join("dep.js"), "").expect("write shared graph dependency");
    let dependency = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: vec![Constant::Number(7.0_f64.to_bits())],
            code: vec![
                Instruction::Const(0),
                Instruction::StoreLocal(0),
                Instruction::Const(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "value".into(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let entry = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![Instruction::LoadLocal(0), Instruction::Return],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["./dep.js".into()],
        imports: vec![ImportBinding {
            source: "./dep.js".into(),
            imported: "value".into(),
            local: "value".into(),
            local_slot: 0,
        }],
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let dependency_path = fs::canonicalize(project.join("dep.js")).expect("canonical dep");
    let graph = ModuleGraph::build_with_modules(project.join("main.js"), &project, move |path| {
        if path == dependency_path {
            dependency.clone()
        } else {
            entry.clone()
        }
    })
    .expect("construct shared graph");
    let modules = graph
        .modules()
        .map(|node| {
            (
                node,
                emit_module_with_logical_path(
                    &node.program,
                    &logical_module_path(graph.project_root(), &node.path)
                        .expect("logical module path"),
                )
                .expect("module emits"),
            )
        })
        .collect::<Vec<_>>();
    let wrapper_symbol = format!(
        "kiln_shared_graph_{}",
        graph_shared_topology_digest(&graph, &modules).to_hex()
    );
    let wrapper =
        graph_shared_wrapper_source(&graph, &modules, &wrapper_symbol).expect("wrapper emits");
    assert!(wrapper.contains(embedding_header()));
    assert!(wrapper.contains(&format!("{}_api_v2", wrapper_symbol)));
    assert!(wrapper.contains("preserve_result"));
    assert!(wrapper.contains("kiln_module_api_v2"));
    assert!(wrapper.contains("static int kiln_v2_graph_"));
    assert!(!wrapper.contains("static kiln_result_v2"));
    assert!(wrapper.contains("extern int kiln_module_path_"));
    assert!(wrapper.contains("malloc((size_t)"));
    assert!(!wrapper.contains("static kiln_value kiln_import"));
    assert!(!wrapper.contains("static kiln_value kiln_export"));
    assert!(!wrapper.contains("kiln_cli_host_call"));
    assert!(!wrapper.contains("setlocale"));
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone());
    let report = builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root
                    .path
                    .join("built")
                    .with_extension(if cfg!(target_os = "macos") {
                        "dylib"
                    } else {
                        "so"
                    }),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build shared graph succeeds");
    assert!(report.module_symbol.starts_with("kiln_shared_graph_"));
    assert!(report.output.is_file());
    let cached = builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: report.output.clone(),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("cached shared graph succeeds");
    assert!(cached.cache_hits.generated_c);
    assert!(cached.cache_hits.object);
    assert!(cached.cache_hits.library);
    assert_eq!(cached.loader, report.loader);
    assert!(cached.loader.is_file());
    let renamed = builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root
                    .path
                    .join("renamed")
                    .with_extension(if cfg!(target_os = "macos") {
                        "dylib"
                    } else {
                        "so"
                    }),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("renamed shared graph succeeds");
    assert!(renamed.cache_hits.generated_c);
    assert!(renamed.cache_hits.object);
    assert!(renamed.cache_hits.library);
    assert_eq!(renamed.loader, report.loader);
    let wrapper_source = root.path.join("wrapper.c");
    fs::write(&wrapper_source, wrapper).expect("write wrapper source");
    let compile_options = CompileOptions {
        position_independent: true,
        ..CompileOptions::default()
    };
    let mut objects = Vec::with_capacity(modules.len() + 1);
    for (index, (_, module)) in modules.iter().enumerate() {
        let source = root.path.join(format!("module-{index}.c"));
        let object = root.path.join(format!("module-{index}.o"));
        fs::write(&source, &module.source).expect("write module source");
        toolchain
            .compile_c(&source, &object, &compile_options)
            .expect("compile module source");
        objects.push(object);
    }
    let wrapper_object = root.path.join("wrapper.o");
    toolchain
        .compile_c(&wrapper_source, &wrapper_object, &compile_options)
        .expect("compile wrapper source");
    objects.push(wrapper_object);
    let target = target_identity(toolchain.identity());
    let (provider, _, _) = builder
        .cached_number_provider(&target)
        .expect("load Realm provider");
    let provider_archive = root.path.join("realm-provider.a");
    materialize(&provider_archive, &provider, false).expect("materialize Realm provider");
    let library = root.path.join(shared_library_name());
    let mut manual_link = LinkOptions::default();
    manual_link.runtime_libraries.push(provider_archive);
    if cfg!(not(target_os = "windows")) {
        manual_link.libraries.push("m".to_owned());
    }
    toolchain
        .link_shared_library(&objects, &library, &manual_link)
        .expect("link shared graph");
    assert!(library.is_file());
    let header = root.path.join("wrapper.h");
    fs::write(
        &header,
        graph_shared_header(&wrapper_symbol).expect("wrapper header emits"),
    )
    .expect("write wrapper header");
    let host_source = root.path.join("null-host.c");
    fs::write(
            &host_source,
            format!(
                "#include \"wrapper.h\"\nint main(void) {{ kiln_value result; return {}(NULL, &result) != KILN_C99_OK || result.tag != KILN_VALUE_NUMBER; }}\n",
                wrapper_symbol
            ),
        )
        .expect("write null-host source");
    let host_object = root.path.join("null-host.o");
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile null-host source");
    let host_executable = root.path.join("null-host");
    let mut host_link = LinkOptions::default();
    host_link.runtime_libraries.push(library.clone());
    host_link
        .extra_args
        .push(format!("-Wl,-rpath,{}", root.path.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &host_link)
        .expect("link null-host executable");
    let output = Command::new(&host_executable)
        .output()
        .expect("run null-host executable");
    assert!(
        output.status.success(),
        "null-host graph failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shared_graph_proxy_preserves_host_strings_across_module_boundaries() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping native shared graph host integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native shared graph host integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-shared-graph-host-test");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create shared graph host project");
    fs::write(project.join("main.js"), "").expect("write shared graph host entry");
    fs::write(project.join("dep.js"), "").expect("write shared graph host dependency");

    let dependency = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 0,
                },
                Instruction::StoreLocal(0),
                Instruction::LoadLocal(0),
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: Vec::new(),
        imports: Vec::new(),
        exports: vec![ExportBinding {
            exported: "value".into(),
            local_slot: 0,
        }],
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let entry = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            constants: Vec::new(),
            code: vec![
                Instruction::LoadLocal(0),
                Instruction::HostCall {
                    op: HostOp::WriteStdout,
                    argc: 1,
                },
                Instruction::Return,
            ],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
        },
        dependencies: vec!["./dep.js".into()],
        imports: vec![ImportBinding {
            source: "./dep.js".into(),
            imported: "value".into(),
            local: "value".into(),
            local_slot: 0,
        }],
        exports: Vec::new(),
        strictness: kiln_ir::Strictness::default(),
        ..Default::default()
    };
    let dependency_path = fs::canonicalize(project.join("dep.js")).expect("canonical dep");
    let graph = ModuleGraph::build_with_modules(project.join("main.js"), &project, move |path| {
        if path == dependency_path {
            dependency.clone()
        } else {
            entry.clone()
        }
    })
    .expect("construct shared host graph");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone());
    let report = builder
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build shared host graph");

    let host_source = root.path.join("host.c");
    let host_object = root.path.join("host.o");
    let host_executable = root.path.join("host");
    let header_name = report
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("shared graph header file name");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <string.h>

typedef struct host_context {{
    int mode;
    char storage[4];
}} host_context;

static int host_call(void *opaque, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    host_context *context = (host_context *)opaque;
    if (context == NULL || operation != KILN_HOST_OP_WRITE_STDOUT || result == NULL)
        return KILN_C99_HOST_ERROR;
    if (argc == 0) {{
        if (context->mode == 2) return 7;
        if (context->mode == 3) {{
            *result = (kiln_value){{ KILN_VALUE_STRING, 3, {{0}} }};
            return KILN_C99_OK;
        }}
        if (context->mode == 1) {{
            *result = (kiln_value){{ KILN_VALUE_STRING, 0, {{0}} }};
            return KILN_C99_OK;
        }}
        memcpy(context->storage, "dep", 3);
        *result = (kiln_value){{KILN_VALUE_STRING, 3, {{0}}}};
        result->ptr = context->storage;
        return KILN_C99_OK;
    }}
    if (argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING)
        return KILN_C99_HOST_ERROR;
    if (context->mode == 1) {{
        if (arguments[0].len != 0 || arguments[0].ptr == NULL)
            return KILN_C99_HOST_ERROR;
    }} else if (arguments[0].len != 3 || arguments[0].ptr == NULL ||
               arguments[0].ptr == context->storage ||
               memcmp(arguments[0].ptr, "dep", 3) != 0) {{
        return KILN_C99_HOST_ERROR;
    }}
    if (context->mode == 0) {{
        *result = (kiln_value){{KILN_VALUE_STRING, 3, {{0}}}};
        result->ptr = context->storage;
    }} else {{
        *result = (kiln_value){{ KILN_VALUE_UNDEFINED, 0, {{0}} }};
    }}
    return KILN_C99_OK;
}}

int main(void) {{
    host_context context = {{0, {{0}}}};
    kiln_host host = {{host_call, &context}};
    kiln_value result;
    int index;
    for (index = 0; index < 64; ++index) {{
        if ({symbol}(&host, &result) != KILN_C99_OK) return 2;
    }}
    context.mode = 1;
    if ({symbol}(&host, &result) != KILN_C99_OK) return 3;
    context.mode = 2;
    if ({symbol}(&host, &result) != KILN_C99_HOST_ERROR) return 4;
    context.mode = 3;
    if ({symbol}(&host, &result) != KILN_C99_HOST_ERROR) return 5;
    if ({symbol}(NULL, &result) != KILN_C99_HOST_ERROR) return 6;
    {{
        const kiln_module_api_v2 *api = {api_symbol}();
        kiln_result_v2 *owned = NULL;
        if (api == NULL || api->abi_version != KILN_MODULE_API_VERSION) return 7;
        context.mode = 0;
        for (index = 0; index < 64; ++index) {{
            if (api->run(&host, &owned) != KILN_C99_OK || owned == NULL) return 8;
            if (api->read(owned, &result) != KILN_C99_OK || result.tag != KILN_VALUE_STRING ||
                result.len != 3 || result.ptr == NULL || memcmp(result.ptr, "dep", 3) != 0)
                return 9;
            api->dispose(&owned);
            if (owned != NULL) return 10;
            api->dispose(&owned);
        }}
        context.mode = 2;
        if (api->run(&host, &owned) != KILN_C99_HOST_ERROR || owned != NULL)
            return 11;
        context.mode = 3;
        if (api->run(&host, &owned) != KILN_C99_INVALID_ARGUMENT || owned != NULL)
            return 12;
    }}
    return 0;
}}
"#,
            symbol = report.module_symbol,
            api_symbol = report.module_api_symbol,
        ),
    )
    .expect("write shared graph host");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile shared graph host");
    let mut link_options = LinkOptions::default();
    link_options.runtime_libraries.push(report.output.clone());
    let loader_directory = report.loader.parent().expect("loader has directory");
    link_options
        .extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link shared graph host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run shared graph host");
    assert!(
        output.status.success(),
        "shared graph host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn content_named_loaders_allow_same_basename_libraries_in_one_host() {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping shared loader integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping shared loader integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-content-loader-test");
    let first_dir = root.path.join("first");
    let second_dir = root.path.join("second");
    let leaf = if cfg!(target_os = "macos") {
        "same.dylib"
    } else {
        "same.so"
    };
    let program = |value: f64| Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![Constant::Number(value.to_bits())],
        code: vec![Instruction::Const(0), Instruction::Return],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone());
    let first = builder
        .build_shared_library(
            &program(11.0),
            &NativeBuildOptions {
                output: first_dir.join(leaf),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build first same-basename library");
    let second = builder
        .build_shared_library(
            &program(22.0),
            &NativeBuildOptions {
                output: second_dir.join(leaf),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build second same-basename library");
    assert_ne!(first.loader.file_name(), second.loader.file_name());
    assert_eq!(
        fs::read(&first.output).unwrap(),
        fs::read(&first.loader).unwrap()
    );
    assert_eq!(
        fs::read(&second.output).unwrap(),
        fs::read(&second.loader).unwrap()
    );

    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "first/same.h"
#include "second/same.h"
#include <stdint.h>

int main(void) {{
    kiln_value result;
    if ({first_symbol}(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({first_bits}))
        return 1;
    if ({second_symbol}(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({second_bits}))
        return 2;
    return 0;
}}
"#,
            first_symbol = first.module_symbol,
            second_symbol = second.module_symbol,
            first_bits = 11.0_f64.to_bits(),
            second_bits = 22.0_f64.to_bits(),
        ),
    )
    .expect("write same-basename host");
    let host_object = root.path.join("host.o");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile same-basename host");
    let host_executable = root.path.join("host");
    let mut link_options = LinkOptions {
        runtime_libraries: vec![first.output.clone(), second.output.clone()],
        ..LinkOptions::default()
    };
    for directory in [
        first.loader.parent().expect("first loader directory"),
        second.loader.parent().expect("second loader directory"),
    ] {
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", directory.display()).into());
    }
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link same-basename host");
    let output = Command::new(&host_executable)
        .output()
        .expect("run same-basename host");
    assert!(
        output.status.success(),
        "same-basename host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn shared_graph_libraries_hide_path_symbols_and_cannot_interpose_each_other() {
    shared_graph_libraries_hide_path_symbols_with_profile(NativeProfile::Dev);
}

#[test]
fn shared_graph_libraries_hide_path_symbols_and_cannot_interpose_each_other_release() {
    shared_graph_libraries_hide_path_symbols_with_profile(NativeProfile::Release);
}

fn shared_graph_libraries_hide_path_symbols_with_profile(profile: NativeProfile) {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping shared graph visibility integration on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping shared graph visibility integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-shared-graph-visibility-test");
    let first_graph = one_module_graph(&root.path.join("first-project"), 11.0, "value");
    let second_graph = one_module_graph(&root.path.join("second-project"), 22.0, "value");
    let first_internal = emit_module_with_logical_path(
        &first_graph
            .modules()
            .next()
            .expect("first graph module")
            .program,
        "main.js",
    )
    .expect("first module emits")
    .symbol;
    let second_internal = emit_module_with_logical_path(
        &second_graph
            .modules()
            .next()
            .expect("second graph module")
            .program,
        "main.js",
    )
    .expect("second module emits")
    .symbol;
    assert_eq!(
        first_internal, second_internal,
        "logical-path entries must collide for this interposition regression"
    );

    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone());
    let first = builder
        .build_shared_graph(
            &first_graph,
            &NativeBuildOptions {
                output: root.path.join(if cfg!(target_os = "macos") {
                    "first.dylib"
                } else {
                    "first.so"
                }),
                profile,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build first graph library");
    let second = builder
        .build_shared_graph(
            &second_graph,
            &NativeBuildOptions {
                output: root.path.join(if cfg!(target_os = "macos") {
                    "second.dylib"
                } else {
                    "second.so"
                }),
                profile,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("build second graph library");
    assert_ne!(
        first.module_symbol, second.module_symbol,
        "the host must be able to address both public wrappers independently"
    );
    assert_ne!(
        first.module_api_symbol, second.module_api_symbol,
        "owned-result accessors must not interpose across same-topology DSOs"
    );

    let first_header = first
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("first header name");
    let second_header = second
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("second header name");
    // Zig's Mach-O linker does not yet implement `-exported_symbols_list`
    // (ziglang/zig#24662). The generated module entries are still hidden by object
    // visibility, which is the interposition property this regression protects; exercise
    // the stronger provider-export policy whenever the selected linker can enforce it.
    let provider_visibility_checks =
        if cfg!(target_os = "macos") && toolchain.identity().kind == CompilerKind::Zig {
            ""
        } else {
            r#"
    if (dlsym(first, "kiln_number_to_js_string") != NULL ||
        dlsym(second, "kiln_number_to_js_string") != NULL ||
        dlsym(first, "kiln_string_to_number") != NULL ||
        dlsym(second, "kiln_string_to_number") != NULL ||
        dlsym(first, "kiln_realm_create") != NULL ||
        dlsym(second, "kiln_realm_create") != NULL ||
        dlsym(first, "kiln_realm_drive") != NULL ||
        dlsym(second, "kiln_realm_drive") != NULL)
        return 4;
"#
        };
    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "{first_header}"
#include "{second_header}"
#include <dlfcn.h>
#include <stdint.h>
#include <string.h>

typedef int (*kiln_entry_fn)(kiln_host *, kiln_value *);

static kiln_entry_fn load_entry(void *handle, const char *name) {{
    void *symbol = dlsym(handle, name);
    kiln_entry_fn entry = NULL;
    if (symbol == NULL || sizeof(entry) != sizeof(symbol)) return NULL;
    memcpy(&entry, &symbol, sizeof(entry));
    return entry;
}}

int main(int argc, char **argv) {{
    kiln_value result;
    void *first;
    void *second;
    kiln_entry_fn first_entry;
    kiln_entry_fn second_entry;
    if (argc != 3) return 1;
    first = dlopen(argv[1], RTLD_NOW | RTLD_GLOBAL);
    if (first == NULL) return 2;
    second = dlopen(argv[2], RTLD_NOW | RTLD_LOCAL);
    if (second == NULL) return 3;
    if (dlsym(first, "{internal}") != NULL || dlsym(second, "{internal}") != NULL)
        return 4;
    {provider_visibility_checks}
    first_entry = load_entry(first, "{first_symbol}");
    second_entry = load_entry(second, "{second_symbol}");
    if (first_entry == NULL || second_entry == NULL) return 5;
    if (first_entry(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({first_bits}))
        return 1;
    if (second_entry(NULL, &result) != KILN_C99_OK ||
        result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({second_bits}))
        return 2;
    {{
        const kiln_module_api_v2 *first_api = {first_api_symbol}();
        const kiln_module_api_v2 *second_api = {second_api_symbol}();
        kiln_result_v2 *first_owned = NULL;
        kiln_result_v2 *second_owned = NULL;
        if (first_api == NULL || second_api == NULL ||
            first_api->run(NULL, &first_owned) != KILN_C99_OK || first_owned == NULL ||
            second_api->run(NULL, &second_owned) != KILN_C99_OK || second_owned == NULL)
            return 3;
        if (first_api->read(first_owned, &result) != KILN_C99_OK ||
            result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({first_bits})) return 4;
        if (second_api->read(second_owned, &result) != KILN_C99_OK ||
            result.tag != KILN_VALUE_NUMBER || result.bits != UINT64_C({second_bits})) return 5;
        first_api->dispose(&first_owned);
        second_api->dispose(&second_owned);
        if (first_owned != NULL || second_owned != NULL) return 6;
        first_api->dispose(&first_owned);
        second_api->dispose(&second_owned);
    }}
    dlclose(second);
    dlclose(first);
    return 0;
}}
"#,
            first_symbol = first.module_symbol,
            second_symbol = second.module_symbol,
            first_api_symbol = first.module_api_symbol,
            second_api_symbol = second.module_api_symbol,
            internal = first_internal,
            first_bits = 11.0_f64.to_bits(),
            second_bits = 22.0_f64.to_bits(),
            provider_visibility_checks = provider_visibility_checks,
        ),
    )
    .expect("write visibility host");
    let host_object = root.path.join("host.o");
    let mut compile_options = CompileOptions::default();
    compile_options.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile_options)
        .expect("compile visibility host");
    let host_executable = root.path.join("host");
    let mut link_options = LinkOptions {
        runtime_libraries: vec![first.output.clone(), second.output.clone()],
        libraries: if cfg!(target_os = "linux") {
            vec!["dl".to_owned()]
        } else {
            Vec::new()
        },
        ..LinkOptions::default()
    };
    for directory in [
        first.loader.parent().expect("first loader directory"),
        second.loader.parent().expect("second loader directory"),
    ] {
        link_options
            .extra_args
            .push(format!("-Wl,-rpath,{}", directory.display()).into());
    }
    toolchain
        .link_executable(&[host_object], &host_executable, &link_options)
        .expect("link visibility host");
    let output = Command::new(&host_executable)
        .arg(&first.output)
        .arg(&second.output)
        .output()
        .expect("run visibility host");
    assert!(
        output.status.success(),
        "shared graph visibility host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn stdout_program(text: &str, result: f64) -> Program {
    Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![
            Constant::String(text.to_owned()),
            Constant::Number(result.to_bits()),
        ],
        code: vec![
            Instruction::Const(0),
            Instruction::HostCall {
                op: HostOp::WriteStdout,
                argc: 1,
            },
            Instruction::Pop,
            Instruction::Const(1),
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    }
}

#[test]
fn native_host_clock_is_explicit_while_default_date_stays_at_epoch() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native host-clock integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-host-clock");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![Constant::String("now".to_owned())],
        code: vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Date),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let deterministic = builder
        .run(
            &program,
            &NativeBuildOptions {
                output: root.path.join("deterministic"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
            },
        )
        .expect("deterministic Date program runs");
    assert_eq!(deterministic.result, Some(HostValue::number(0.0)));

    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock is after epoch")
        .as_secs_f64()
        * 1_000.0;
    let host_backed = builder
        .run(
            &program,
            &NativeBuildOptions {
                output: root.path.join("host"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::host_clock(),
                march_native: false,
            },
        )
        .expect("host-backed Date program runs");
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock is after epoch")
        .as_secs_f64()
        * 1_000.0;
    let observed = host_backed
        .result
        .as_ref()
        .and_then(HostValue::as_number)
        .expect("Date.now returns a number");
    assert_eq!(
        observed,
        observed.trunc(),
        "Date.now must return integral milliseconds"
    );
    assert!(
        observed >= before && observed <= after,
        "host Date.now {observed} outside build/run interval {before}..={after}"
    );
    assert_ne!(
        deterministic.build.module_symbol,
        host_backed.build.module_symbol
    );
}

#[test]
fn native_entropy_policy_is_reproducible_or_explicitly_host_backed() {
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native entropy integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-entropy");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: vec![Constant::String("random".to_owned())],
        code: vec![
            Instruction::LoadBuiltin(kiln_ir::Builtin::Math),
            Instruction::CallMethod { name: 0, argc: 0 },
            Instruction::Return,
        ],
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };
    let run = |name: &str, entropy: NativeEntropyPolicy| {
        builder
            .run(
                &program,
                &NativeBuildOptions::new(root.path.join(name), NativeProfile::Dev)
                    .with_realm_policy(NativeRealmPolicy::default().with_entropy(entropy)),
            )
            .unwrap_or_else(|error| panic!("native entropy fixture {name} failed: {error}"))
    };

    let deterministic = run("deterministic", NativeEntropyPolicy::Deterministic);
    let seeded_a = run(
        "seeded-a",
        NativeEntropyPolicy::Seeded(kiln_runtime::DEFAULT_ENTROPY_SEED),
    );
    let seeded_b = run(
        "seeded-b",
        NativeEntropyPolicy::Seeded(kiln_runtime::DEFAULT_ENTROPY_SEED),
    );
    let other_seed = run("seeded-other", NativeEntropyPolicy::Seeded(7));
    assert_eq!(deterministic.result, seeded_a.result);
    assert_eq!(seeded_a.result, seeded_b.result);
    assert_eq!(seeded_a.build.module_symbol, seeded_b.build.module_symbol);
    assert_ne!(seeded_a.build.module_symbol, other_seed.build.module_symbol);
    assert_ne!(seeded_a.result, other_seed.result);

    let host_a = run("host-a", NativeEntropyPolicy::Host);
    let host_b = run("host-b", NativeEntropyPolicy::Host);
    for report in [&host_a, &host_b] {
        let value = report
            .result
            .as_ref()
            .and_then(HostValue::as_number)
            .expect("Math.random returns a number");
        assert!((0.0..1.0).contains(&value));
    }
    assert_eq!(host_a.build.module_symbol, host_b.build.module_symbol);
    assert_ne!(
        host_a.build.module_symbol,
        deterministic.build.module_symbol
    );
}

#[test]
fn native_deterministic_entropy_is_one_stream_across_modules() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-graph-entropy");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    fs::write(
        root.path.join("first.mjs"),
        "export const first = Math.random();",
    )
    .expect("write first entropy consumer");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        r#"import { first } from "./first.mjs";
console.log(first === Math.random());"#,
    )
    .expect("write second entropy consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build entropy graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native graph entropy integration: no C compiler found");
        return;
    };
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    for (name, entropy) in [
        ("default", NativeEntropyPolicy::Deterministic),
        ("seeded", NativeEntropyPolicy::Seeded(7)),
    ] {
        let report = builder
            .run_graph(
                &graph,
                &NativeBuildOptions::new(root.path.join(name), NativeProfile::Dev)
                    .with_realm_policy(NativeRealmPolicy::default().with_entropy(entropy)),
            )
            .unwrap_or_else(|error| panic!("native graph entropy fixture failed: {error}"));
        assert_eq!(
            report.exit_code,
            Some(0),
            "stderr={}",
            String::from_utf8_lossy(&report.stderr)
        );
        assert_eq!(report.stdout, b"false\n");
    }
}

fn scalar_equivalence_programs() -> Vec<(&'static str, Program)> {
    fn push_constant(constants: &mut Vec<Constant>, value: Constant) -> u32 {
        let index = constants.len() as u32;
        constants.push(value);
        index
    }

    fn push_unary_result(
        constants: &mut Vec<Constant>,
        code: &mut Vec<Instruction>,
        operator: kiln_ir::UnaryOp,
        input: Constant,
    ) {
        let input = push_constant(constants, input);
        code.extend([Instruction::Const(input), Instruction::Unary(operator)]);
    }

    fn push_binary_result(
        constants: &mut Vec<Constant>,
        code: &mut Vec<Instruction>,
        operator: kiln_ir::BinaryOp,
        left: Constant,
        right: Constant,
    ) {
        let left = push_constant(constants, left);
        let right = push_constant(constants, right);
        code.extend([
            Instruction::Const(left),
            Instruction::Const(right),
            Instruction::Binary(operator),
        ]);
    }

    let string_numeric_inputs = [
        "",
        "\u{00a0}\u{1680}\u{2000}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}1\u{feff}",
        "\u{0085}1\u{0085}",
        "-0",
        "Infinity",
        "+Infinity",
        "-Infinity",
        "inf",
        "1e309",
        "-1e-9999",
        "5e-324",
        "0x10",
        "0b11",
        "0o10",
        "0x20000000000001",
        "0x20000000000003",
        "+0x10",
        "0x1p2",
        "1e+",
        "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    ];
    let mut string_numeric_code = Vec::with_capacity(string_numeric_inputs.len() * 2 + 2);
    for index in 0..string_numeric_inputs.len() {
        string_numeric_code.push(Instruction::Const(index as u32));
        string_numeric_code.push(Instruction::Unary(kiln_ir::UnaryOp::Plus));
    }
    string_numeric_code.push(Instruction::HostCall {
        op: HostOp::WriteStdout,
        argc: string_numeric_inputs.len() as u16,
    });
    string_numeric_code.push(Instruction::Return);
    let string_numeric_program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: string_numeric_inputs
            .into_iter()
            .map(|value| Constant::String(value.to_owned()))
            .collect(),
        code: string_numeric_code,
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    let number = |value: f64| Constant::Number(value.to_bits());
    let mut operator_constants = Vec::new();
    let mut operator_code = Vec::new();
    let mut operator_results = 0;
    for input in [
        Constant::Undefined,
        Constant::Null,
        Constant::Boolean(true),
        number(1.0),
        Constant::String("x".to_owned()),
    ] {
        push_unary_result(
            &mut operator_constants,
            &mut operator_code,
            kiln_ir::UnaryOp::TypeOf,
            input,
        );
        operator_results += 1;
    }
    for (operator, input) in [
        (kiln_ir::UnaryOp::BitwiseNot, number(5.0)),
        (kiln_ir::UnaryOp::BitwiseNot, number(4_294_967_296.0)),
        (kiln_ir::UnaryOp::Void, number(42.0)),
    ] {
        push_unary_result(&mut operator_constants, &mut operator_code, operator, input);
        operator_results += 1;
    }
    for (operator, left, right) in [
        (
            kiln_ir::BinaryOp::LooseEqual,
            Constant::Null,
            Constant::Undefined,
        ),
        (
            kiln_ir::BinaryOp::LooseEqual,
            number(0.0),
            Constant::Boolean(false),
        ),
        (
            kiln_ir::BinaryOp::LooseEqual,
            Constant::String("42".to_owned()),
            number(42.0),
        ),
        (
            kiln_ir::BinaryOp::LooseEqual,
            Constant::String(String::new()),
            number(0.0),
        ),
        (kiln_ir::BinaryOp::LooseEqual, Constant::Null, number(0.0)),
        (
            kiln_ir::BinaryOp::LooseNotEqual,
            number(f64::NAN),
            number(f64::NAN),
        ),
        (kiln_ir::BinaryOp::BitwiseAnd, number(5.0), number(3.0)),
        (kiln_ir::BinaryOp::BitwiseOr, number(5.0), number(2.0)),
        (kiln_ir::BinaryOp::BitwiseXor, number(5.0), number(3.0)),
        (kiln_ir::BinaryOp::ShiftLeft, number(1.0), number(31.0)),
        (kiln_ir::BinaryOp::ShiftLeft, number(1.0), number(32.0)),
        (kiln_ir::BinaryOp::ShiftRight, number(-1.0), number(1.0)),
        (
            kiln_ir::BinaryOp::UnsignedShiftRight,
            number(-1.0),
            number(1.0),
        ),
        (
            kiln_ir::BinaryOp::BitwiseOr,
            number(4_294_967_296.0),
            number(0.0),
        ),
        (kiln_ir::BinaryOp::BitwiseOr, number(-1.9), number(0.0)),
        (
            kiln_ir::BinaryOp::BitwiseOr,
            number(f64::INFINITY),
            number(0.0),
        ),
    ] {
        push_binary_result(
            &mut operator_constants,
            &mut operator_code,
            operator,
            left,
            right,
        );
        operator_results += 1;
    }
    operator_code.extend([
        Instruction::HostCall {
            op: HostOp::WriteStdout,
            argc: operator_results,
        },
        Instruction::Return,
    ]);
    let operator_program = Program {
        source: "".into(),
        version: BYTECODE_VERSION,
        constants: operator_constants,
        code: operator_code,
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
    };

    vec![
        (
            "loop",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::Number(0.0_f64.to_bits()),
                    Constant::Number(3.0_f64.to_bits()),
                    Constant::Number(1.0_f64.to_bits()),
                    Constant::String("loop".to_owned()),
                ],
                code: vec![
                    Instruction::Const(0),
                    Instruction::StoreLocal(0),
                    Instruction::LoadLocal(0),
                    Instruction::Const(1),
                    Instruction::Binary(kiln_ir::BinaryOp::LessThan),
                    Instruction::JumpIfFalse(11),
                    Instruction::LoadLocal(0),
                    Instruction::Const(2),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::StoreLocal(0),
                    Instruction::Jump(2),
                    Instruction::Const(3),
                    Instruction::LoadLocal(0),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 2,
                    },
                    Instruction::Return,
                ],
                local_count: 1,
                functions: Vec::new(),
                template_sites: Vec::new(),
            },
        ),
        (
            "concat",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::String("value=".to_owned()),
                    Constant::Number(40.0_f64.to_bits()),
                    Constant::Number(2.0_f64.to_bits()),
                ],
                code: vec![
                    Instruction::Const(0),
                    Instruction::Const(1),
                    Instruction::Const(2),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
                local_count: 0,
                functions: Vec::new(),
                template_sites: Vec::new(),
            },
        ),
        (
            "number-formatting",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::String("v=".to_owned()),
                    Constant::Number(0.1_f64.to_bits()),
                    Constant::Number((1.0_f64 / 3.0).to_bits()),
                    Constant::Number((-0.0_f64).to_bits()),
                    Constant::Number(1e20_f64.to_bits()),
                    Constant::Number(1e21_f64.to_bits()),
                    Constant::Number(1e-6_f64.to_bits()),
                    Constant::Number(1e-7_f64.to_bits()),
                    Constant::Number(1.2345e-7_f64.to_bits()),
                    Constant::Number(f64::from_bits(1).to_bits()),
                    Constant::Number(f64::NAN.to_bits()),
                    Constant::Number(f64::INFINITY.to_bits()),
                    Constant::Number(f64::NEG_INFINITY.to_bits()),
                ],
                code: vec![
                    // Keep the direct-number call separate from the concatenated call: the
                    // former exercises console formatting (including negative zero), while
                    // the latter exercises Number-to-string conversion in the C backend.
                    Instruction::Const(1),
                    Instruction::Const(2),
                    Instruction::Const(3),
                    Instruction::Const(4),
                    Instruction::Const(5),
                    Instruction::Const(6),
                    Instruction::Const(7),
                    Instruction::Const(8),
                    Instruction::Const(9),
                    Instruction::Const(10),
                    Instruction::Const(11),
                    Instruction::Const(12),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 12,
                    },
                    Instruction::Pop,
                    Instruction::Const(0),
                    Instruction::Const(1),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(2),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(3),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(4),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(5),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(6),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(7),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(8),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(9),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(10),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(11),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::Const(0),
                    Instruction::Const(12),
                    Instruction::Binary(kiln_ir::BinaryOp::Add),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 12,
                    },
                    Instruction::Return,
                ],
                local_count: 0,
                functions: Vec::new(),
                template_sites: Vec::new(),
            },
        ),
        ("scalar-operators", operator_program),
        ("string-numeric-literals", string_numeric_program),
        (
            "utf16-order",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::String("😀".to_owned()),
                    Constant::String("\u{e000}".to_owned()),
                ],
                code: vec![
                    Instruction::Const(0),
                    Instruction::Const(1),
                    Instruction::Binary(kiln_ir::BinaryOp::LessThan),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 1,
                    },
                    Instruction::Return,
                ],
                local_count: 0,
                functions: Vec::new(),
                template_sites: Vec::new(),
            },
        ),
        (
            "special-numbers",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                constants: vec![
                    Constant::Number(f64::NAN.to_bits()),
                    Constant::Number(f64::INFINITY.to_bits()),
                    Constant::Number((-0.0_f64).to_bits()),
                ],
                code: vec![
                    Instruction::Const(0),
                    Instruction::Const(1),
                    Instruction::Const(2),
                    Instruction::HostCall {
                        op: HostOp::WriteStdout,
                        argc: 3,
                    },
                    Instruction::Return,
                ],
                local_count: 0,
                functions: Vec::new(),
                template_sites: Vec::new(),
            },
        ),
    ]
}

/// Every native builtin body must fill exactly the slots the shared export table declares.
///
/// The bodies index `exports[]` by position, and the positions are decided in the leaf ABI
/// crate. Nothing makes that a compile error, and it has already
/// broken once: `node:http`'s body was written against a six-name list, the table later grew
/// three names at the front, and every slot shifted by two without a single warning. The
/// result was not a refusal but a wrong answer - `default` bound to `request`, `createServer`
/// bound to `get` - which is the failure mode this repository treats as the expensive one.
///
/// So the coupling is pinned here rather than trusted. Adding a name to the shared table now
/// fails this test, which is the cheapest place to find out.
#[test]
fn native_builtin_slots_match_the_shared_export_table() {
    use kiln_module_resolver::NodeBuiltin;

    for builtin in NodeBuiltin::ALL.iter().copied() {
        let Some(body) = native_builtin_body(builtin) else {
            // Resolver-known but not-yet-materialized builtins are deliberately rejected by
            // `validate_native_node_builtins`; they have no native slot contract to check.
            continue;
        };
        let specifier = builtin.specifier();
        let abi = kiln_node_abi::builtin_abi(specifier)
            .unwrap_or_else(|| panic!("{specifier} has no declared Node ABI row"));
        for supported in native_builtin_supported_exports(builtin) {
            assert!(
                abi.exports.contains(supported),
                "{specifier} claims native export `{supported}` outside its declared ABI {:?}",
                abi.exports
            );
        }

        let mut assigned: Vec<usize> = body
            .lines()
            .filter_map(|line| line.trim().strip_prefix("exports["))
            .filter_map(|rest| rest.split(']').next())
            .map(|index| {
                index
                    .parse()
                    .unwrap_or_else(|_| panic!("{specifier}: unparseable slot `{index}`"))
            })
            .collect();
        assigned.sort_unstable();

        let expected: Vec<usize> = (0..abi.exports.len()).collect();
        assert_eq!(
            assigned, expected,
            "{specifier} assigns slots {assigned:?} but its declared exports are {:?}; \
                 a duplicate slot is also invalid; \
                 a body that skips a slot leaves an export `undefined` and one that overruns \
                 writes past the array",
            abi.exports
        );
    }
}

#[test]
fn native_builtin_support_is_the_materialized_subset_not_the_declared_surface() {
    use kiln_module_resolver::NodeBuiltin;

    assert!(native_builtin_supports_export(
        NodeBuiltin::Fs,
        "existsSync"
    ));
    assert!(!native_builtin_supports_export(
        NodeBuiltin::Fs,
        "accessSync"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Path, "join"));
    assert!(native_builtin_supports_export(
        NodeBuiltin::Path,
        "basename"
    ));
    assert!(native_builtin_supports_export(NodeBuiltin::Path, "dirname"));
    assert!(native_builtin_supports_export(NodeBuiltin::Path, "format"));
    assert!(native_builtin_supports_export(NodeBuiltin::Os, "hostname"));
    assert!(native_builtin_supports_export(NodeBuiltin::Os, "platform"));
    assert!(!native_builtin_supports_export(
        NodeBuiltin::Os,
        "nonexistent"
    ));
    assert!(!native_builtin_supports_export(
        NodeBuiltin::Http,
        "IncomingMessage"
    ));
}

#[test]
fn resolver_and_node_abi_cover_the_same_builtin_set() {
    let resolver = kiln_module_resolver::NodeBuiltin::ALL
        .iter()
        .map(|builtin| builtin.specifier())
        .collect::<BTreeSet<_>>();
    let abi = kiln_node_abi::SPECIFIERS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(resolver, abi);
}

#[test]
fn executable_graph_omits_unused_modules_only_with_package_permission() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-side-effect-free-graph");
    let package = project.path.join("node_modules/example");
    fs::create_dir_all(&package).expect("create package");
    fs::write(
        package.join("package.json"),
        r#"{"name":"example","type":"module","main":"index.js","sideEffects":false}"#,
    )
    .expect("write package metadata");
    fs::write(
        package.join("index.js"),
        "export { used } from './used.js'; export { unused } from './unused.js';",
    )
    .expect("write package barrel");
    fs::write(package.join("used.js"), "export const used = 42;").expect("write used module");
    fs::write(
        package.join("unused.js"),
        "console.log('must be removable'); export const unused = 7;",
    )
    .expect("write unused module");
    let entry = project.path.join("main.mjs");
    fs::write(&entry, "import { used } from 'example'; console.log(used);").expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let selected =
        native_executable_graph(&graph, NativeProfile::Release).expect("select executable modules");
    let names = selected
        .modules
        .iter()
        .filter_map(|node| node.path.file_name())
        .collect::<BTreeSet<_>>();
    assert!(names.contains(std::ffi::OsStr::new("main.mjs")));
    assert!(names.contains(std::ffi::OsStr::new("used.js")));
    assert!(!names.contains(std::ffi::OsStr::new("index.js")));
    assert!(!names.contains(std::ffi::OsStr::new("unused.js")));
}

#[test]
fn executable_graph_keeps_unused_import_without_package_permission() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-side-effectful-graph");
    let package = project.path.join("node_modules/example");
    fs::create_dir_all(&package).expect("create package");
    fs::write(
        package.join("package.json"),
        r#"{"name":"example","type":"module","main":"index.js"}"#,
    )
    .expect("write package metadata");
    fs::write(
        package.join("index.js"),
        "console.log('observable'); export const unused = 7;",
    )
    .expect("write package module");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import { unused } from 'example'; console.log('entry');",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let selected =
        native_executable_graph(&graph, NativeProfile::Release).expect("select executable modules");
    assert_eq!(selected.modules.len(), 2);
    assert!(
        selected
            .modules
            .iter()
            .any(|node| node.path.ends_with("node_modules/example/index.js"))
    );
}

#[test]
fn executable_graph_demands_only_statically_read_namespace_members() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-static-namespace-graph");
    let package = project.path.join("node_modules/example");
    fs::create_dir_all(&package).expect("create package");
    fs::write(
        package.join("package.json"),
        r#"{"name":"example","type":"module","main":"index.js","sideEffects":false}"#,
    )
    .expect("write package metadata");
    fs::write(
        package.join("index.js"),
        "export * as helpers from './helpers.js';",
    )
    .expect("write namespace barrel");
    fs::write(
        package.join("helpers.js"),
        "export function used() { return 42; } export function unused() { return 7; }",
    )
    .expect("write namespace provider");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import { helpers } from 'example'; console.log(helpers.used());",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let selected =
        native_executable_graph(&graph, NativeProfile::Release).expect("select executable graph");
    let helpers = fs::canonicalize(package.join("helpers.js")).expect("canonical helpers");
    let helper_node = graph.module(&helpers).expect("helpers node");
    let used = helper_node
        .program
        .exports
        .iter()
        .find(|binding| binding.exported == "used")
        .expect("used export");
    let unused = helper_node
        .program
        .exports
        .iter()
        .find(|binding| binding.exported == "unused")
        .expect("unused export");
    let demanded = selected
        .live_export_slots
        .get(&helpers)
        .expect("namespace provider demands");
    assert!(demanded.contains(&used.local_slot));
    assert!(!demanded.contains(&unused.local_slot));
}

#[test]
fn executable_graph_keeps_a_namespace_import_captured_by_a_live_closure() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-captured-namespace-import");
    fs::write(project.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let helpers = project.path.join("helpers.js");
    fs::write(
        &helpers,
        "export function used() { return 42; } export function unused() { return 7; }",
    )
    .expect("write namespace provider");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import * as helpers from './helpers.js'; const invoke = () => helpers.used(); console.log(invoke());",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let selected =
        native_executable_graph(&graph, NativeProfile::Release).expect("select executable graph");
    let entry = fs::canonicalize(entry).expect("canonical entry");
    assert!(selected.live_imports.contains(&(entry, 0)));
    let helpers = fs::canonicalize(helpers).expect("canonical helpers");
    let members = selected
        .partial_namespace_members
        .get(&helpers)
        .expect("partial namespace demand");
    assert_eq!(members, &["used".to_owned()].into_iter().collect());
}

#[test]
fn executable_graph_full_namespace_demand_overrides_a_partial_demand() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-full-beats-partial-namespace");
    fs::write(project.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let helpers = project.path.join("helpers.js");
    fs::write(
        &helpers,
        "export function used() { return 42; } export function captured() { return 7; }",
    )
    .expect("write namespace provider");
    fs::write(
        project.path.join("capturing.js"),
        "import * as helpers from './helpers.js'; globalThis.invoke = () => helpers;",
    )
    .expect("write capturing importer");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import * as helpers from './helpers.js'; import './capturing.js'; console.log(helpers.used());",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let selected =
        native_executable_graph(&graph, NativeProfile::Release).expect("select executable graph");
    let helpers = fs::canonicalize(helpers).expect("canonical helpers");
    let helper_node = graph.module(&helpers).expect("helpers node");
    let demanded = selected
        .live_export_slots
        .get(&helpers)
        .expect("namespace provider demands");
    for export in &helper_node.program.exports {
        assert!(demanded.contains(&export.local_slot));
    }
    assert!(!selected.partial_namespace_members.contains_key(&helpers));
}

#[test]
fn release_graph_selects_the_production_arm_of_an_exact_commonjs_dispatcher() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-production-dispatch");
    let entry = project.path.join("main.cjs");
    fs::write(&entry, "module.exports = require('./dispatch.cjs');").expect("write entry");
    fs::write(
        project.path.join("dispatch.cjs"),
        r#"'use strict';
if (process.env.NODE_ENV === 'production') {
  module.exports = require('./production.cjs');
} else {
  module.exports = require('./development.cjs');
}
"#,
    )
    .expect("write dispatcher");
    fs::write(
        project.path.join("production.cjs"),
        "module.exports = 'production';",
    )
    .expect("write production module");
    fs::write(
        project.path.join("development.cjs"),
        "module.exports = 'development';",
    )
    .expect("write development module");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let release =
        native_executable_graph(&graph, NativeProfile::Release).expect("select release graph");
    let release_names = release
        .modules
        .iter()
        .filter_map(|node| node.path.file_name())
        .collect::<BTreeSet<_>>();
    assert!(release_names.contains(std::ffi::OsStr::new("production.cjs")));
    assert!(!release_names.contains(std::ffi::OsStr::new("development.cjs")));
    assert_eq!(release.inactive_commonjs_dependencies.len(), 1);

    let dev = native_executable_graph(&graph, NativeProfile::Dev).expect("select dev graph");
    let dev_names = dev
        .modules
        .iter()
        .filter_map(|node| node.path.file_name())
        .collect::<BTreeSet<_>>();
    assert!(dev_names.contains(std::ffi::OsStr::new("production.cjs")));
    assert!(dev_names.contains(std::ffi::OsStr::new("development.cjs")));
    assert!(dev.inactive_commonjs_dependencies.is_empty());
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
    fs::write(&entry, "console.log('workers-ok');").expect("write fixture");
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
fn native_node_os_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-os-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import os, { platform, homedir, EOL } from 'node:os';\n",
            "console.log('os-test:', typeof platform(), typeof homedir(), EOL === '\\n', typeof os.tmpdir());\n",
        ),
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
            &NativeBuildOptions::new(project.path.join("os_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run os binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "os-test: string string true string");
}

#[test]
fn native_node_os_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-os-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const os = require('node:os');\n",
            "console.log('cjs-os:', typeof os.platform, typeof os.homedir, os.EOL === '\\n', typeof os.tmpdir);\n",
        ),
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
            &NativeBuildOptions::new(project.path.join("os_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run os cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-os: function function true function");
}

#[test]
fn native_node_path_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-path-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import path, { join, resolve, dirname, basename, extname, parse, format, posix, sep } from 'node:path';\n",
            "console.log('path-test:', join('a', 'b'), resolve('/x', 'y'), dirname('/a/b/c.js'), basename('a/b.txt'), extname('a.txt'), sep === '/');\n",
            "console.log('parse-test:', JSON.stringify(parse('/a/b/c.txt')));\n",
            "console.log('format-test:', format({ dir: '/x/y', base: 'z.txt' }));\n",
            "console.log('posix-test:', path.posix === path);\n",
        ),
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
            &NativeBuildOptions::new(project.path.join("path_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run path binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(
        stdout.trim(),
        concat!(
            "path-test: a/b /x/y /a/b b.txt .txt true\n",
            "parse-test: {\"root\":\"/\",\"dir\":\"/a/b\",\"base\":\"c.txt\",\"ext\":\".txt\",\"name\":\"c\"}\n",
            "format-test: /x/y/z.txt\n",
            "posix-test: true"
        )
    );
}

#[test]
fn native_node_path_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-path-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const path = require('node:path');\n",
            "console.log('cjs-path:', path.join('foo', 'bar'), path.posix === path, path.default === path);\n",
        ),
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
            &NativeBuildOptions::new(project.path.join("path_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run path cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-path: foo/bar true true");
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
fn number_provider_variant_selection_is_conservative() {
    use kiln_module_resolver::NodeRuntimeResolver;

    fn selection(entry_name: &str, files: &[(&str, &str)]) -> bool {
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
        NativeBuilder::graph_wants_http_provider(&graph)
    }

    assert!(
        !selection("main.mjs", &[("main.mjs", "console.log(1 + 2);\n")]),
        "a plain ESM entry links the slim archive"
    );
    assert!(
        !selection(
            "main.mjs",
            &[
                (
                    "main.mjs",
                    "import { a } from './dep.mjs';\nconsole.log(a);\n"
                ),
                ("dep.mjs", "export const a = 1;\n"),
            ]
        ),
        "an HTTP-free static graph links the slim archive"
    );
    assert!(
        selection(
            "main.mjs",
            &[(
                "main.mjs",
                "import http from 'node:http';\nconsole.log(typeof http.createServer);\n",
            )]
        ),
        "node:http selects the full archive"
    );
    assert!(
        selection(
            "main.mjs",
            &[(
                "main.mjs",
                "import https from 'node:https';\nconsole.log(typeof https);\n",
            )]
        ),
        "node:https selects the full archive"
    );
    assert!(
        selection(
            "main.mjs",
            &[(
                "main.mjs",
                "import { dlopen } from 'node:ffi';\nconsole.log(typeof dlopen);\n",
            )]
        ),
        "node:ffi selects the full archive"
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
        ),
        "a dynamic import selects the full archive"
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
        ),
        "a CommonJS require selects the full archive"
    );
}

#[test]
fn native_node_assert_typescript_builtin_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-assert-ts");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        concat!(
            "import assert, { ok, strict, strictEqual, deepStrictEqual, throws, doesNotThrow, AssertionError } from 'node:assert';\n",
            "assert(true);\n",
            "ok(true);\n",
            "strict(true);\n",
            "strictEqual(10 + 20, 30);\n",
            "deepStrictEqual({ x: [1, 2] }, { x: [1, 2] });\n",
            "doesNotThrow(() => 42);\n",
            "let caught = false;\n",
            "try { strictEqual(1, 2); } catch (e) { caught = e instanceof AssertionError; }\n",
            "console.log('assert-test:', caught);\n",
        ),
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
            &NativeBuildOptions::new(project.path.join("assert_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run assert binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "assert-test: true");
}

#[test]
fn native_node_assert_commonjs_require_builds_and_runs() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-node-assert-cjs");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        concat!(
            "const assert = require('node:assert');\n",
            "assert(true);\n",
            "assert.strictEqual(3 * 3, 9);\n",
            "console.log('cjs-assert:', typeof assert, assert.name, assert.default === assert, assert.strict === assert.strict.strict);\n",
        ),
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
            &NativeBuildOptions::new(project.path.join("assert_cjs_bin"), NativeProfile::Release),
        )
        .unwrap();
    let result = Command::new(&report.output)
        .output_when_launchable()
        .expect("run assert cjs binary");
    assert!(
        result.status.success(),
        "process failed: stderr={}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert_eq!(stdout.trim(), "cjs-assert: function assert true true");
}
