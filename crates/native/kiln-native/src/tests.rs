use super::*;
use kiln_ir::{
    BYTECODE_VERSION, Constant, ExportBinding, FunctionProto, HostOp, ImportBinding, Instruction,
    ModuleProgram,
};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier};
use std::time::{SystemTime, UNIX_EPOCH};

fn empty_function() -> FunctionProto {
    FunctionProto {
        inferred_name: None,
        span: kiln_ir::Span::default(),
        prologue_end: 0,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        parameter_count: 0,
        declared_length: 0,
        mapped_arguments: false,
        local_count: 0,
        code: vec![Instruction::Return],
        upvalues: Vec::new(),
        type_annotations: None,
    }
}

fn budget_module(program: Program) -> ModuleProgram {
    let mut module = kiln_ir::compile_module(&kiln_frontend_oxc::parse("main.mjs", "")).unwrap();
    module.program = program;
    module
}

struct OxcFrontend;

impl kiln_module_graph::Frontend for OxcFrontend {
    fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
        kiln_frontend_oxc::parse(path, source)
    }

    fn parse_commonjs(&mut self, path: &str, source: &str) -> kiln_module_graph::CommonJsParse {
        let (module, static_requires) =
            kiln_frontend_oxc::parse_commonjs_with_requires(path, source);
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
            if let Some(configured) = std::env::var_os("KILN_NATIVE_TEST_CACHE_ROOT") {
                let configured = PathBuf::from(configured);
                assert!(
                    !configured.as_os_str().is_empty(),
                    "KILN_NATIVE_TEST_CACHE_ROOT must not be empty"
                );
                assert!(
                    configured.is_absolute(),
                    "KILN_NATIVE_TEST_CACHE_ROOT must be an absolute path: {}",
                    configured.display()
                );
                fs::create_dir_all(&configured).unwrap_or_else(|error| {
                    panic!(
                        "cannot create KILN_NATIVE_TEST_CACHE_ROOT {}: {error}",
                        configured.display()
                    )
                });
                return configured;
            }

            if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
                let target_cache = Path::new(&manifest_dir).join("../../../target/kiln-test-cache");
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
        let path = fs::canonicalize(&path).unwrap_or(path);
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

/// How long a fixture program may run before it counts as hung. Generous for a loaded machine;
/// the point is that a fixture that never exits -- a server whose request never ends -- fails
/// naming itself instead of holding the whole suite (and the pre-push hook) indefinitely.
const FIXTURE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// Run a fixture program built moments ago and collect its output, failing the test if it has not
/// exited by [`FIXTURE_DEADLINE`]. Readers drain both pipes while it runs, so a fixture that
/// prints more than a pipe holds cannot stall either.
fn fixture_output(mut command: std::process::Command, what: &str) -> std::process::Output {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn_when_launchable()
        .unwrap_or_else(|error| panic!("run {what}: {error}"));
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    };
    let stdout = drain(Box::new(child.stdout.take().expect("fixture stdout is piped")));
    let stderr = drain(Box::new(child.stderr.take().expect("fixture stderr is piped")));
    let deadline = Instant::now() + FIXTURE_DEADLINE;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap_or_else(|error| panic!("poll {what}: {error}")) {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = stdout.join().unwrap_or_default();
            let stderr = stderr.join().unwrap_or_default();
            panic!(
                "{what} did not exit within {FIXTURE_DEADLINE:?}; stdout so far {:?}, stderr {:?}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    std::process::Output {
        status,
        stdout: stdout.join().expect("stdout reader"),
        stderr: stderr.join().expect("stderr reader"),
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native node:http graph builds");
    let output = fixture_output(Command::new(&report.output), "native node:http fixture");
    assert!(
        output.status.success(),
        "status={} stdout={:?} stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8(output.stdout).expect("fixture output is UTF-8"))
}

fn run_native_release_fixture(name: &str, source: &str) -> Option<String> {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native release fixture: no C compiler found");
        return None;
    };
    let project = TestDirectory::new(name);
    let entry = project.path.join("main.mjs");
    fs::write(&entry, source).expect("write native release fixture");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("release source graph compiles");
    let report = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions {
                output: project.path.join("program"),
                profile: NativeProfile::Release,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native release graph builds");
    let run = |stress: bool| {
        let mut command = Command::new(&report.output);
        if stress {
            command.env("KILN_GC_STRESS", "1");
        } else {
            command.env_remove("KILN_GC_STRESS");
        }
        fixture_output(command, "native release fixture")
    };
    let normal = run(false);
    assert!(
        normal.status.success(),
        "normal status={} stdout={:?} stderr={:?}",
        normal.status,
        String::from_utf8_lossy(&normal.stdout),
        String::from_utf8_lossy(&normal.stderr)
    );
    let stress = run(true);
    assert!(
        stress.status.success(),
        "stress status={} stdout={:?} stderr={:?}",
        stress.status,
        String::from_utf8_lossy(&stress.stdout),
        String::from_utf8_lossy(&stress.stderr)
    );
    assert_eq!(
        stress.stdout, normal.stdout,
        "GC stress changed native fixture output"
    );
    Some(String::from_utf8(normal.stdout).expect("fixture output is UTF-8"))
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native diagnostic graph builds");
    let output = fixture_output(Command::new(&report.output), "native diagnostic fixture");
    assert!(!output.status.success(), "fixture unexpectedly succeeded");
    Some(String::from_utf8(output.stderr).expect("diagnostic output is UTF-8"))
}

/// A rejected top-level await while a handle keeps the loop alive. The launcher used to read the
/// entry promise only after the turn loop went quiescent, which a timer or a listening server
/// never lets it do, so the process hung where node exits with the error. Bounded wait, because
/// the regression *is* a hang.

fn run_native_failure_fixture_with_dependency(
    name: &str,
    source: &str,
    dependency_source: &str,
    profile: NativeProfile,
) -> Option<String> {
    use kiln_module_resolver::NodeRuntimeResolver;
    use std::process::Command;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native diagnostic fixture: no C compiler found");
        return None;
    };
    let project = TestDirectory::new(name);
    fs::write(project.path.join("main.mjs"), source).expect("write native diagnostic fixture");
    fs::write(project.path.join("dep.mjs"), dependency_source)
        .expect("write native diagnostic dependency");
    let entry = project.path.join("main.mjs");
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
                profile,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("native diagnostic graph builds");
    let output = fixture_output(Command::new(&report.output), "native diagnostic fixture");
    assert!(!output.status.success(), "fixture unexpectedly succeeded");
    Some(String::from_utf8(output.stderr).expect("diagnostic output is UTF-8"))
}

fn environment_digest(kind: CompilerKind, entries: &[(&str, &str)]) -> Digest {
    let values = entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), std::ffi::OsString::from(value)))
        .collect::<std::collections::HashMap<_, _>>();
    compiler_environment_digest_with(kind, |name| values.get(name).cloned())
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../conformance/server/packages");
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
                ..NativeBuildOptions::default()
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
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
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
            debug_call_sites: Vec::new(),
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
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![
                Constant::String("dependency microtask".into()),
                Constant::String("dependency timer".into()),
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
                    inferred_name: None,
                    span: kiln_ir::Span::default(),
                    prologue_end: 0,
                    asynchronous: false,
                    name: None,
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    is_method: false,
                    is_class: false,
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
                    inferred_name: None,
                    span: kiln_ir::Span::default(),
                    prologue_end: 0,
                    asynchronous: false,
                    name: None,
                    arrow: false,
                    generator: false,
                    async_generator: false,
                    is_method: false,
                    is_class: false,
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
            debug_call_sites: Vec::new(),
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
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::String("entry body".into()), Constant::Undefined],
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
            debug_call_sites: Vec::new(),
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

/// A compiled binary is a single-executable application in node's sense, so it reports the
/// command line the way node does for one: `argv[0]` and `argv[1]` both name the executable,
/// and the program's own arguments start at index 2. That is not a cosmetic choice -- every
/// argument-reading program, and `const [, , first] = process.argv`, counts from there, and a
/// one-element argv would silently hand each of them the wrong argument.
///
/// Run through `Command` rather than `run_graph`, because the whole question is what the OS
/// put in `main`'s `argv` and there is no other way to put anything there.

fn star_reexport_graph(project: &Path) -> ModuleGraph {
    fs::create_dir_all(project).expect("create star graph project");
    for name in ["origin.js", "barrel.js", "main.js"] {
        fs::write(project.join(name), "").expect("write star graph module");
    }
    let origin = ModuleProgram {
        program: Program {
            source: "".into(),
            version: BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
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
            debug_call_sites: Vec::new(),
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
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![Constant::Undefined],
            code: vec![Instruction::Const(0), Instruction::Return],
            local_count: 1,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
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
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
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
            debug_call_sites: Vec::new(),
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
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
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
            debug_call_sites: Vec::new(),
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
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
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
            debug_call_sites: Vec::new(),
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

/// A CommonJS entry completes with `module.exports`, which is an object unless the module
/// assigns a scalar to it. The result file carries only the five boundary scalars, so the
/// launcher declines to write an object -- and used to charge that refusal to the program,
/// which exited 1 after printing exactly the right bytes and saying nothing on stderr. The
/// run succeeded and simply has no scalar to report.

#[test]
#[ignore = "diagnostic probe for the native Fastify constructor"]
fn native_fastify_constructor_probe() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: vec![
            Constant::String(text.into()),
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
        debug_call_sites: Vec::new(),
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: string_numeric_inputs
            .into_iter()
            .map(|value| Constant::String(value.into()))
            .collect(),
        code: string_numeric_code,
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
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
        Constant::String("x".into()),
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
            Constant::String("42".into()),
            number(42.0),
        ),
        (
            kiln_ir::BinaryOp::LooseEqual,
            Constant::String(String::new().into()),
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
        source_origin: None,
        static_source_identity: None,
        static_source_provenance: None,
        constants: operator_constants,
        code: operator_code,
        local_count: 0,
        functions: Vec::new(),
        template_sites: Vec::new(),
        debug_call_sites: Vec::new(),
    };

    vec![
        (
            "loop",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                source_origin: None,
                static_source_identity: None,
                static_source_provenance: None,
                constants: vec![
                    Constant::Number(0.0_f64.to_bits()),
                    Constant::Number(3.0_f64.to_bits()),
                    Constant::Number(1.0_f64.to_bits()),
                    Constant::String("loop".into()),
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
                debug_call_sites: Vec::new(),
            },
        ),
        (
            "concat",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                source_origin: None,
                static_source_identity: None,
                static_source_provenance: None,
                constants: vec![
                    Constant::String("value=".into()),
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
                debug_call_sites: Vec::new(),
            },
        ),
        (
            "number-formatting",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                source_origin: None,
                static_source_identity: None,
                static_source_provenance: None,
                constants: vec![
                    Constant::String("v=".into()),
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
                debug_call_sites: Vec::new(),
            },
        ),
        ("scalar-operators", operator_program),
        ("string-numeric-literals", string_numeric_program),
        (
            "utf16-order",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                source_origin: None,
                static_source_identity: None,
                static_source_provenance: None,
                constants: vec![
                    Constant::String("😀".into()),
                    Constant::String("\u{e000}".into()),
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
                debug_call_sites: Vec::new(),
            },
        ),
        (
            "special-numbers",
            Program {
                source: "".into(),
                version: BYTECODE_VERSION,
                source_origin: None,
                static_source_identity: None,
                static_source_provenance: None,
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
                debug_call_sites: Vec::new(),
            },
        ),
    ]
}

mod compilation;
mod emission_determinism;
mod module_graph;
mod node_builtins;
mod node_http;
mod runtime;
mod state_owners;
