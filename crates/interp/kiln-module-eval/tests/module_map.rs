//! One module map per realm: a module is evaluated once however it is reached -- by the static
//! graph, by `import()`, by `require` -- and every importer sees the same bindings and the same
//! namespace object.

use kiln_host_abi::{HostError, HostOp, HostOps, HostValue};
use kiln_interpreter::{ExecutionCapabilities, ExecutionError, ExecutionLimits};
use kiln_module_eval::{GraphModuleLoader, ModuleEvaluationError, ModuleGraphExecution};
use kiln_module_graph::{CommonJsParse, Frontend, ModuleGraph};
use kiln_module_resolver::NodeRuntimeResolver;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project {
    root: PathBuf,
}

impl Project {
    fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("kiln-module-map-{}-{id}", std::process::id()));
        fs::create_dir_all(&root).expect("create project");
        Self {
            root: fs::canonicalize(root).expect("canonical root"),
        }
    }

    fn write(&self, name: &str, source: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, source).expect("write source");
        path
    }

    /// Run `main.mjs` as `kiln --node run` does: the Node graph, with `import()` linked
    /// against the same graph. The program's standard output, line by line, or its failure.
    fn run(&self, main: &str) -> (Vec<String>, Result<(), ModuleEvaluationError>) {
        struct Oxc;
        impl Frontend for Oxc {
            fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
                kiln_frontend_oxc::parse_with_goal(path, source, kiln_hir::SourceGoal::Module)
            }
            fn parse_commonjs(&mut self, path: &str, source: &str) -> CommonJsParse {
                let (module, static_requires) =
                    kiln_frontend_oxc::parse_commonjs_with_requires(path, source);
                CommonJsParse {
                    module,
                    static_requires,
                }
            }
        }
        let entry = self.write("main.mjs", main);
        let graph = Arc::new(
            ModuleGraph::build_node(
                &entry,
                NodeRuntimeResolver::new(&self.root).expect("resolver"),
                |path: &Path| fs::read_to_string(path),
                Oxc,
            )
            .expect("build graph"),
        );
        let mut host = Output::default();
        let outcome = graph
            .execute_node(
                &mut host,
                ExecutionLimits::default(),
                ExecutionCapabilities::default().with_module_loader_service(Arc::new(
                    GraphModuleLoader::new(Arc::clone(&graph)),
                )),
            )
            .map(|_| ());
        (host.lines, outcome)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct Output {
    lines: Vec<String>,
}

impl HostOps for Output {
    fn call(&mut self, operation: HostOp, arguments: &[HostValue]) -> Result<HostValue, HostError> {
        if operation == HostOp::ClockNow {
            return Ok(HostValue::number(0.0));
        }
        if operation == HostOp::WriteStdout {
            self.lines.push(
                arguments
                    .iter()
                    .map(HostValue::to_js_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        Ok(HostValue::Undefined)
    }
}

fn expect_lines(project: &Project, main: &str, expected: &[&str]) {
    let (lines, outcome) = project.run(main);
    outcome.unwrap_or_else(|error| panic!("program failed: {error}; printed {lines:?}"));
    assert_eq!(lines, expected);
}

#[test]
fn static_then_dynamic_links_to_the_static_record() {
    let project = Project::new();
    project.write(
        "m.mjs",
        "globalThis.runs = (globalThis.runs || 0) + 1;\n\
         export class K {}\n\
         export let n = 1;\n\
         export function bump() { n++; }",
    );
    expect_lines(
        &project,
        "import { K, bump } from './m.mjs';\n\
         import * as ns from './m.mjs';\n\
         const d = await import('./m.mjs');\n\
         const again = await import('./m.mjs');\n\
         bump();\n\
         console.log(K === d.K, ns === d, d === again, d.n, globalThis.runs);",
        &["true true true 2 1"],
    );
}

#[test]
fn dynamic_then_static_links_to_the_dynamic_record() {
    let project = Project::new();
    project.write("a.mjs", "export const early = await import('./b.mjs');");
    project.write(
        "b.mjs",
        "globalThis.bRuns = (globalThis.bRuns || 0) + 1;\nexport const v = {};",
    );
    expect_lines(
        &project,
        "import { early } from './a.mjs';\n\
         import { v } from './b.mjs';\n\
         import * as b from './b.mjs';\n\
         console.log(early.v === v, early === b, globalThis.bRuns);",
        &["true true 1"],
    );
}

#[test]
fn two_dynamic_importers_share_one_instance() {
    let project = Project::new();
    project.write("x.mjs", "export const get = () => import('./shared.mjs');");
    project.write("y.mjs", "export const get = () => import('./shared.mjs');");
    project.write(
        "shared.mjs",
        "globalThis.sRuns = (globalThis.sRuns || 0) + 1;\nexport const o = {};",
    );
    expect_lines(
        &project,
        "import { get as gx } from './x.mjs';\n\
         import { get as gy } from './y.mjs';\n\
         const [p, q] = await Promise.all([gx(), gy()]);\n\
         console.log(p === q, p.o === q.o, globalThis.sRuns);",
        &["true true 1"],
    );
}

#[test]
fn computed_file_url_loads_a_new_graph_and_reuses_static_dependencies() {
    let project = Project::new();
    project.write(
        "shared.mjs",
        "globalThis.sharedRuns = (globalThis.sharedRuns || 0) + 1; export const shared = {};",
    );
    project.write(
        "late module.mjs",
        "import { shared } from './shared.mjs';\n\
         globalThis.lateRuns = (globalThis.lateRuns || 0) + 1;\n\
         export { shared }; export const marker = 'loaded';",
    );
    let directory_url = url::Url::from_directory_path(&project.root).expect("directory URL");
    expect_lines(
        &project,
        &format!(
            "import {{ shared }} from './shared.mjs';\n\
             const target = [{directory_url:?}, 'late%20module.mjs'].join('');\n\
             const first = await import(target);\n\
             const second = await import(target);\n\
             console.log(first.marker, first === second, first.shared === shared,\n\
                         globalThis.sharedRuns, globalThis.lateRuns);",
            directory_url = directory_url.as_str(),
        ),
        &["loaded true true 1 1"],
    );
}

#[test]
fn computed_file_url_rejects_a_missing_file() {
    let project = Project::new();
    let directory_url = url::Url::from_directory_path(&project.root).expect("directory URL");
    expect_lines(
        &project,
        &format!(
            "const target = [{directory_url:?}, 'missing.mjs'].join('');\n\
             try {{ await import(target); console.log('unexpected success'); }}\n\
             catch (error) {{ console.log(error instanceof TypeError); }}",
            directory_url = directory_url.as_str(),
        ),
        &["true"],
    );
}

#[test]
fn builtin_modules_are_one_instance_per_realm() {
    let project = Project::new();
    expect_lines(
        &project,
        "import { EventEmitter } from 'node:events';\n\
         import * as events from 'node:events';\n\
         const ev = await import('node:events');\n\
         console.log(EventEmitter === ev.EventEmitter, events === ev);",
        &["true true"],
    );
}

#[test]
fn a_failed_module_rethrows_the_same_error_and_runs_once() {
    let project = Project::new();
    project.write(
        "bad.mjs",
        "console.log('bad ran');\nthrow new Error('boom');",
    );
    project.write(
        "a.mjs",
        "let first;\n\
         try { await import('./bad.mjs'); } catch (error) { first = error; }\n\
         let second;\n\
         try { await import('./bad.mjs'); } catch (error) { second = error; }\n\
         console.log(first === second, first.message);",
    );
    // `bad.mjs` is also a static dependency of the entry, reached after `a.mjs` caught its
    // failure: the static graph rethrows the same error rather than running it again.
    let (lines, outcome) =
        project.run("import './a.mjs';\nimport './bad.mjs';\nconsole.log('unreachable');");
    assert_eq!(lines, ["bad ran", "true boom"]);
    match outcome {
        Err(ModuleEvaluationError::Execution {
            module,
            error: ExecutionError::Uncaught { message, .. },
        }) => {
            assert!(module.ends_with("bad.mjs"), "{}", module.display());
            assert!(message.contains("boom"), "{message}");
        }
        other => panic!("expected bad.mjs's error, got {other:?}"),
    }
}

#[test]
fn cycles_link_through_the_map_statically_and_dynamically() {
    let project = Project::new();
    project.write(
        "c1.mjs",
        "import { two } from './c2.mjs';\n\
         globalThis.c1Runs = (globalThis.c1Runs || 0) + 1;\n\
         export function one() { return 'one'; }\n\
         export const seen = () => two();",
    );
    project.write(
        "c2.mjs",
        "import { one } from './c1.mjs';\n\
         export function two() { return one(); }",
    );
    // Reached statically first.
    expect_lines(
        &project,
        "import { seen } from './c1.mjs';\n\
         const d1 = await import('./c1.mjs');\n\
         const d2 = await import('./c2.mjs');\n\
         console.log(seen(), d1.seen === seen, d2.two(), globalThis.c1Runs);",
        &["one true one 1"],
    );
    // Reached only dynamically: the plan links the cycle's back edge to a dead-zone cell that
    // `c1.mjs` fills when it publishes.
    let other = Project::new();
    other.write(
        "c1.mjs",
        "import { two } from './c2.mjs';\n\
         export function one() { return 'one'; }\n\
         export const seen = () => two();",
    );
    other.write(
        "c2.mjs",
        "import { one } from './c1.mjs';\n\
         export function two() { return one(); }",
    );
    expect_lines(
        &other,
        "const m = await import('./c1.mjs');\n\
         const n = await import('./c2.mjs');\n\
         console.log(m.seen(), n.two === n.two, m === await import('./c1.mjs'));",
        &["one true true"],
    );
}

#[test]
fn import_and_require_of_commonjs_share_module_exports() {
    let project = Project::new();
    project.write(
        "c.cjs",
        "globalThis.cRuns = (globalThis.cRuns || 0) + 1;\nmodule.exports = { a: 1 };",
    );
    expect_lines(
        &project,
        "import { createRequire } from 'node:module';\n\
         const require = createRequire(import.meta.url);\n\
         const r = require('./c.cjs');\n\
         const d = await import('./c.cjs');\n\
         console.log(d.default === r, d.a, globalThis.cRuns);",
        &["true 1 1"],
    );
}

#[test]
fn reset_keeps_commonjs_value_but_scopes_dynamic_import_namespace_per_generation() {
    let project = Project::new();
    project.write(
        "c.cjs",
        "globalThis.cRuns = (globalThis.cRuns || 0) + 1; globalThis.cReads = 0; module.exports = { a: 1 }; Object.defineProperty(module.exports, 'value', { enumerable: true, get() { globalThis.cReads++; return 7; } });",
    );
    expect_lines(
        &project,
        "import { createRequire } from 'node:module';\n\
         const loader = process.__kilnNativeModules.__kilnHostModuleLoader;\n\
         if (!loader('registerRunner', import.meta.url)) throw new Error('runner registration failed');\n\
         const require = createRequire(import.meta.url);\
         const required = require('./c.cjs');\
         const oldImport = () => import('./c.cjs');\
         const old = await oldImport();\
         loader('reset');\
         await (async () => { const nested = async () => { await Promise.resolve(); }; await nested(); })();\
         const actual = await loader('importActual', './c.cjs', import.meta.url, new URL('./c.cjs', import.meta.url).href);\
         const again = await import('./c.cjs');\
         console.log(actual.default === required, actual === again, old === actual, actual.a, actual.value, globalThis.cRuns, globalThis.cReads);",
        &["true true false 1 7 1 1"],
    );
}

#[test]
fn runner_import_uses_current_generation_after_nested_reset_but_old_closure_keeps_its_generation() {
    let project = Project::new();
    project.write(
        "leaf.mjs",
        "globalThis.leafRuns = (globalThis.leafRuns || 0) + 1; export const run = globalThis.leafRuns;",
    );
    project.write(
        "helper.mjs",
        "export const reset = (loader) => loader('reset');",
    );
    project.write("old.mjs", "export const load = () => import('./leaf.mjs');");
    expect_lines(
        &project,
        "const loader = process.__kilnNativeModules.__kilnHostModuleLoader;\n\
         if (!loader('registerRunner', import.meta.url)) throw new Error('runner registration failed');\n\
         import { reset } from './helper.mjs';\n\
         const old = await import('./old.mjs'); const oldLeaf = await old.load();\n\
         reset(loader);\n\
         const runnerLeaf = await import('./leaf.mjs'); const retainedLeaf = await old.load();\n\
         console.log(oldLeaf.run, runnerLeaf.run, oldLeaf !== runnerLeaf, retainedLeaf === oldLeaf, globalThis.leafRuns);",
        &["1 2 true true 2"],
    );
}

#[test]
fn reset_recreates_dynamic_cycle_and_keeps_old_generation_namespace_alive() {
    let project = Project::new();
    project.write(
        "a.mjs",
        "globalThis.aRuns = (globalThis.aRuns || 0) + 1; import { b } from './b.mjs'; export const a = () => 'a' + b();",
    );
    project.write(
        "b.mjs",
        "globalThis.bRuns = (globalThis.bRuns || 0) + 1; import { a } from './a.mjs'; export const b = () => 'b'; export const readA = () => a();",
    );
    expect_lines(
        &project,
        "const loader = process.__kilnNativeModules.__kilnHostModuleLoader;\n\
         if (!loader('registerRunner', import.meta.url)) throw new Error('runner registration failed');\n\
         const old = await import('./a.mjs');\n\
         const oldValue = old.a();\n\
         loader('reset');\n\
         await (async () => { const nested = async () => { await Promise.resolve(); }; await nested(); })();\n\
         const fresh = await loader('importActual', './a.mjs', import.meta.url, new URL('./a.mjs', import.meta.url).href);\n\
         console.log(oldValue, fresh.a(), old !== fresh, globalThis.aRuns, globalThis.bRuns);",
        &["ab ab true 2 2"],
    );
}

#[test]
fn dynamic_esm_default_import_of_commonjs_uses_the_published_default() {
    let project = Project::new();
    project.write("value.cjs", "module.exports = (value) => value + 1;");
    project.write(
        "wrapper.mjs",
        "import value from './value.cjs'; export default value;",
    );
    let (lines, outcome) = project.run(
        "const wrapper = await import('./wrapper.mjs');\n\
         console.log(wrapper.default(41));",
    );
    assert!(outcome.is_ok(), "{outcome:?}");
    assert_eq!(lines, ["42"]);
}

#[test]
fn export_star_as_is_the_modules_own_namespace_everywhere() {
    let project = Project::new();
    project.write("inner.mjs", "export const b = 2;");
    project.write(
        "barrel.mjs",
        "export * as inner from './inner.mjs';\nexport const outer = 'outer';",
    );
    project.write("dyn-inner.mjs", "export const c = 3;");
    project.write(
        "dyn-barrel.mjs",
        "export * as nested from './dyn-inner.mjs';\nexport const d = 4;",
    );
    expect_lines(
        &project,
        "import * as inner from './inner.mjs';\n\
         import * as barrel from './barrel.mjs';\n\
         const dynBarrel = await import('./barrel.mjs');\n\
         console.log(barrel.inner === inner, dynBarrel === barrel, dynBarrel.inner === inner, Object.keys(dynBarrel).join(','));\n\
         const only = await import('./dyn-barrel.mjs');\n\
         const nested = await import('./dyn-inner.mjs');\n\
         console.log(Object.keys(only).join(','), only.nested === nested, only.nested.c);",
        &["true true true inner,outer", "d,nested true 3"],
    );
}

#[test]
fn a_module_importing_itself_sees_its_own_finished_bindings() {
    let project = Project::new();
    // The entry's body finishes before the job its `import()` queued runs, so the import links
    // to the entry's own record -- live bindings, one namespace -- rather than running it again.
    expect_lines(
        &project,
        "globalThis.runs = (globalThis.runs || 0) + 1;\n\
         let x = 0;\n\
         export { x, x as y };\n\
         async function fn() {\n\
           const imported = await import('./main.mjs');\n\
           const before = [imported.x, imported.y].join();\n\
           x = 1;\n\
           console.log(before, imported.x, imported.y, imported === await import('./main.mjs'), globalThis.runs);\n\
         }\n\
         fn();",
        &["0,0 1 1 true 1"],
    );
}

/// A loader that builds a graph of its own for every request, as the Test262 runner's does: the
/// plan it hands back links by module key, so a module the static graph already evaluated is
/// still not evaluated again.
struct FreshGraphLoader;

impl kiln_interpreter::ModuleLoaderService for FreshGraphLoader {
    fn load(
        &self,
        request: kiln_interpreter::ModuleLoadRequest,
    ) -> Result<kiln_interpreter::ModulePlan, kiln_interpreter::ModuleLoadError> {
        let base = Path::new(request.importer.as_deref().expect("importer"))
            .parent()
            .expect("importer directory")
            .to_path_buf();
        let entry = base
            .join(&request.specifier)
            .canonicalize()
            .expect("canonical target");
        let graph = ModuleGraph::build(
            &entry,
            &base,
            |path: &Path| fs::read_to_string(path),
            kiln_frontend_oxc::parse,
        )
        .expect("build request graph");
        graph
            .import_plan_for_request(&entry, None)
            .map_err(|error| kiln_interpreter::ModuleLoadError::Unsupported(error.to_string()))
    }
}

#[test]
fn a_loader_with_its_own_graph_links_to_the_static_record() {
    let project = Project::new();
    let entry = project.write(
        "main.mjs",
        "globalThis.runs = (globalThis.runs || 0) + 1;\n\
         let x = 0;\n\
         export { x, x as y };\n\
         async function fn() {\n\
           const imported = await import('./main.mjs');\n\
           const before = [imported.x, imported.y].join();\n\
           x = 1;\n\
           console.log(before, imported.x, imported.y, globalThis.runs);\n\
         }\n\
         fn();",
    );
    let graph = ModuleGraph::build(
        &entry,
        &project.root,
        |path: &Path| fs::read_to_string(path),
        kiln_frontend_oxc::parse,
    )
    .expect("build graph");
    let mut host = Output::default();
    graph
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default().with_module_loader_service(Arc::new(FreshGraphLoader)),
        )
        .expect("run");
    assert_eq!(host.lines, ["0,0 1 1 1"]);
}

#[test]
fn commonjs_named_import_reads_accessor_export() {
    let project = Project::new();
    project.write(
        "api.cjs",
        "exports.ValueType = undefined;\n\
         const value = { INT: 0, DOUBLE: 1 };\n\
         Object.defineProperty(exports, 'ValueType', { enumerable: true, get() { return value; } });",
    );
    expect_lines(
        &project,
        "import { ValueType } from './api.cjs'; console.log(ValueType.INT, ValueType.DOUBLE);",
        &["0 1"],
    );
}

#[test]
fn commonjs_namespace_getters_are_lazy_and_snapshotted_once() {
    let project = Project::new();
    project.write("api.cjs", "exports.value = undefined; globalThis.reads = 0; exports.marker = 7; Object.defineProperty(exports, 'value', { enumerable: true, get() { globalThis.reads++; return this.marker; } });");
    project.write("loader.cjs", "exports.api = require('./api.cjs');");
    expect_lines(
        &project,
        "import loader from './loader.cjs'; console.log(globalThis.reads); const ns = await import('./api.cjs'); const again = await import('./api.cjs'); console.log(ns.value, globalThis.reads, ns === again, loader.api === ns.default); loader.api.marker = 9; console.log(ns.value, loader.api.value, globalThis.reads);",
        &["0", "7 1 true true", "7 9 2"],
    );
}

#[test]
fn commonjs_namespace_failure_is_cached_without_invalidating_require() {
    let project = Project::new();
    project.write("api.cjs", "exports.value = undefined; globalThis.reads = 0; globalThis.failure = {}; Object.defineProperty(exports, 'value', { enumerable: true, get() { globalThis.reads++; throw globalThis.failure; } });");
    project.write("loader.cjs", "exports.load = () => require('./api.cjs');");
    expect_lines(
        &project,
        "import loader from './loader.cjs'; const api = loader.load(); let first; try { await import('./api.cjs'); } catch (error) { first = error; } let second; try { await import('./api.cjs'); } catch (error) { second = error; } console.log(first === globalThis.failure, first === second, globalThis.reads, loader.load() === api);",
        &["true true 1 true"],
    );
}

#[test]
fn commonjs_namespace_snapshots_cells_from_required_esm() {
    let project = Project::new();
    project.write(
        "live.mjs",
        "export let n = 1; export function bump() { n++; }",
    );
    project.write(
        "proxy.cjs",
        "exports.n = undefined; exports.bump = undefined; module.exports = require('./live.mjs');",
    );
    expect_lines(
        &project,
        "import { n, bump } from './proxy.cjs'; import live from './proxy.cjs'; bump(); console.log(n, live.n);",
        &["1 2"],
    );
}

#[test]
fn require_esm_keeps_namespace_identity_and_late_commonjs_snapshot() {
    let project = Project::new();
    project.write(
        "live.mjs",
        "export let n = 1; export function bump() { n++; }",
    );
    project.write(
        "proxy.cjs",
        "exports.n = undefined; exports.bump = undefined; module.exports = require('./live.mjs');",
    );
    project.write("loader.cjs", "exports.load = () => require('./proxy.cjs');");
    expect_lines(
        &project,
        "import loader from './loader.cjs'; const live = loader.load(); live.bump(); const esm = await import('./live.mjs'); const proxy = await import('./proxy.cjs'); console.log(live === esm, proxy.default === esm, proxy.n, live.n); live.bump(); console.log(proxy.n, live.n);",
        &["true true 2 2", "2 3"],
    );
}
