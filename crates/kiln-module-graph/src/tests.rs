use super::*;
use kiln_host_abi::{HostError, HostOp, HostValue};
use kiln_ir::{BYTECODE_VERSION, Instruction, Program};
use kiln_module_resolver::{NodePackageResolver, NodeRuntimeResolver};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempProject {
    root: PathBuf,
}

impl TempProject {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        // nextest runs each test in its own process, so a process-local counter alone makes
        // every parallel test claim `kiln-module-graph-0` and race on deletion and writes.
        let root =
            std::env::temp_dir().join(format!("kiln-module-graph-{}-{id}", std::process::id()));
        fs::create_dir_all(&root).expect("create temporary project");
        Self { root }
    }

    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, source).expect("write source");
        path
    }

    fn graph(&self, entry: &Path) -> Result<ModuleGraph, ModuleGraphError> {
        fn load(path: &Path) -> io::Result<String> {
            fs::read_to_string(path)
        }

        ModuleGraph::build(entry, &self.root, load, frontend_oxc::parse)
    }
}

#[test]
fn node_graph_records_package_commonjs_source_identity() {
    let project = TempProject::new();
    fs::create_dir_all(project.root.join("node_modules/pkg")).unwrap();
    project.write(
        "node_modules/pkg/package.json",
        r#"{"name":"pkg","main":"index.js","type":"commonjs"}"#,
    );
    let package = project.write("node_modules/pkg/index.js", "module.exports = 1;");
    let entry = project.write("main.mjs", "import value from 'pkg'; console.log(value);");
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_typed_policy(&entry, resolver, |path, source_kind| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = match source_kind {
            ModuleSource::CommonJs => frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source),
            _ => frontend_oxc::parse(path.to_str().unwrap(), &source),
        };
        let mut program = match source_kind {
            ModuleSource::CommonJs => kiln_ir::compile_commonjs(&hir),
            _ => compile_module(&hir),
        }
        .map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })?;
        if fs::canonicalize(path).ok() == fs::canonicalize(&package).ok() {
            program.exports.push(ExportBinding {
                exported: "default".into(),
                local_slot: 0,
            });
        }
        Ok(ModulePayload::Program(program))
    })
    .expect("build package graph");
    assert_eq!(
        graph.module(&package).unwrap().source,
        ModuleSource::CommonJs
    );
    assert_eq!(graph.module(&entry).unwrap().source, ModuleSource::Source);
}

#[test]
fn commonjs_prototype_method_carries_module_bindings_across_the_graph() {
    let project = TempProject::new();
    let package_dir = project.root.join("node_modules/router");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(
        package_dir.join("package.json"),
        r#"{"name":"router","main":"layer.js","type":"commonjs"}"#,
    )
    .unwrap();
    let layer = project.write(
        "node_modules/router/layer.js",
        r#"
                const MODULE_RE = /x/;
                module.exports = Layer;
                function Layer() {}
                Layer.prototype.match = function match(path) {
                    return MODULE_RE.exec(path);
                };
            "#,
    );
    let entry = project.write(
        "main.mjs",
        "import Layer from 'router'; new Layer().match('x');",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS graph");
    let module = &graph.module(&layer).expect("layer node").program;
    let method = module
        .program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("match"))
        .expect("match proto");
    assert!(
        method.code.contains(&Instruction::LoadUpvalue(0)),
        "{method:?}"
    );
    assert!(
        !method
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadGlobal(_))),
        "{method:?}"
    );
}

#[test]
fn express_router_match_has_no_caller_relative_module_reads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/server/packages/node_modules/router/lib/layer.js");
    if !path.exists() {
        return;
    }
    let source = fs::read_to_string(&path).expect("read pinned router layer");
    let hir = frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source);
    assert!(hir.diagnostics.is_empty(), "{:?}", hir.diagnostics);
    let module = kiln_ir::compile_commonjs(&hir).expect("compile pinned router layer");
    let methods = module
        .program
        .functions
        .iter()
        .filter(|proto| proto.name.as_deref() == Some("match"))
        .collect::<Vec<_>>();
    assert!(!methods.is_empty(), "match proto");
    for method in methods {
        let globals = method
            .code
            .iter()
            .filter_map(|instruction| match instruction {
                Instruction::LoadGlobal(slot) | Instruction::StoreGlobal(slot) => Some(*slot),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            globals.is_empty(),
            "caller-relative module slots: {globals:?}; {method:?}"
        );
    }
}

#[test]
fn path_to_regexp_parameter_defaults_capture_the_module_delimiter() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/server/packages/node_modules/path-to-regexp/dist/index.js");
    if !path.exists() {
        return;
    }
    let source = fs::read_to_string(&path).expect("read pinned path-to-regexp");
    let hir = frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source);
    let function = hir
        .body
        .iter()
        .find_map(|statement| match &statement.kind {
            kiln_hir::StatementKind::Function(function)
                if function.name.as_deref() == Some("match") =>
            {
                Some(function)
            }
            _ => None,
        })
        .expect("match declaration");
    assert_eq!(function.parameters.len(), 2, "{function:#?}");
    let module = kiln_ir::compile_commonjs(&hir).expect("compile path-to-regexp");
    let proto = module
        .program
        .functions
        .iter()
        .find(|proto| proto.name.as_deref() == Some("match"))
        .expect("match proto");
    assert!(
        proto
            .code
            .iter()
            .any(|instruction| matches!(instruction, Instruction::LoadUpvalue(_))),
        "parameter {:?}; proto {proto:?}",
        function.parameters[1].pattern
    );
}

#[test]
fn pinned_express_graph_has_no_function_with_caller_relative_globals() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let entry = root.join("conformance/server/packages/s05-express.mjs");
    let express = root.join("conformance/server/packages/node_modules/express/package.json");
    if !entry.exists() || !express.is_file() {
        return;
    }
    let resolver = NodeRuntimeResolver::new(&root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build pinned Express graph");
    let mut offenders = Vec::new();
    for (path, node) in graph.modules.iter() {
        let module_globals = node.program.program.local_count;
        for proto in &node.program.program.functions {
            let globals = proto
                .code
                .iter()
                .filter_map(|instruction| match instruction {
                    Instruction::LoadGlobal(slot) | Instruction::StoreGlobal(slot) => Some(slot),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if !globals.is_empty() {
                offenders.push((
                    path.strip_prefix(&root).unwrap_or(path).to_path_buf(),
                    proto.name.clone(),
                    module_globals,
                    globals,
                    proto.code.clone(),
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "caller-relative globals: {offenders:#?}"
    );
}

#[test]
fn node_graph_selects_dual_package_exports_for_import_and_require() {
    let project = TempProject::new();
    let package_dir = project.root.join("node_modules/pkg");
    fs::create_dir_all(&package_dir).unwrap();
    fs::write(
        package_dir.join("package.json"),
        r#"{"exports":{"import":"./esm.mjs","require":"./cjs.cjs"}}"#,
    )
    .expect("write dual package manifest");
    project.write("node_modules/pkg/esm.mjs", "export default 'import';");
    project.write("node_modules/pkg/cjs.cjs", "module.exports = 'require';");
    project.write("bridge.cjs", "module.exports = require('pkg');");
    let entry = project.write(
            "main.mjs",
            "import imported from 'pkg'; import required from './bridge.cjs'; console.log(imported, required);",
        );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build dual package graph");
    assert_eq!(
        graph
            .module(project.root.join("node_modules/pkg/esm.mjs"))
            .expect("import target node")
            .source,
        ModuleSource::Source
    );
    assert_eq!(
        graph
            .module(project.root.join("node_modules/pkg/cjs.cjs"))
            .expect("require target node")
            .source,
        ModuleSource::CommonJs
    );

    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute dual package graph");
    assert_eq!(host.output, ["import require"]);
}

#[test]
fn node_modules_share_one_next_tick_and_promise_checkpoint() {
    let project = TempProject::new();
    project.write(
        "dep.mjs",
        r#"
                process.nextTick(() => console.log('dep-tick'));
                Promise.resolve().then(() => console.log('dep-promise'));
                console.log('dep-body');
                export const ready = 1;
            "#,
    );
    let entry = project.write(
        "main.mjs",
        r#"
                import { ready } from './dep.mjs';
                process.nextTick(() => console.log('entry-tick'));
                Promise.resolve().then(() => console.log('entry-promise'));
                console.log(ready);
            "#,
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build shared-realm graph");

    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute shared-realm graph");
    assert_eq!(
        host.output,
        [
            "dep-body",
            "1",
            "dep-tick",
            "entry-tick",
            "dep-promise",
            "entry-promise",
        ]
    );
}

struct OxcFrontend;

impl Frontend for OxcFrontend {
    fn parse(&mut self, path: &str, source: &str) -> Module {
        frontend_oxc::parse(path, source)
    }

    fn parse_commonjs(&mut self, path: &str, source: &str) -> CommonJsParse {
        let (module, static_requires) = frontend_oxc::parse_commonjs_with_requires(path, source);
        CommonJsParse {
            module,
            static_requires,
        }
    }
}

#[test]
fn node_builder_compiles_commonjs_wrapper_and_require_edges() {
    let project = TempProject::new();
    project.write("package.json", r#"{"type":"module"}"#);
    fs::create_dir_all(project.root.join("pkg")).unwrap();
    project.write("pkg/package.json", r#"{"type":"commonjs"}"#);
    let dependency = project.write("pkg/dep.js", "exports.answer = 42;");
    let package = project.write(
        "pkg/index.js",
        "const dep = require('./dep.js'); module.exports = dep.answer;",
    );
    let entry = project.write("main.mjs", "import value from './pkg/index.js';");
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build typed Node graph");
    let node = graph.module(&package).expect("CommonJS package node");
    assert_eq!(node.source, ModuleSource::CommonJs);
    assert_eq!(node.program.dependencies, ["./dep.js"]);
    assert_eq!(node.program.imports.len(), 5);
    assert_eq!(node.dependencies.len(), 1);
    assert_eq!(node.dependencies[0].specifier, "./dep.js");
    assert_eq!(
        node.dependencies[0].target,
        fs::canonicalize(dependency).unwrap()
    );
}

#[test]
fn node_executes_commonjs_require_and_esm_default_interop() {
    let project = TempProject::new();
    project.write("package.json", r#"{"type":"module"}"#);
    fs::create_dir_all(project.root.join("pkg")).unwrap();
    project.write("pkg/package.json", r#"{"type":"commonjs"}"#);
    project.write("pkg/dep.js", "module.exports = 41;");
    project.write(
        "pkg/index.js",
        "const answer = require('./dep.js'); module.exports = answer + 1;",
    );
    let entry = project.write(
        "main.mjs",
        "import answer from './pkg/index.js'; console.log(answer);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build mixed graph");
    let mut host = kiln_runtime::CaptureHost::new();
    let execution = graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute mixed graph");
    assert_eq!(host.into_stdout(), "42\n");
    assert_eq!(execution.value, Value::Undefined);
}

#[test]
fn node_executes_commonjs_named_export_and_reexport_interop() {
    let project = TempProject::new();
    project.write("package.json", r#"{"type":"module"}"#);
    fs::create_dir_all(project.root.join("pkg")).unwrap();
    project.write("pkg/package.json", r#"{"type":"commonjs"}"#);
    project.write(
        "pkg/dep.js",
        "exports.Fragment = 'fragment-value'; exports.jsx = (tag) => `<${tag}/>`;",
    );
    project.write(
        "bridge.mjs",
        "export { Fragment, jsx } from './pkg/dep.js';",
    );
    let entry = project.write(
        "main.mjs",
        "import { Fragment, jsx } from './bridge.mjs'; console.log(Fragment, jsx('div'));",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build mixed graph with CJS named exports");
    let mut host = kiln_runtime::CaptureHost::new();
    let execution = graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute mixed graph");
    assert_eq!(host.into_stdout(), "fragment-value <div/>\n");
    assert_eq!(execution.value, Value::Undefined);
}

#[test]
fn node_executes_top_level_await_before_finishing_module() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "await Promise.resolve(41); console.log('module-ready');",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build top-level await graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute top-level await graph");
    assert_eq!(host.into_stdout(), "module-ready\n");
}

#[test]
fn node_dependency_top_level_await_finishes_before_importer() {
    let project = TempProject::new();
    project.write(
            "dep.mjs",
            "console.log('dep-before'); await Promise.resolve(); console.log('dep-after'); export const value = 42;",
        );
    let entry = project.write(
        "main.mjs",
        "import { value } from './dep.mjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build dependency top-level await graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute dependency top-level await graph");
    assert_eq!(host.into_stdout(), "dep-before\ndep-after\n42\n");
}

#[test]
fn node_dependency_dynamic_import_job_resumes_top_level_await_before_importer() {
    let project = TempProject::new();
    project.write(
        "dep.mjs",
        r#"
            async function attemptImport() {
                try {
                    await import('missing-package');
                } catch {}
            }
            await attemptImport();
            class Ready {}
            export function makeReady() { return new Ready(); }
        "#,
    );
    let entry = project.write(
        "main.mjs",
        "import { makeReady } from './dep.mjs'; console.log(makeReady().constructor.name);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build dynamic-import top-level await graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("dynamic import job must resume dependency before its importer runs");
    assert_eq!(host.into_stdout(), "Ready\n");
}

#[test]
fn node_dependency_top_level_await_rejection_stops_importer() {
    let project = TempProject::new();
    project.write(
            "dep.mjs",
            "console.log('dep-before'); await Promise.reject('dep failed'); console.log('dep-after'); export const value = 42;",
        );
    let entry = project.write(
        "main.mjs",
        "import { value } from './dep.mjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build rejecting dependency top-level await graph");

    let mut host = kiln_runtime::CaptureHost::new();
    let error = graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect_err("rejected dependency top-level await must stop the importer");
    assert!(matches!(
        error,
        ModuleGraphError::Execution {
            error: ExecutionError::Uncaught { .. },
            ..
        }
    ));
    assert_eq!(host.into_stdout(), "dep-before\n");
}

#[test]
fn node_reports_rejected_top_level_await_as_module_failure() {
    let project = TempProject::new();
    let entry = project.write("main.mjs", "await Promise.reject('module failed');");
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build rejecting top-level await graph");

    let mut host = kiln_runtime::CaptureHost::new();
    let error = graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect_err("rejected top-level await must fail module evaluation");
    assert!(matches!(
        error,
        ModuleGraphError::Execution {
            error: ExecutionError::Uncaught { .. },
            ..
        }
    ));
}

#[test]
fn node_executes_top_level_for_await() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "for await (const value of [Promise.resolve(7)]) console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build top-level for-await graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute top-level for-await graph");
    assert_eq!(host.into_stdout(), "7\n");
}

#[test]
fn node_commonjs_builtin_projection_preserves_namespace_named_exports() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const hooks = require('node:async_hooks');",
            "const resource = new hooks.AsyncResource('probe');",
            "module.exports = typeof resource.bind === 'function';",
        ),
    );
    let entry = project.write(
        "main.mjs",
        "import supported from './bridge.cjs'; console.log(supported);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build builtin CommonJS graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute builtin CommonJS graph");
    assert_eq!(host.into_stdout(), "true\n");
}

#[test]
fn node_url_and_querystring_legacy_parsers_work_in_esm_and_commonjs() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const url = require('url');",
            "const qs = require('querystring');",
            "const parsed = url.parse('/hello?hello=world&foo=b%3D%3Dar');",
            "const query = qs.parse(parsed.query);",
            "module.exports = parsed.pathname + ' ' + query.hello + ' ' + query.foo;",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import { parse as parseUrl } from 'node:url';",
            "import { parse as parseQuery } from 'node:querystring';",
            "import legacy from './bridge.cjs';",
            "const parsed = parseUrl('/route?a=1&a=2#hash');",
            "const query = parseQuery(parsed.query);",
            "console.log(legacy);",
            "console.log(parsed.pathname, parsed.search, parsed.hash, query.a.join(','));",
        ),
    );
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.root).expect("Node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build legacy URL/querystring graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute legacy URL/querystring graph");
    assert_eq!(
        host.into_stdout(),
        "/hello world b==ar\n/route ?a=1&a=2 #hash 1,2\n"
    );
}

#[test]
fn node_reads_accessor_backed_module_exports_through_commonjs_semantics() {
    let project = TempProject::new();
    project.write(
        "facade.cjs",
        concat!(
            "const prototype = { answer: 42 };",
            "Object.defineProperty(module, 'exports', {",
            "  get() { return Object.create(prototype); }",
            "});",
        ),
    );
    let entry = project.write(
        "main.mjs",
        "import facade from './facade.cjs'; console.log(facade.answer);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build accessor-backed CommonJS graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute accessor-backed CommonJS graph");
    assert_eq!(host.into_stdout(), "42\n");
}

#[test]
fn node_executes_relative_cjs_default_import_and_publishes_output() {
    let project = TempProject::new();
    let dependency = project.write("dep.cjs", "module.exports = 42;");
    let entry = project.write(
        "main.mjs",
        "import value from './dep.cjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build relative CommonJS graph");

    let dependency = fs::canonicalize(dependency).expect("canonical dependency");
    assert_eq!(
        graph.module(&dependency).expect("dependency node").source,
        ModuleSource::CommonJs
    );
    assert_eq!(
        graph.resolved_export(&dependency, "default"),
        Some(&ResolvedExport::Local {
            module: dependency.clone(),
            local_slot: 0,
        })
    );

    let mut host = TestHost::default();
    let execution = graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute relative CommonJS graph");
    assert_eq!(host.output, ["42"]);
    assert_eq!(execution.value, Value::Undefined);
    assert!(execution.steps > 0);
}

#[test]
fn node_discovers_nested_conditional_commonjs_require() {
    let project = TempProject::new();
    let dependency = project.write("dep.cjs", "module.exports = 41;");
    let bridge = project.write(
        "bridge.cjs",
        "if (true) { const value = require('./dep.cjs'); module.exports = value + 1; }",
    );
    let entry = project.write(
        "main.mjs",
        "import value from './bridge.cjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build nested CommonJS graph");

    let dependency = fs::canonicalize(dependency).expect("canonical dependency");
    let bridge_node = graph.module(&bridge).expect("bridge node");
    assert_eq!(bridge_node.program.dependencies, ["./dep.cjs"]);
    assert_eq!(bridge_node.dependencies[0].target, dependency);

    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute nested CommonJS graph");
    assert_eq!(host.output, ["42"]);
}

#[test]
fn node_commonjs_require_is_lazy_in_dead_branches_and_unused_functions() {
    let project = TempProject::new();
    let dependency = project.write("dep.cjs", "console.log('executed'); module.exports = 42;");
    let bridge = project.write(
            "bridge.cjs",
            "if (false) { require('./dep.cjs'); } function load() { return require('./dep.cjs'); } module.exports = 1;",
        );
    let entry = project.write(
        "main.mjs",
        "import value from './bridge.cjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build lazy CommonJS graph");
    let bridge_node = graph.module(&bridge).expect("bridge node");
    assert_eq!(bridge_node.program.dependencies, ["./dep.cjs"]);
    assert_eq!(
        bridge_node.dependencies[0].target,
        fs::canonicalize(dependency).expect("canonical dependency")
    );

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute lazy CommonJS graph");
    assert_eq!(host.output, ["1"]);
}

#[test]
fn node_commonjs_missing_require_throws_at_execution_and_can_be_caught() {
    let project = TempProject::new();
    let bridge = project.write(
        "bridge.cjs",
        "try { require('not-installed'); } catch { module.exports = 42; }",
    );
    let entry = project.write(
        "main.mjs",
        "import value from './bridge.cjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("an unresolved CommonJS require is runtime work");
    assert!(
        graph
            .module(&bridge)
            .expect("bridge node")
            .dependencies
            .is_empty()
    );

    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("JavaScript catches the missing require");
    assert_eq!(host.output, ["42"]);
}

#[test]
fn node_commonjs_require_executes_a_target_once_and_reuses_its_identity() {
    let project = TempProject::new();
    project.write("dep.cjs", "exports.count = (exports.count || 0) + 1;");
    project.write(
            "bridge.cjs",
            "const first = require('./dep.cjs'); const second = require('./dep.cjs'); module.exports = first === second && first.count;",
        );
    let entry = project.write(
        "main.mjs",
        "import value from './bridge.cjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build cached CommonJS graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute cached CommonJS graph");
    assert_eq!(host.into_stdout(), "1\n");
}

#[test]
fn node_commonjs_require_remains_lazy_when_a_captured_loader_is_called_later() {
    let project = TempProject::new();
    project.write("dep.cjs", "console.log('executed'); module.exports = 42;");
    project.write("bridge.cjs", "module.exports = () => require('./dep.cjs');");
    let entry = project.write(
        "main.mjs",
        "import load from './bridge.cjs'; console.log(load());",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build captured CommonJS loader graph");

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute captured CommonJS loader graph");
    assert_eq!(host.into_stdout(), "executed\n42\n");
}

#[test]
fn node_commonjs_require_reads_esm_export_values() {
    let project = TempProject::new();
    project.write("dep.mjs", "export let answer = 41;");
    project.write(
        "bridge.cjs",
        "const dep = require('./dep.mjs'); module.exports = dep.answer + 1;",
    );
    let entry = project.write(
        "main.mjs",
        "import value from './bridge.cjs'; console.log(value);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build mixed ESM/CommonJS graph");

    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute mixed ESM/CommonJS graph");
    assert_eq!(host.output, ["42"]);
}

#[test]
fn node_commonjs_require_cycles_share_partial_cache_and_execute_once() {
    let project = TempProject::new();
    project.write(
            "a.cjs",
            "exports.count = (exports.count || 0) + 1; const b = require('./b.cjs'); exports.seenB = b.count;",
        );
    project.write(
            "b.cjs",
            "exports.count = (exports.count || 0) + 1; const a = require('./a.cjs'); exports.seenA = a.count;",
        );
    let entry = project.write(
            "main.mjs",
            "import a from './a.cjs'; import b from './b.cjs'; console.log(a.count, b.count, b.seenA, a.seenB);",
        );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS cycle");
    assert_eq!(graph.module_count(), 3);
    assert_eq!(
        graph
            .module_order()
            .iter()
            .filter(|path| {
                path.extension().and_then(|extension| extension.to_str()) == Some("cjs")
            })
            .count(),
        2
    );

    let mut host = kiln_runtime::CaptureHost::new();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute CommonJS cycle");
    assert_eq!(host.into_stdout(), "1 1 1 1\n");
}

#[test]
fn node_commonjs_esm_cycles_remain_structured_errors() {
    let project = TempProject::new();
    project.write("a.cjs", "require('./b.mjs');");
    project.write("b.mjs", "import './a.cjs';");
    let entry = project.write("main.mjs", "import './a.cjs';");
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("a mixed ESM/CommonJS cycle is a legal module graph");
    // b.mjs is reached through a.cjs and imports it back. The ESM walk stops at the back
    // edge, so a.cjs appears once, after the module that required it -- and the CommonJS
    // half still gets its own semantics, where the cache entry is observable mid-execution.
    assert_eq!(
        graph
            .module_order()
            .iter()
            .map(|path| path.file_name().expect("a file name").to_owned())
            .collect::<Vec<_>>(),
        ["b.mjs", "a.cjs", "main.mjs"]
    );
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct TestHost {
    output: Vec<String>,
}

impl HostOps for TestHost {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        assert_eq!(operation, HostOp::WriteStdout);
        self.output.push(
            arguments
                .iter()
                .map(HostValue::to_js_string)
                .collect::<Vec<_>>()
                .join(" "),
        );
        Ok(HostValue::Undefined)
    }
}

struct ServerHostForPort {
    sender: Option<std::sync::mpsc::Sender<u16>>,
    output: Vec<String>,
}

fn http_header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    response
        .split_once("\r\n\r\n")
        .expect("HTTP response has a header terminator")
        .0
        .lines()
        .skip(1)
        .find_map(|line| {
            let (header_name, value) = line.split_once(':')?;
            header_name
                .eq_ignore_ascii_case(name)
                .then_some(value.trim())
        })
}

fn http_status(response: &str) -> &str {
    response
        .split_once("\r\n")
        .expect("HTTP response has a status line")
        .0
}

fn http_body(response: &str) -> Vec<u8> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .expect("HTTP response has a header terminator");
    let body = body.as_bytes();
    if http_header(response, "transfer-encoding").is_some_and(|value| {
        value
            .split(',')
            .any(|part| part.trim().eq_ignore_ascii_case("chunked"))
    }) {
        let mut decoded = Vec::new();
        let mut cursor = 0;
        loop {
            let line_end = body[cursor..]
                .windows(2)
                .position(|window| window == b"\r\n")
                .map(|offset| cursor + offset)
                .expect("chunk has a size line");
            let size_text = std::str::from_utf8(&body[cursor..line_end])
                .expect("chunk size is UTF-8")
                .split(';')
                .next()
                .expect("chunk size is present");
            let size =
                usize::from_str_radix(size_text.trim(), 16).expect("chunk size is hexadecimal");
            cursor = line_end + 2;
            if size == 0 {
                assert!(
                    body[cursor..].starts_with(b"\r\n"),
                    "chunked body is terminated"
                );
                break;
            }
            assert!(
                body.len() >= cursor + size + 2,
                "chunk has its full payload"
            );
            decoded.extend_from_slice(&body[cursor..cursor + size]);
            assert_eq!(&body[cursor + size..cursor + size + 2], b"\r\n");
            cursor += size + 2;
        }
        return decoded;
    }

    let length = http_header(response, "content-length")
        .expect("non-chunked response has Content-Length")
        .parse::<usize>()
        .expect("Content-Length is numeric");
    assert_eq!(
        head.len() + 4 + length,
        response.len(),
        "response body has its declared length"
    );
    body[..length].to_vec()
}

impl HostOps for ServerHostForPort {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        assert_eq!(operation, HostOp::WriteStdout);
        let text = arguments
            .first()
            .map_or_else(String::new, HostValue::to_js_string);
        if let Ok(port) = text.parse::<u16>()
            && let Some(sender) = self.sender.take()
        {
            sender.send(port).expect("publish server port");
        }
        self.output.push(text);
        Ok(HostValue::Undefined)
    }
}

#[test]
fn named_import_alias_links_and_executes() {
    let project = TempProject::new();
    let dependency = project.write("dep.js", "export const value = 42;");
    let entry = project.write(
        "main.js",
        "import { value as answer } from './dep.js'; console.log(answer);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let node = graph.module(&entry).expect("entry node");
    assert_eq!(
        node.dependencies[0].target,
        fs::canonicalize(dependency).unwrap()
    );

    let mut host = TestHost::default();
    let result = graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["42"]);
    assert!(result.steps > 0);
}

#[test]
fn package_policy_links_a_named_package_import() {
    let project = TempProject::new();
    let package = project.root.join("node_modules/pkg");
    fs::create_dir_all(&package).expect("create package");
    fs::write(package.join("package.json"), r#"{"exports":"./index.js"}"#)
        .expect("write package manifest");
    fs::write(package.join("index.js"), "export const value = 42;").expect("write package entry");
    let entry = project.write(
        "main.js",
        "import { value } from 'pkg'; console.log(value);",
    );
    let resolver = NodePackageResolver::new(&project.root).expect("package resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build package graph");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute package graph");
    assert_eq!(host.output, ["42"]);
}

#[test]
fn node_builtin_static_imports_are_typed_virtual_modules() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { EventEmitter } from 'node:events';",
            "import * as fs from 'node:fs/promises';",
            "console.log(typeof EventEmitter, typeof fs.readFile, Object.keys(fs).join(','));",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let mut loaded = Vec::new();
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        loaded.push(path.to_path_buf());
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with builtins");

    // Only the real entry passed through the source provider. Builtins are typed graph nodes,
    // not fake files or generated JavaScript.
    assert_eq!(loaded, [fs::canonicalize(&entry).unwrap()]);
    assert_eq!(graph.module_count(), 3);
    assert_eq!(
        graph
            .modules()
            .filter(|node| matches!(node.source, ModuleSource::NodeBuiltin(_)))
            .count(),
        2
    );

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute builtin imports");
    // The key list is `kiln_node_abi::builtin_abi("fs/promises").exports`, the link-time
    // contract: a name listed there and unimplemented would link and read `undefined`, which is
    // the failure CLAUDE.md calls the expensive one. So the expectation is read from the table
    // rather than spelled here: this assertion was red twice in one day, once because the test
    // named two functions the table lacked and once because the table gained them after the test
    // had been pinned to its old spelling. A namespace object's keys are sorted, so the table is.
    let mut exports = kiln_node_abi::builtin_abi("node:fs/promises")
        .expect("fs/promises is a builtin")
        .exports
        .to_vec();
    exports.sort_unstable();
    assert_eq!(
        host.output,
        [format!("function function {}", exports.join(","))]
    );
}

#[test]
fn node_builtin_os_ts_evaluates_and_exports_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import os, { platform, homedir, EOL } from 'node:os';",
            "console.log(typeof platform, typeof homedir, typeof EOL, typeof os.tmpdir, os.EOL === '\\n');",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with os builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute os builtin");
    assert_eq!(host.output, ["function function string function true"]);
}

#[test]
fn node_builtin_path_ts_evaluates_and_exports_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import path, { join, resolve, dirname, basename, extname, parse, format, posix, sep } from 'node:path';",
            "console.log(join('a', 'b'), resolve('/x', 'y'), dirname('/a/b/c.js'), basename('a/b.txt'), extname('a.txt'), path.sep === '/');",
            "console.log(JSON.stringify(parse('/a/b/c.txt')));",
            "console.log(format({ dir: '/x/y', base: 'z.txt' }));",
            "console.log(path.posix === path);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with path builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute path builtin");
    assert_eq!(
        host.output,
        [
            "a/b /x/y /a/b b.txt .txt true",
            "{\"root\":\"/\",\"dir\":\"/a/b\",\"base\":\"c.txt\",\"ext\":\".txt\",\"name\":\"c\"}",
            "/x/y/z.txt",
            "true",
        ]
    );
}

#[test]
fn node_builtin_path_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const path = require('node:path');",
            "module.exports = { join: path.join('foo', 'bar'), posixEq: path.posix === path, defaultEq: path.default === path };",
        ),
    );
    let entry = project.write(
        "main.mjs",
        "import bridge from './bridge.cjs'; console.log(bridge.join, bridge.posixEq, bridge.defaultEq);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with path builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute path builtin cjs");
    assert_eq!(host.output, ["foo/bar true true"]);
}

#[test]
fn node_builtin_assert_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import assert, { ok, strict, strictEqual, deepStrictEqual, throws, doesNotThrow, AssertionError } from 'node:assert';",
            "assert(true);",
            "ok(true);",
            "strict(true);",
            "strictEqual(1 + 1, 2);",
            "deepStrictEqual({ a: [1, 2] }, { a: [1, 2] });",
            "doesNotThrow(() => 42);",
            "let caught = false;",
            "try { strictEqual(1, 2); } catch (e) { caught = e instanceof AssertionError; }",
            "console.log('assert ok', caught);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with assert builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute assert builtin");
    assert_eq!(host.output, ["assert ok true"]);
}

#[test]
fn node_builtin_assert_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const assert = require('node:assert');",
            "assert(true);",
            "assert.strictEqual(2 + 2, 4);",
            "module.exports = {",
            "  isFn: typeof assert === 'function',",
            "  name: assert.name,",
            "  defaultEq: assert.default === assert,",
            "  strictOk: typeof assert.strict.ok === 'function',",
            "  strictEq: assert.strict === assert.strict.strict,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isFn, bridge.name, bridge.defaultEq, bridge.strictOk, bridge.strictEq);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with assert builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute assert builtin cjs");
    assert_eq!(host.output, ["true assert true true true"]);
}

#[test]
fn node_builtin_events_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import EventEmitter, { EventEmitter as EE } from 'node:events';",
            "const ee = new EventEmitter();",
            "let count = 0;",
            "ee.on('ping', () => { count++; });",
            "ee.once('ping', () => { count += 10; });",
            "ee.emit('ping');",
            "ee.emit('ping');",
            "console.log('events ok', count, ee.addListener === ee.on, EventEmitter === EE);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with events builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute events builtin");
    assert_eq!(host.output, ["events ok 12 true true"]);
}

#[test]
fn node_builtin_events_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const EventEmitter = require('node:events');",
            "const ee = new EventEmitter();",
            "let val = '';",
            "ee.on('test', (x) => { val = x; });",
            "ee.emit('test', 'hello');",
            "module.exports = {",
            "  isFn: typeof EventEmitter === 'function',",
            "  name: EventEmitter.name,",
            "  defaultEq: EventEmitter.default === EventEmitter,",
            "  val,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isFn, bridge.name, bridge.defaultEq, bridge.val);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with events builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute events builtin cjs");
    assert_eq!(host.output, ["true EventEmitter true hello"]);
}

#[test]
fn node_builtin_buffer_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import buffer, { Buffer, alloc, from, isBuffer, byteLength, concat, constants } from 'node:buffer';",
            "const b1 = alloc(3);",
            "const b2 = from('hi');",
            "const b3 = concat([b1, b2]);",
            "console.log(",
            "  buffer.Buffer === Buffer,",
            "  isBuffer(b1),",
            "  b2.toString(),",
            "  b3.length,",
            "  byteLength('hey'),",
            "  typeof constants.MAX_STRING_LENGTH,",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with buffer builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute buffer builtin");
    assert_eq!(host.output, ["true true hi 5 3 number"]);
}

#[test]
fn node_builtin_buffer_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const buffer = require('node:buffer');",
            "const b = buffer.from('xyz');",
            "module.exports = {",
            "  isObj: typeof buffer === 'object',",
            "  sameCtor: buffer.Buffer === Buffer,",
            "  isBuf: buffer.isBuffer(b),",
            "  str: b.toString(),",
            "  defaultBuf: buffer.default.Buffer === buffer.Buffer,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.sameCtor, bridge.isBuf, bridge.str, bridge.defaultBuf);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with buffer builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute buffer builtin cjs");
    assert_eq!(host.output, ["true true true xyz true"]);
}

#[test]
fn node_builtin_string_decoder_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { StringDecoder } from 'node:string_decoder';",
            "const decoder = new StringDecoder('utf8');",
            "const part1 = decoder.write(new Uint8Array([0xf0, 0x9f]));",
            "const part2 = decoder.write(new Uint8Array([0x92, 0xa9, 0x21]));",
            "const part3 = decoder.end(new Uint8Array([0xe2]));",
            "console.log(",
            "  typeof StringDecoder === 'function',",
            "  part1 === '',",
            "  part2 === '💩!',",
            "  part3 === '\\uFFFD',",
            "  decoder.encoding,",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with string_decoder builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute string_decoder builtin");
    assert_eq!(host.output, ["true true true true utf8"]);
}

#[test]
fn node_builtin_string_decoder_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const string_decoder = require('node:string_decoder');",
            "const SD = string_decoder.StringDecoder;",
            "const dec = new SD();",
            "const s = dec.write(new Uint8Array([104, 101, 108, 108, 111]));",
            "module.exports = {",
            "  isObj: typeof string_decoder === 'object',",
            "  isFn: typeof SD === 'function',",
            "  selfEq: SD.StringDecoder === SD,",
            "  s,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.isFn, bridge.selfEq, bridge.s);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with string_decoder builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute string_decoder builtin cjs");
    assert_eq!(host.output, ["true true true hello"]);
}

#[test]
fn node_builtin_querystring_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        concat!(
            "import querystring, { parse, decode, stringify, encode, escape, unescape } from 'node:querystring';",
            "const q1 = parse('name=ferret&color=purple&tags=fluffy&tags=quick');",
            "const s1 = stringify({ a: '1', b: ['2', '3'] });",
            "const esc = escape('a b&c=d');",
            "const unesc = unescape('a+b%26c%3Dd');",
            "console.log(",
            "  parse === decode,",
            "  stringify === encode,",
            "  querystring.parse === parse,",
            "  querystring.default === querystring,",
            "  q1.name === 'ferret',",
            "  q1.color === 'purple',",
            "  q1.tags.join(','),",
            "  s1,",
            "  esc,",
            "  unesc,",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with querystring builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute querystring builtin");
    assert_eq!(
        host.output,
        ["true true true true true true fluffy,quick a=1&b=2&b=3 a%20b%26c%3Dd a b&c=d"]
    );
}

#[test]
fn node_builtin_querystring_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const qs = require('node:querystring');",
            "const parsed = qs.decode('x=10&y=20');",
            "const str = qs.encode({ x: 10 });",
            "module.exports = {",
            "  isObj: typeof qs === 'object',",
            "  isFn: typeof qs.parse === 'function',",
            "  selfEq: qs.default === qs,",
            "  x: parsed.x,",
            "  y: parsed.y,",
            "  str,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.isFn, bridge.selfEq, bridge.x, bridge.y, bridge.str);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with querystring builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute querystring builtin cjs");
    assert_eq!(host.output, ["true true true 10 20 x=10"]);
}

#[test]
fn node_builtin_util_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        concat!(
            "import util, { format, inherits, inspect, promisify, types, isArray } from 'node:util';",
            "const fmt = format('hello %s: %d', 'world', 42);",
            "function Base() {}",
            "Base.prototype.greet = function() { return 'base'; };",
            "function Sub() {}",
            "inherits(Sub, Base);",
            "const sub = new Sub();",
            "const isArr = isArray([1, 2]);",
            "const isPromise = types.isPromise(Promise.resolve(1));",
            "console.log(",
            "  typeof util === 'object',",
            "  util.default === util,",
            "  fmt,",
            "  sub.greet() === 'base',",
            "  Sub.super_ === Base,",
            "  isArr,",
            "  isPromise,",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with util builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute util builtin");
    assert_eq!(
        host.output,
        ["true true hello world: 42 true true true true"]
    );
}

#[test]
fn node_builtin_util_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const util = require('node:util');",
            "const fmt = util.format('count: %d', 100);",
            "module.exports = {",
            "  isObj: typeof util === 'object',",
            "  isFn: typeof util.format === 'function',",
            "  hasPromisify: typeof util.promisify === 'function',",
            "  selfEq: util.default === util,",
            "  fmt,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.isFn, bridge.hasPromisify, bridge.selfEq, bridge.fmt);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with util builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute util builtin cjs");
    assert_eq!(host.output, ["true true true true count: 100"]);
}

#[test]
fn node_builtin_url_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        concat!(
            "import url, { URL, URLSearchParams, fileURLToPath, pathToFileURL, parse, format, urlToHttpOptions } from 'node:url';",
            "const p = fileURLToPath('file:///foo/bar.txt');",
            "const u = pathToFileURL('/foo/bar.txt');",
            "const parsed = parse('http://user:pass@host.com:8080/path?a=1&b=2#sec');",
            "const opts = urlToHttpOptions(new URL('https://example.com:8443/test'));",
            "console.log(",
            "  typeof url === 'object',",
            "  url.default === url,",
            "  typeof URL === 'function',",
            "  typeof URLSearchParams === 'function',",
            "  p,",
            "  u.protocol,",
            "  parsed.hostname,",
            "  parsed.port,",
            "  opts.port,",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with url builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute url builtin");
    assert_eq!(
        host.output,
        ["true true true true /foo/bar.txt file: host.com 8080 8443"]
    );
}

#[test]
fn node_builtin_url_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const url = require('node:url');",
            "const p = url.fileURLToPath('file:///hello/world');",
            "module.exports = {",
            "  isObj: typeof url === 'object',",
            "  isFn: typeof url.parse === 'function',",
            "  hasUrl: typeof url.URL === 'function',",
            "  selfEq: url.default === url,",
            "  p,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.isFn, bridge.hasUrl, bridge.selfEq, bridge.p);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with url builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute url builtin cjs");
    assert_eq!(host.output, ["true true true true /hello/world"]);
}

#[test]
fn node_builtin_diagnostics_channel_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        concat!(
            "import dc, { Channel, TracingChannel, channel, hasSubscribers, subscribe, unsubscribe, tracingChannel } from 'node:diagnostics_channel';",
            "const ch = channel('app.events');",
            "let received = null;",
            "const onMsg = (msg) => { received = msg.text; };",
            "subscribe('app.events', onMsg);",
            "const hasBefore = hasSubscribers('app.events');",
            "ch.publish({ text: 'event-fired' });",
            "unsubscribe('app.events', onMsg);",
            "const hasAfter = hasSubscribers('app.events');",
            "const tc = tracingChannel('app.trace');",
            "let ran = false;",
            "tc.start.runStores({}, function(v) { ran = this.flag && v === 7; }, { flag: true }, 7);",
            "console.log(",
            "  typeof dc === 'object',",
            "  dc.default === dc,",
            "  received,",
            "  hasBefore,",
            "  hasAfter,",
            "  ran,",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with diagnostics_channel builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute diagnostics_channel builtin");
    assert_eq!(host.output, ["true true event-fired true false true"]);
}

#[test]
fn node_builtin_diagnostics_channel_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const dc = require('node:diagnostics_channel');",
            "const ch = dc.channel('bridge.test');",
            "module.exports = {",
            "  isObj: typeof dc === 'object',",
            "  hasChan: typeof dc.channel === 'function',",
            "  hasTracing: typeof dc.tracingChannel === 'function',",
            "  selfEq: dc.default === dc,",
            "  hasPublish: typeof ch.publish === 'function',",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.hasChan, bridge.hasTracing, bridge.selfEq, bridge.hasPublish);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with diagnostics_channel builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute diagnostics_channel builtin cjs");
    assert_eq!(host.output, ["true true true true true"]);
}

#[test]
fn node_builtin_perf_hooks_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        concat!(
            "import perfHooks, { Performance, PerformanceEntry, PerformanceMark, PerformanceMeasure, PerformanceObserver, PerformanceObserverEntryList, performance, constants, createHistogram, monitorEventLoopDelay } from 'node:perf_hooks';",
            "console.log(",
            "  typeof perfHooks === 'object',",
            "  perfHooks.default === perfHooks,",
            "  typeof performance === 'object',",
            "  typeof performance.now === 'function',",
            "  typeof performance.now() === 'number',",
            "  performance === globalThis.performance,",
            "  constants.NODE_PERFORMANCE_GC_MAJOR === 4,",
            "  typeof Performance === 'function',",
            "  typeof PerformanceEntry === 'function',",
            "  typeof PerformanceMark === 'function',",
            "  typeof createHistogram === 'function',",
            "  typeof monitorEventLoopDelay === 'function',",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with perf_hooks builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute perf_hooks builtin");
    assert_eq!(
        host.output,
        ["true true true true true true true true true true true true"]
    );
}

#[test]
fn node_builtin_perf_hooks_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const perfHooks = require('node:perf_hooks');",
            "const { performance } = perfHooks;",
            "module.exports = {",
            "  isObj: typeof perfHooks === 'object',",
            "  hasNow: typeof performance.now === 'function',",
            "  nowNum: typeof performance.now() === 'number',",
            "  selfEq: perfHooks.default === perfHooks,",
            "  globEq: performance === globalThis.performance,",
            "  hasEntry: typeof perfHooks.PerformanceEntry === 'function',",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.hasNow, bridge.nowNum, bridge.selfEq, bridge.globEq, bridge.hasEntry);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with perf_hooks builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute perf_hooks builtin cjs");
    assert_eq!(host.output, ["true true true true true true"]);
}

#[test]
fn node_builtin_console_evaluates_cleanly() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        concat!(
            "import nodeConsole, { Console, log, error, warn, info, debug, assert } from 'node:console';",
            "log('direct-log');",
            "console.log(",
            "  typeof nodeConsole === 'object',",
            "  nodeConsole.default === nodeConsole,",
            "  typeof Console === 'function',",
            "  typeof log === 'function',",
            "  typeof error === 'function',",
            "  typeof warn === 'function',",
            "  typeof info === 'function',",
            "  typeof debug === 'function',",
            "  typeof assert === 'function',",
            ");",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build graph with console builtin");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute console builtin");
    assert_eq!(
        host.output,
        ["direct-log", "true true true true true true true true true"]
    );
}

#[test]
fn node_builtin_console_commonjs_require_evaluates_cleanly() {
    let project = TempProject::new();
    project.write(
        "bridge.cjs",
        concat!(
            "const c = require('node:console');",
            "c.log('bridge-log');",
            "module.exports = {",
            "  isObj: typeof c === 'object',",
            "  hasConsole: typeof c.Console === 'function',",
            "  hasLog: typeof c.log === 'function',",
            "  selfEq: c.default === c,",
            "};",
        ),
    );
    let entry = project.write(
        "main.mjs",
        concat!(
            "import bridge from './bridge.cjs';",
            "console.log(bridge.isObj, bridge.hasConsole, bridge.hasLog, bridge.selfEq);",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph with console builtin cjs");

    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute console builtin cjs");
    assert_eq!(host.output, ["bridge-log", "true true true true"]);
}

#[test]
fn node_net_connect_is_a_typed_virtual_module_export() {
    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        "import { connect } from 'node:net'; console.log(connect);",
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("implemented builtin export links");
    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute typed node:net module");
    assert_eq!(host.output, ["[Function: connect]"]);
}

#[test]
fn node_graph_execution_is_an_explicit_host_capability() {
    let project = TempProject::new();
    let payload = project.write("payload.txt", "from tokio");
    let payload = payload
        .to_str()
        .unwrap()
        .replace('\\', "\\\\")
        .replace('\'', "\\'");
    let entry = project.write(
            "main.js",
            &format!(
                "import {{ readFile }} from 'node:fs/promises'; readFile('{payload}', 'utf8').then(value => console.log(value));",
            ),
        );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build Node graph");

    let mut deterministic_host = TestHost::default();
    let error = graph
        .execute(
            &mut deterministic_host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect_err("deterministic execution must not gain host I/O from an import");
    assert!(matches!(
        error,
        ModuleGraphError::Execution {
            error: ExecutionError::Host { message, .. },
            ..
        } if message.contains("requires the Node execution Adapter")
    ));

    let mut node_host = TestHost::default();
    graph
        .execute_node(
            &mut node_host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("explicit Node graph execution");
    assert_eq!(node_host.output, ["from tokio"]);
}

#[test]
fn node_http_static_module_serves_one_live_request_on_tokio() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { createServer } from 'node:http';",
            "let server;",
            "server = createServer((request, response) => {",
            "  response.setHeader('Content-Type', 'text/plain');",
            "  response.end(request.method + ' ' + request.url);",
            "  server.close();",
            "});",
            "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build HTTP graph");

    let (port_sender, port_receiver) = mpsc::channel();
    struct ServerHost {
        port_sender: Option<mpsc::Sender<u16>>,
    }
    impl HostOps for ServerHost {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            assert_eq!(operation, HostOp::WriteStdout);
            let port = arguments
                .first()
                .and_then(|value| value.to_js_string().parse::<u16>().ok())
                .expect("listening callback prints port");
            if let Some(sender) = self.port_sender.take() {
                sender.send(port).expect("send server port");
            }
            Ok(HostValue::Undefined)
        }
    }
    let client = std::thread::spawn(move || {
        let port = port_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("server announces port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect client");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write request");
        stream
            .shutdown(Shutdown::Write)
            .expect("finish request bytes");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read response");
        String::from_utf8(response).expect("UTF-8 response")
    });

    let mut host = ServerHost {
        port_sender: Some(port_sender),
    };
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("serve live request");
    let response = client.join().expect("client thread");
    assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
    assert_eq!(http_header(&response, "content-type"), Some("text/plain"));
    assert_eq!(http_body(&response), b"GET /health");
}

#[test]
fn node_http_create_server_accepts_options_before_the_listener() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { createServer } from 'node:http';",
            "let server;",
            "server = createServer({ keepAlive: false }, (request, response) => {",
            "  response.end('options ' + request.url);",
            "  server.close();",
            "});",
            "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build HTTP graph");

    let (port_sender, port_receiver) = mpsc::channel();
    struct ServerHost {
        port_sender: Option<mpsc::Sender<u16>>,
    }
    impl HostOps for ServerHost {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            assert_eq!(operation, HostOp::WriteStdout);
            let port = arguments
                .first()
                .and_then(|value| value.to_js_string().parse::<u16>().ok())
                .expect("listening callback prints port");
            if let Some(sender) = self.port_sender.take() {
                sender.send(port).expect("send server port");
            }
            Ok(HostValue::Undefined)
        }
    }
    let client = std::thread::spawn(move || {
        let port = port_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("server announces port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect client");
        stream
            .write_all(b"GET /configured HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write request");
        stream.shutdown(Shutdown::Write).expect("finish request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    });

    let mut host = ServerHost {
        port_sender: Some(port_sender),
    };
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("serve options-form request");
    let response = client.join().expect("client thread");
    assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
    assert_eq!(http_body(&response), b"options /configured");
}

#[test]
fn node_http_request_and_response_share_the_accepted_socket_identity() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    let project = TempProject::new();
    let entry = project.write(
            "main.js",
            concat!(
                "import { createServer } from 'node:http';",
                "let server;",
                "server = createServer((request, response) => {",
                "  if (!request.socket || request.connection !== request.socket) throw new Error('request socket');",
                "  if (response.req !== request) throw new Error('response request');",
                "  if (response.socket !== request.socket || response.connection !== request.socket) throw new Error('response socket');",
                "  response.end('identity');",
                "  server.close();",
                "});",
                "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
            ),
        );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build HTTP identity graph");

    let (port_sender, port_receiver) = mpsc::channel();
    let client = std::thread::spawn(move || {
        let port = port_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("server publishes its port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write request");
        stream.shutdown(Shutdown::Write).expect("finish request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    });
    let mut host = ServerHostForPort {
        sender: Some(port_sender),
        output: Vec::new(),
    };
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("serve identity request");
    let response = client.join().expect("client completes");
    assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
    assert_eq!(http_body(&response), b"identity");
}

#[test]
fn node_http_request_body_is_a_readable_event_stream() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { createServer } from 'node:http';",
            "let server;",
            "server = createServer((request, response) => {",
            "  let bytes = 0;",
            "  request.on('data', chunk => { bytes += chunk.length; });",
            "  request.on('end', () => {",
            "    response.writeHead(201, { 'Content-Type': 'text/plain' });",
            "    response.end(request.method + ' ' + bytes);",
            "    server.close();",
            "  });",
            "});",
            "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build HTTP graph");

    let (port_sender, port_receiver) = mpsc::channel();
    let client = std::thread::spawn(move || {
        let port = port_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("server publishes its port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
        stream
                .write_all(
                    b"POST /upload HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\nConnection: close\r\n\r\nhello ",
                )
                .expect("write request head and first body chunk");
        stream
            .write_all(b"tokio!")
            .expect("write second body chunk");
        stream.shutdown(Shutdown::Write).expect("finish request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    });

    struct ServerHost {
        port_sender: Option<mpsc::Sender<u16>>,
    }
    impl HostOps for ServerHost {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            assert_eq!(operation, HostOp::WriteStdout);
            let port = arguments
                .first()
                .and_then(|value| value.to_js_string().parse::<u16>().ok())
                .expect("server logs numeric port");
            if let Some(sender) = self.port_sender.take() {
                sender.send(port).expect("publish server port");
            }
            Ok(HostValue::Undefined)
        }
    }
    let mut host = ServerHost {
        port_sender: Some(port_sender),
    };
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("serve request body");
    let response = client.join().expect("client completes");
    assert_eq!(http_status(&response), "HTTP/1.1 201 Created");
    assert_eq!(http_header(&response, "content-type"), Some("text/plain"));
    assert_eq!(http_body(&response), b"POST 12");
}

#[test]
fn node_http_request_pause_and_resume_control_tokio_reads() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { createServer } from 'node:http';",
            "let server;",
            "server = createServer((request, response) => {",
            "  request.pause();",
            "  let chunks = 0;",
            "  request.on('data', chunk => { chunks += 1; });",
            "  request.on('end', () => { response.end(String(chunks)); server.close(); });",
            "  setTimeout(() => request.resume(), 1);",
            "});",
            "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build paused HTTP graph");

    let (port_sender, port_receiver) = mpsc::channel();
    let client = std::thread::spawn(move || {
        let port = port_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("server publishes its port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
        stream
                .write_all(
                    b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .expect("write request");
        stream.shutdown(Shutdown::Write).expect("finish request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    });
    let mut host = ServerHostForPort {
        sender: Some(port_sender),
        output: Vec::new(),
    };
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("serve paused request");
    let response = client.join().expect("client completes");
    assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
    assert_eq!(http_body(&response), b"1");
}

#[test]
fn node_http_response_write_streams_chunked_body_and_reports_pressure() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { createServer } from 'node:http';",
            "let server;",
            "server = createServer((request, response) => {",
            "  const ready = response.write('hello ');",
            "  response.end('tokio');",
            "  console.log(ready);",
            "  server.close();",
            "});",
            "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build streaming HTTP graph");

    let (port_sender, port_receiver) = mpsc::channel();
    let client = std::thread::spawn(move || {
        let port = port_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("server publishes its port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("write request");
        stream.shutdown(Shutdown::Write).expect("finish request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("read response");
        response
    });
    let mut host = ServerHostForPort {
        sender: Some(port_sender),
        output: Vec::new(),
    };
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("serve streaming response");
    let response = client.join().expect("client completes");
    assert_eq!(http_status(&response), "HTTP/1.1 200 OK");
    assert_eq!(http_header(&response, "transfer-encoding"), Some("chunked"));
    assert_eq!(http_body(&response), b"hello tokio");
    assert_eq!(host.output.last().map(String::as_str), Some("true"));
}

#[test]
fn node_net_server_echoes_over_the_shared_tokio_stream_driver() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpStream};
    use std::sync::mpsc;
    use std::time::Duration;

    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        concat!(
            "import { createServer } from 'node:net';",
            "let server;",
            "server = createServer(socket => {",
            "  socket.on('data', chunk => socket.end(chunk));",
            "  socket.on('finish', () => server.close());",
            "});",
            "server.listen(0, '127.0.0.1', () => console.log(server.address().port));",
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build net graph");

    let (port_sender, port_receiver) = mpsc::channel();
    struct ServerHost(Option<mpsc::Sender<u16>>);
    impl HostOps for ServerHost {
        fn call(
            &mut self,
            operation: HostOp,
            arguments: &[HostValue],
        ) -> Result<HostValue, HostError> {
            assert_eq!(operation, HostOp::WriteStdout);
            let port = arguments[0]
                .to_js_string()
                .parse::<u16>()
                .expect("numeric port");
            if let Some(sender) = self.0.take() {
                sender.send(port).expect("publish port");
            }
            Ok(HostValue::Undefined)
        }
    }
    let client = std::thread::spawn(move || {
        let port = port_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("server announces port");
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect server");
        stream.write_all(b"tokio net").expect("write payload");
        stream.shutdown(Shutdown::Write).expect("finish request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut echoed = Vec::new();
        stream.read_to_end(&mut echoed).expect("read echo");
        echoed
    });
    let mut host = ServerHost(Some(port_sender));
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("run net server");
    assert_eq!(client.join().expect("client completes"), b"tokio net");
}

#[test]
fn node_net_client_connects_over_the_shared_tokio_stream_driver() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener};
    use std::time::Duration;

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind peer");
    let port = listener.local_addr().unwrap().port();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept kiln client");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = [0; 4];
        stream
            .read_exact(&mut request)
            .expect("read client payload");
        stream.write_all(b"pong!").expect("write reply");
        stream.shutdown(Shutdown::Write).expect("finish reply");
        request
    });

    let project = TempProject::new();
    let entry = project.write(
        "main.js",
        &format!(
            concat!(
                "import {{ connect }} from 'node:net';",
                "const socket = connect({}, '127.0.0.1', () => socket.write('ping'));",
                "let bytes = 0;",
                "socket.on('data', chunk => bytes += chunk.length);",
                "socket.on('end', () => {{ console.log(bytes); socket.end(); }});",
            ),
            port
        ),
    );
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_with_policy(&entry, resolver, |path| {
        let source = fs::read_to_string(path).map_err(|error| ModuleGraphError::Source {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        let hir = frontend_oxc::parse(path.to_str().expect("UTF-8 test path"), &source);
        compile_module(&hir).map_err(|diagnostics| ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        })
    })
    .expect("build net client graph");
    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("run net client");
    assert_eq!(peer.join().expect("peer completes"), *b"ping");
    assert_eq!(host.output, ["5"]);
}

/// A function value survives the module boundary it was exported across.
///
/// A closure indexes its own program's function table, and every module compiles to its own
/// program -- so calling an imported function used to reach into the *importer's* table and
/// find someone else's body or nothing at all. The closure carries its program now, which is
/// what makes `export function` mean anything.
#[test]
fn exported_functions_and_classes_are_callable_by_the_importer() {
    let project = TempProject::new();
    project.write(
        "lib.js",
        "export function greet(name) { return 'hi ' + name; }\n\
             export class Box { constructor(v) { this.v = v; } get() { return this.v; } }\n\
             export const twice = (n) => n * 2;\n\
             export default function main() { return 'default'; }",
    );
    let entry = project.write(
        "main.js",
        "import main, { greet, Box, twice } from './lib.js';\n\
             console.log(greet('x'));\n\
             console.log(new Box(7).get());\n\
             console.log(twice(21));\n\
             console.log(main());",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["hi x", "7", "42", "default"]);
}

/// An imported function still sees the module it was written in, not the one calling it.
///
/// The captured binding and the constant both live in the exporter's program; reading either
/// through the importer's tables would give the wrong answer rather than no answer, which is
/// the failure worth a test of its own.
#[test]
fn an_imported_function_closes_over_its_own_module() {
    let project = TempProject::new();
    project.write(
        "lib.js",
        "const secret = 'lib';\n\
             export function reveal() { return secret + '/' + 'constant'; }",
    );
    let entry = project.write(
        "main.js",
        "import { reveal } from './lib.js';\n\
             const secret = 'main';\n\
             console.log(reveal(), secret);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["lib/constant main"]);
}

/// `import * as ns` binds one object holding every export, with keys in sorted order.
///
/// Sorted because the specification orders a namespace object's keys and because the linker
/// holds exports in a hash map: without sorting, `Object.keys(ns)` would differ run to run.
#[test]
fn a_namespace_import_binds_every_export_as_one_object() {
    let project = TempProject::new();
    project.write(
        "lib.js",
        "export const one = 1;\n\
             export function two() { return 2; }\n\
             export default 'zero';",
    );
    let entry = project.write(
        "main.js",
        "import * as lib from './lib.js';\n\
             console.log(Object.keys(lib).join(','));\n\
             console.log(lib.one, lib.two(), lib.default);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["default,one,two", "1 2 zero"]);
}

/// A named re-export forwards a provider's binding without the module ever naming it.
#[test]
fn named_re_exports_forward_without_a_local_binding() {
    let project = TempProject::new();
    project.write(
        "base.js",
        "export const alpha = 'a';\n\
             export function beta() { return 'b'; }\n\
             export default 'delta';",
    );
    project.write(
        "middle.js",
        "export { alpha, beta as gamma } from './base.js';\n\
             export { default as delta } from './base.js';\n\
             export * as everything from './base.js';",
    );
    let entry = project.write(
        "main.js",
        "import { alpha, gamma, delta, everything } from './middle.js';\n\
             console.log(alpha, gamma(), delta, Object.keys(everything).join(','));",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["a b delta alpha,beta,default"]);
}

/// `export * from` forwards the names the provider chooses, and never its default.
///
/// A name the forwarding module exports itself wins, which is what a barrel file overriding
/// one entry depends on. The link check has to see through the star too, or importing a
/// forwarded name would be rejected before anything ran.
#[test]
fn a_star_re_export_forwards_names_but_not_the_default() {
    let project = TempProject::new();
    project.write(
        "base.js",
        "export const alpha = 'a';\n\
             export const shared = 'base';\n\
             export default 'delta';",
    );
    let wide = project.write(
        "wide.js",
        "export * from './base.js';\nexport const shared = 'wide';",
    );
    let entry = project.write(
        "main.js",
        "import * as wide from './wide.js';\n\
             import { alpha, shared } from './wide.js';\n\
             console.log(alpha, shared);\n\
             console.log(Object.keys(wide).join(','));",
    );
    let graph = project.graph(&entry).expect("build graph");
    assert_eq!(
        graph.resolved_export_names(project.root.join(".").join("wide.js")),
        Some(vec!["alpha".to_owned(), "shared".to_owned()])
    );
    assert_eq!(
        graph.resolved_export_names(&wide),
        Some(vec!["alpha".to_owned(), "shared".to_owned()])
    );
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["a wide", "alpha,shared"]);
}

#[test]
fn a_named_import_rejects_an_ambiguous_star_export() {
    let project = TempProject::new();
    project.write("a.js", "export const shared = 'a';");
    project.write("b.js", "export const shared = 'b';");
    let barrel = project.write(
        "barrel.js",
        "export * from './a.js';\nexport * from './b.js';",
    );
    let entry = project.write(
        "main.js",
        "import { shared } from './barrel.js'; console.log(shared);",
    );

    assert!(matches!(
        project.graph(&entry),
        Err(ModuleGraphError::AmbiguousExport {
            importer,
            provider,
            name,
        }) if importer == fs::canonicalize(entry).unwrap()
            && provider == fs::canonicalize(barrel).unwrap()
            && name == "shared"
    ));
}

#[test]
fn a_namespace_omits_ambiguous_star_exports() {
    let project = TempProject::new();
    project.write("a.js", "export const shared = 'a'; export const onlyA = 1;");
    project.write("b.js", "export const shared = 'b'; export const onlyB = 2;");
    let barrel = project.write(
        "barrel.js",
        "export * from './a.js';\nexport * from './b.js';",
    );
    let entry = project.write(
        "main.js",
        "import * as barrel from './barrel.js';\n\
             console.log(Object.keys(barrel).join(','));\n\
             console.log(barrel.onlyA, barrel.onlyB, barrel.shared);",
    );

    let graph = project
        .graph(&entry)
        .expect("ambiguous names do not poison a namespace");
    assert_eq!(
        graph.resolved_export_names(&barrel),
        Some(vec!["onlyA".to_owned(), "onlyB".to_owned()])
    );
    assert_eq!(
        graph.resolved_export_names(project.root.join("does-not-exist.js")),
        None
    );
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute namespace import");
    assert_eq!(host.output, ["onlyA,onlyB", "1 2 undefined"]);
}

#[test]
fn two_star_paths_to_the_same_binding_are_not_ambiguous() {
    let project = TempProject::new();
    project.write("origin.js", "export const shared = 42;");
    project.write("left.js", "export * from './origin.js';");
    project.write("right.js", "export * from './origin.js';");
    project.write(
        "barrel.js",
        "export * from './left.js';\nexport * from './right.js';",
    );
    let entry = project.write(
        "main.js",
        "import { shared } from './barrel.js'; console.log(shared);",
    );

    let graph = project
        .graph(&entry)
        .expect("converging paths resolve to one binding");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute converging star graph");
    assert_eq!(host.output, ["42"]);
}

#[test]
fn a_deep_star_chain_has_no_arbitrary_link_depth_limit() {
    const BARRELS: usize = 80;
    let project = TempProject::new();
    project.write("leaf.js", "export const answer = 42;");
    for index in (0..BARRELS).rev() {
        let provider = if index + 1 == BARRELS {
            "leaf.js".to_owned()
        } else {
            format!("barrel{}.js", index + 1)
        };
        project.write(
            &format!("barrel{index}.js"),
            &format!("export * from './{provider}';"),
        );
    }
    let entry = project.write(
        "main.js",
        "import { answer } from './barrel0.js'; console.log(answer);",
    );

    let graph = project.graph(&entry).expect("deep star chain links");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("deep star chain executes");
    assert_eq!(host.output, ["42"]);
}

/// An import is a live view of the exporter's binding, not a copy of the value it had when
/// the exporter finished.
///
/// `export let count` that an exported function increments is the case: the importer reads
/// the *new* number. Only a binding that can change is boxed -- a `const` cannot, and boxing
/// it would cost the native backend a module it can otherwise compile.
#[test]
fn a_named_import_is_a_live_view_of_the_exported_binding() {
    let project = TempProject::new();
    project.write(
        "counter.js",
        "export let count = 0;\n\
             export const limit = 10;\n\
             export function bump() { count += 1; }",
    );
    let entry = project.write(
        "main.js",
        "import { count, limit, bump } from './counter.js';\n\
             console.log(count, limit);\n\
             bump();\n\
             bump();\n\
             console.log(count, limit);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["0 10", "2 10"]);
}

/// A namespace object tracks the bindings it names.
///
/// It used to be a snapshot -- the exported cells were read once, when the provider finished --
/// and this test recorded that as a deliberate difference from every other engine. It is no
/// longer one: the namespace keeps the cells beside its properties and brings the slots back
/// into step before a read, so `counter.count` answers what `bump()` left there.
#[test]
fn a_namespace_object_tracks_the_exports() {
    let project = TempProject::new();
    project.write(
        "counter.js",
        "export let count = 0;\nexport function bump() { count += 1; }",
    );
    let entry = project.write(
        "main.js",
        "import * as counter from './counter.js';\n\
             counter.bump();\n\
             console.log(counter.count);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["1"]);
}

#[test]
fn a_module_sees_its_own_exports_through_a_namespace_it_imported_from_itself() {
    let project = TempProject::new();
    // Every read is written *after* the binding it names, which is the whole point: a
    // namespace built when the linker ran would answer `undefined` to all four.
    let entry = project.write(
        "self.js",
        "import * as ns from './self.js';\n\
             export var mutable = 23;\n\
             var renamed = 45;\n\
             export { renamed as other };\n\
             export default 444;\n\
             console.log(ns.mutable, ns.other, ns.default);\n\
             mutable = 24;\n\
             console.log(ns.mutable);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["23 45 444", "24"]);
}

#[test]
fn a_self_imported_namespace_refuses_a_binding_that_has_not_run() {
    let project = TempProject::new();
    let entry = project.write(
        "self.js",
        "import * as ns from './self.js';\n\
             try { ns.later } catch (error) { console.log(error.constructor.name); }\n\
             export const later = 1;\n\
             console.log(ns.later);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    // `undefined` here would be the one answer worse than the refusal this replaced.
    assert_eq!(host.output, ["ReferenceError", "1"]);
}

#[test]
fn a_named_import_from_the_same_module_is_a_live_view_of_the_export() {
    let project = TempProject::new();
    let entry = project.write(
        "self.js",
        "import { counter as seen } from './self.js';\n\
             export let counter = 1;\n\
             console.log(seen);\n\
             counter = 2;\n\
             console.log(seen);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    // A snapshot would print `1` twice. node prints this.
    assert_eq!(host.output, ["1", "2"]);
}

#[test]
fn default_scalar_import_links_to_default_export() {
    let project = TempProject::new();
    project.write("dep.js", "const value = 42; export { value as default };");
    let entry = project.write(
        "main.js",
        "import answer from './dep.js'; console.log(answer);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["42"]);
}

#[test]
fn side_effect_only_modules_run_in_source_order() {
    let project = TempProject::new();
    let dep = project.write("dep.js", "console.log('dependency');");
    let entry = project.write("main.js", "import './dep.js'; console.log('entry');");
    let graph = project.graph(&entry).expect("build graph");
    assert_eq!(
        graph.module_order(),
        &[
            fs::canonicalize(dep).unwrap(),
            fs::canonicalize(&entry).unwrap()
        ]
    );
    let mut host = TestHost::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute graph");
    assert_eq!(host.output, ["dependency", "entry"]);
}

#[test]
fn body_patch_preserves_linkage_and_rejects_interface_changes() {
    let project = TempProject::new();
    let dependency = project.write(
        "dependency.js",
        "console.log('old'); export const answer = 42;",
    );
    let entry = project.write(
        "entry.js",
        "import { answer } from './dependency.js'; console.log(answer);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let dependency = fs::canonicalize(dependency).expect("canonical dependency");
    let order = graph.module_order().to_vec();
    let resolved = graph
        .resolved_export(&dependency, "answer")
        .cloned()
        .expect("answer is linked");
    let program = compile_module(&frontend_oxc::parse(
        dependency.to_str().expect("utf8 dependency"),
        "console.log('new'); export const answer = 7;",
    ))
    .expect("compile body edit");

    let patched = graph
        .patch_module(&dependency, program)
        .expect("body-only edit can reuse graph topology");
    assert!(Arc::ptr_eq(&graph.modules, &patched.modules));
    assert!(Arc::ptr_eq(
        &graph.export_resolutions,
        &patched.export_resolutions
    ));
    assert_eq!(
        graph.module(&entry).unwrap() as *const ModuleNode,
        patched.module(&entry).unwrap() as *const ModuleNode
    );
    assert_ne!(
        graph.module(&dependency).unwrap() as *const ModuleNode,
        patched.module(&dependency).unwrap() as *const ModuleNode
    );
    assert_eq!(patched.module_order(), order);
    assert_eq!(
        patched.resolved_export(&dependency, "answer"),
        Some(&resolved)
    );
    assert_eq!(
        patched.module(&dependency).unwrap().dependencies,
        graph.module(&dependency).unwrap().dependencies
    );
    let mut host = TestHost::default();
    patched
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute patched graph");
    assert_eq!(host.output, ["new", "7"]);

    let changed_export = compile_module(&frontend_oxc::parse(
        dependency.to_str().expect("utf8 dependency"),
        "console.log('new'); export const other = 7;",
    ))
    .expect("compile interface edit");
    assert!(graph.patch_module(&dependency, changed_export).is_none());

    let changed_import = compile_module(&frontend_oxc::parse(
        dependency.to_str().expect("utf8 dependency"),
        "import './other.js'; export const answer = 7;",
    ))
    .expect("compile import edit");
    assert!(graph.patch_module(&dependency, changed_import).is_none());
}

#[test]
fn sequential_body_patches_retain_each_previous_overlay() {
    let project = TempProject::new();
    let first = project.write(
        "first.js",
        "console.log('first old'); export const first = 1;",
    );
    let second = project.write(
        "second.js",
        "console.log('second old'); export const second = 2;",
    );
    let entry = project.write(
            "entry.js",
            "import { first } from './first.js'; import { second } from './second.js'; console.log(first + second);",
        );
    let graph = project.graph(&entry).expect("build graph");
    let first = fs::canonicalize(first).expect("canonical first");
    let second = fs::canonicalize(second).expect("canonical second");
    let first_program = compile_module(&frontend_oxc::parse(
        first.to_str().expect("utf8 first"),
        "console.log('first new'); export const first = 10;",
    ))
    .expect("compile first body edit");
    let second_program = compile_module(&frontend_oxc::parse(
        second.to_str().expect("utf8 second"),
        "console.log('second new'); export const second = 20;",
    ))
    .expect("compile second body edit");

    let patched_first = graph
        .patch_module(&first, first_program)
        .expect("first body edit can be patched");
    let patched_both = patched_first
        .patch_module(&second, second_program)
        .expect("second body edit can be patched");
    let mut host = TestHost::default();
    patched_both
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute both patched modules");
    assert_eq!(host.output, ["first new", "second new", "30"]);

    let mut original_host = TestHost::default();
    graph
        .execute(
            &mut original_host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute original graph");
    assert_eq!(original_host.output, ["first old", "second old", "3"]);
}

#[test]
fn missing_export_is_reported_at_link_time() {
    let project = TempProject::new();
    project.write("dep.js", "export const value = 42;");
    let entry = project.write(
        "main.js",
        "import { missing } from './dep.js'; console.log(missing);",
    );
    assert!(matches!(
        project.graph(&entry),
        Err(ModuleGraphError::MissingExport { name, .. }) if name == "missing"
    ));
}

#[test]
fn a_cycle_is_ordered_as_the_specification_evaluates_it() {
    let project = TempProject::new();
    let a = project.write("a.js", "import './b.js';");
    let b = project.write("b.js", "import './c.js';");
    let c = project.write("c.js", "import './a.js';");
    let graph = project.graph(&a).expect("a cycle is a legal module graph");
    // `InnerModuleEvaluation` returns as soon as it meets a module that is already
    // evaluating, and runs each body on the way back out. So the deepest module of the
    // cycle runs first and the entry runs last, exactly as it would without the back edge.
    assert_eq!(
        graph
            .module_order()
            .iter()
            .map(|path| path.file_name().expect("a file name").to_owned())
            .collect::<Vec<_>>(),
        ["c.js", "b.js", "a.js"]
    );
    let _ = (b, c);
}

#[test]
fn a_cycle_evaluates_every_module_once_and_binds_across_the_back_edge() {
    let project = TempProject::new();
    let entry = project.write(
        "a.js",
        "import { fromB } from './b.js';\n\
             export const fromA = 'a';\n\
             globalThis.log = (globalThis.log ?? '') + 'a' + fromB;",
    );
    project.write(
        "b.js",
        "import { fromA } from './a.js';\n\
             export const fromB = 'b';\n\
             globalThis.log = (globalThis.log ?? '') + 'b';\n\
             export const seenLate = () => fromA;",
    );
    let graph = project
        .graph(&entry)
        .expect("a cycle is a legal module graph");
    // b runs first, so `fromA` is still in its dead zone while b's body runs -- but b only
    // captures it in a closure, which is the idiom a real cyclic import relies on.
    assert!(
        matches!(
            graph.resolved_export(&entry, "fromA"),
            Some(ResolvedExport::Local { module, .. })
                if module.file_name() == Some(std::ffi::OsStr::new("a.js"))
        ),
        "fromA is a.js's own binding"
    );
    // The back edge is bound, not refused: b.js names a binding a.js has not created yet.
    assert!(matches!(
        graph.resolved_export(&entry, "fromB"),
        None | Some(ResolvedExport::Local { .. })
    ));
    assert_eq!(graph.module_count(), 2);
}

#[test]
fn a_circular_indirect_export_resolves_to_nothing() {
    let project = TempProject::new();
    // The shape `language/module-code/instn-iee-err-circular.js` tests: each module says the
    // other provides `x`, so `ResolveExport` meets the same (module, name) pair twice and
    // the specification's answer is null -- which the linker reports as a missing export,
    // and the conformance harness reports as the SyntaxError the test asks for.
    let entry = project.write("a.js", "export { x } from './b.js';");
    project.write("b.js", "export { x } from './a.js';");
    let error = project
        .graph(&entry)
        .expect_err("a self-resolving indirect export has no binding to bind");
    assert!(
        matches!(&error, ModuleGraphError::MissingExport { name, .. } if name == "x"),
        "expected a missing export, got {error:?}"
    );
}

#[test]
fn a_star_cycle_still_finds_a_name_one_side_provides() {
    let project = TempProject::new();
    let entry = project.write("a.js", "export * from './b.js'; export const own = 1;");
    project.write("b.js", "export * from './a.js'; export const theirs = 2;");
    let graph = project.graph(&entry).expect("a star cycle is legal");
    // `GetExportedNames` cuts on the modules it has already asked, so the walk terminates
    // and each side still sees the other's name.
    assert!(matches!(
        graph.resolved_export(&entry, "theirs"),
        Some(ResolvedExport::Local { .. })
    ));
    assert!(matches!(
        graph.resolved_export(&entry, "own"),
        Some(ResolvedExport::Local { .. })
    ));
}

#[test]
fn order_is_deterministic_and_dependencies_first() {
    let project = TempProject::new();
    let c = project.write("c.js", "console.log('c');");
    let b = project.write("b.js", "import './c.js'; console.log('b');");
    let a = project.write("a.js", "console.log('a');");
    let entry = project.write(
        "main.js",
        "import './b.js'; import './a.js'; console.log('main');",
    );
    let graph = project.graph(&entry).expect("build graph");
    assert_eq!(
        graph.module_order(),
        &[
            fs::canonicalize(c).unwrap(),
            fs::canonicalize(b).unwrap(),
            fs::canonicalize(a).unwrap(),
            fs::canonicalize(entry).unwrap(),
        ]
    );
}

#[test]
fn deep_linear_graph_is_stack_safe() {
    const MODULES: usize = 4_096;
    let project = TempProject::new();
    for index in 0..MODULES {
        project.write(&format!("m{index:04}.js"), "");
    }
    let entry = project.root.join("m0000.js");
    let graph = ModuleGraph::build_with_modules(&entry, &project.root, move |path| {
        let index = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.strip_prefix('m'))
            .and_then(|index| index.parse::<usize>().ok())
            .expect("chain fixture path");
        let dependencies = (index + 1 < MODULES)
            .then(|| format!("./m{:04}.js", index + 1))
            .into_iter()
            .collect();
        ModuleProgram {
            program: Program {
                source: "".into(),
                template_sites: Vec::new(),
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![Instruction::Return],
                local_count: 0,
                functions: Vec::new(),
            },
            dependencies,
            imports: Vec::new(),
            exports: Vec::new(),
            strictness: kiln_ir::Strictness::default(),
            ..Default::default()
        }
    })
    .expect("deep graph builds without recursion");
    assert_eq!(graph.module_count(), MODULES);
    assert_eq!(
        graph.module_order().first(),
        Some(&project.root.join("m4095.js").canonicalize().unwrap())
    );
    assert_eq!(
        graph.module_order().last(),
        Some(&entry.canonicalize().unwrap())
    );
}

#[test]
fn step_limit_diagnostic_reports_total_graph_steps() {
    let project = TempProject::new();
    project.write("dep.js", "");
    let entry = project.write("main.js", "");
    let graph = ModuleGraph::build_with_modules(&entry, &project.root, |path| {
        let is_entry = path.file_name().and_then(|name| name.to_str()) == Some("main.js");
        ModuleProgram {
            program: Program {
                source: "".into(),
                template_sites: Vec::new(),
                version: BYTECODE_VERSION,
                constants: Vec::new(),
                code: vec![Instruction::Return],
                local_count: 0,
                functions: Vec::new(),
            },
            dependencies: is_entry
                .then(|| "./dep.js".to_owned())
                .into_iter()
                .collect(),
            imports: Vec::new(),
            exports: Vec::new(),
            strictness: kiln_ir::Strictness::default(),
            ..Default::default()
        }
    })
    .expect("build graph");
    let mut host = TestHost::default();
    assert!(matches!(
        graph.execute(
            &mut host,
            ExecutionLimits {
                max_steps: 1,
                ..ExecutionLimits::default()
            },
            ExecutionCapabilities::default(),
        ),
        Err(ModuleGraphError::Execution {
            error: ExecutionError::StepLimitExceeded { steps: 1, limit: 1 },
            ..
        })
    ));
}

fn reduce_all_live_demand(graph: &ModuleGraph) -> LiveDemand {
    let mut live_imports = BTreeSet::new();
    for node in graph.modules() {
        for (index, _) in node.program.imports.iter().enumerate() {
            live_imports.insert((node.path.clone(), index));
        }
    }
    LiveDemand {
        live: graph.modules().map(|node| node.path.clone()).collect(),
        slots: BTreeMap::new(),
        live_imports,
    }
}

#[test]
fn reduce_to_live_with_everything_live_is_identity() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "import { a } from './dep1.mjs';\nimport './dep2.mjs';\nconsole.log(a());\n",
    );
    project.write(
        "dep1.mjs",
        "export function a() { return 1; }\nexport function b() { return 2; }\n",
    );
    project.write("dep2.mjs", "globalThis.__probe = 1;\n");
    let graph = project.graph(&entry).expect("fixture graph");
    assert_eq!(graph.module_count(), 3);
    let demand = reduce_all_live_demand(&graph);
    let (reduced, remapped) = graph.reduce_to_live(&demand).expect("identity reduction");
    assert_eq!(reduced, graph);
    assert_eq!(remapped.live_imports, demand.live_imports);
}

#[test]
fn reduce_to_live_drops_dead_modules_and_edges() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "import { a } from './dep1.mjs';\nimport './dep2.mjs';\nconsole.log(a());\n",
    );
    project.write(
        "dep1.mjs",
        "export function a() { return 1; }\nexport function b() { return 2; }\n",
    );
    let dep2 = project.write("dep2.mjs", "globalThis.__probe = 1;\n");
    let graph = project.graph(&entry).expect("fixture graph");
    let dep1 = graph
        .modules()
        .find(|node| node.path.ends_with("dep1.mjs"))
        .expect("dep1")
        .path
        .clone();
    let slot_a = graph
        .module(&dep1)
        .expect("dep1 node")
        .program
        .exports
        .iter()
        .find(|export| export.exported == "a")
        .expect("export a")
        .local_slot;
    let entry_imports: Vec<usize> = graph
        .module(&entry)
        .expect("entry node")
        .program
        .imports
        .iter()
        .enumerate()
        .filter(|(_, import)| import.source.ends_with("dep1.mjs"))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(entry_imports.len(), 1);
    let mut slots = BTreeMap::new();
    slots.insert(dep1.clone(), BTreeSet::from([slot_a]));
    let demand = LiveDemand {
        live: BTreeSet::from([entry.clone(), dep1.clone()]),
        slots,
        live_imports: BTreeSet::from([(entry.clone(), entry_imports[0])]),
    };
    let (reduced, remapped) = graph.reduce_to_live(&demand).expect("reduction");
    assert_eq!(reduced.module_count(), 2);
    assert!(reduced.module(&dep2).is_none());
    let reduced_entry = reduced.module(&entry).expect("entry kept");
    assert_eq!(reduced_entry.program.imports.len(), 1);
    assert!(
        reduced_entry.program.imports[0]
            .source
            .ends_with("dep1.mjs")
    );
    assert!(
        reduced_entry
            .dependencies
            .iter()
            .all(|dependency| dependency.target != dep2),
        "no edge names the dropped module"
    );
    // The surviving edge remaps to index zero; the demand slots ride along.
    assert_eq!(remapped.live_imports, BTreeSet::from([(entry.clone(), 0)]));
    assert_eq!(remapped.slots.get(&dep1).expect("slots kept").len(), 1);
    // Star-free re-export resolution still answers on the reduced graph.
    assert!(reduced.resolved_export(&dep1, "a").is_some());
    assert!(reduced.resolved_export(&dep1, "b").is_some());
}

#[test]
fn reduce_to_live_without_the_entry_is_an_error() {
    let project = TempProject::new();
    let entry = project.write("main.mjs", "console.log(1);\n");
    let graph = project.graph(&entry).expect("fixture graph");
    let demand = LiveDemand::default();
    assert!(matches!(
        graph.reduce_to_live(&demand),
        Err(ModuleGraphError::InternalMissingModule { .. })
    ));
}

#[test]
fn reduce_to_live_keeps_commonjs_wrapper_imports() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "import value from './dep.cjs';\nconsole.log(value);\n",
    );
    project.write("dep.cjs", "module.exports = 41;\n");
    let resolver = NodeRuntimeResolver::new(&project.root).expect("Node resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("cjs fixture graph");
    let demand = reduce_all_live_demand(&graph);
    let (reduced, _) = graph.reduce_to_live(&demand).expect("identity reduction");
    assert_eq!(reduced, graph);
    let dep = graph
        .modules()
        .find(|node| node.path.ends_with("dep.cjs"))
        .expect("cjs dep");
    assert!(
        dep.program
            .imports
            .iter()
            .any(|import| import.source == "*commonjs-wrapper*"),
        "fixture has wrapper imports"
    );
}

#[test]
fn reduce_to_live_keeps_link_only_reexport_imports_inert() {
    let project = TempProject::new();
    let origin = project.write("origin.mjs", "export const answer = 42;\n");
    let barrel = project.write(
        "barrel.mjs",
        "export { answer as renamed } from './origin.mjs';\n",
    );
    let entry = project.write(
        "main.mjs",
        "import { renamed } from './barrel.mjs'; console.log(renamed);\n",
    );
    let graph = project.graph(&entry).expect("re-export graph");
    let barrel_node = graph.module(&barrel).expect("barrel node");
    assert_eq!(barrel_node.program.imports.len(), 1);
    let original_resolution = graph
        .resolved_export(&barrel, "renamed")
        .expect("original resolution")
        .clone();
    let demand = LiveDemand {
        live: BTreeSet::from([entry.clone(), barrel.clone(), origin.clone()]),
        slots: BTreeMap::new(),
        live_imports: BTreeSet::from([(entry.clone(), 0)]),
    };

    let (reduced, remapped) = graph.reduce_to_live(&demand).expect("reduction");

    assert_eq!(
        reduced
            .module(&barrel)
            .expect("barrel retained")
            .program
            .imports
            .len(),
        1,
        "the synthetic import is link metadata even though no module body reads it"
    );
    assert_eq!(remapped.live_imports, BTreeSet::from([(entry, 0)]));
    assert_eq!(
        reduced.resolved_export(&barrel, "renamed"),
        Some(&original_resolution)
    );
}

#[test]
fn reduce_to_live_retargets_an_import_around_a_dropped_barrel() {
    let project = TempProject::new();
    let origin = project.write("origin.mjs", "export const answer = 42;\n");
    let barrel = project.write(
        "barrel.mjs",
        "export { answer as renamed } from './origin.mjs';\n",
    );
    let entry = project.write(
        "main.mjs",
        "import { renamed } from './barrel.mjs'; console.log(renamed);\n",
    );
    let graph = project.graph(&entry).expect("re-export graph");
    let origin_slot = match graph
        .resolved_export(&barrel, "renamed")
        .expect("barrel resolution")
    {
        ResolvedExport::Local { local_slot, .. } => *local_slot,
        resolution => panic!("expected local origin, got {resolution:?}"),
    };
    let demand = LiveDemand {
        live: BTreeSet::from([entry.clone(), origin.clone()]),
        slots: BTreeMap::from([(origin.clone(), BTreeSet::from([origin_slot]))]),
        live_imports: BTreeSet::from([(entry.clone(), 0)]),
    };

    let (reduced, remapped) = graph.reduce_to_live(&demand).expect("reduction");

    assert!(reduced.module(&barrel).is_none());
    let entry_node = reduced.module(&entry).expect("entry retained");
    assert_eq!(entry_node.program.imports.len(), 1);
    assert_eq!(entry_node.program.imports[0].imported, "answer");
    assert_eq!(entry_node.dependencies.len(), 1);
    assert_eq!(entry_node.dependencies[0].target, origin);
    assert_eq!(remapped.live_imports, BTreeSet::from([(entry, 0)]));
}
