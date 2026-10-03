use super::*;

#[test]
fn binary_asset_is_retained_as_an_explicit_path_module() {
    let project = TempProject::new();
    let asset = project.root.join("tree-sitter.wasm");
    fs::write(&asset, [0_u8, 159, 146, 150]).expect("write binary asset");
    let entry = project.write(
        "main.mjs",
        "import wasm from './tree-sitter.wasm' with { type: 'file' }; console.log(wasm);",
    );
    let graph = ModuleGraph::build_node(
        &entry,
        kiln_module_resolver::ModuleResolver::new(&project.root).expect("resolver"),
        |path: &Path| fs::read_to_string(path).map_err(|error| error.to_string()),
        OxcFrontend,
    )
    .expect("build asset graph");
    let node = graph.module(&asset).expect("asset node");
    assert_eq!(node.source, ModuleSource::Asset);
    let expected = fs::canonicalize(&asset).unwrap();
    assert_eq!(
        node.data.as_deref(),
        Some(expected.to_string_lossy().as_ref())
    );
}

#[test]
fn delayed_exported_async_import_uses_defining_module_referrer() {
    let project = TempProject::new();
    fs::create_dir_all(project.root.join("src")).unwrap();
    let entry = project.write(
        "main.mjs",
        "import('./src/processCapability.mjs').then(ns => ns.run().then(ns => console.log(ns.source)));\n\
         import('./dep.mjs').then(ns => console.log(ns.source));",
    );
    project.write(
        "src/processCapability.mjs",
        "export const run = async () => { await Promise.resolve(); return import('./dep.mjs'); };",
    );
    project.write("src/dep.mjs", "export const source = 'src';");
    project.write("dep.mjs", "export const source = 'entry';");

    let graph = Arc::new(project.graph(&entry).expect("build referrer graph"));
    let loader = GraphModuleLoader::new(Arc::clone(&graph));
    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default().with_module_loader_service(Arc::new(loader)),
        )
        .expect("execute delayed imports");
    assert!(
        host.output.iter().any(|line| line == "src"),
        "{:?}",
        host.output
    );
    assert!(
        host.output.iter().any(|line| line == "entry"),
        "{:?}",
        host.output
    );
}

#[test]
fn failed_static_subtree_still_rejects_graph_build() {
    let project = TempProject::new();
    let entry = project.write("main.mjs", "import './optional.mjs';");
    project.write("optional.mjs", "import './missing.mjs';");
    let error = project
        .graph(&entry)
        .expect_err("a static bad dependency remains fatal");
    assert!(error.to_string().contains("missing.mjs"));
}

#[test]
fn express_router_match_has_no_caller_relative_module_reads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../conformance/server/packages/node_modules/router/lib/layer.js");
    if !path.exists() {
        return;
    }
    let source = fs::read_to_string(&path).expect("read pinned router layer");
    let hir = kiln_frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source);
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
        .join("../../../conformance/server/packages/node_modules/path-to-regexp/dist/index.js");
    if !path.exists() {
        return;
    }
    let source = fs::read_to_string(&path).expect("read pinned path-to-regexp");
    let hir = kiln_frontend_oxc::parse_commonjs(path.to_str().unwrap(), &source);
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
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
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
    for (path, node) in graph.module_map().iter() {
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
fn pinned_js_format_survives_graph_entry_without_reclassification() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "import value from './dep.js'; console.log(value);",
    );
    let dep = project.write("dep.js", "export default 42;");
    let dep_canonical = fs::canonicalize(&dep).unwrap();
    let resolver = PinnedPackageResolver::new_with_node_builtins_and_formats(
        &project.root,
        [(entry.clone(), "./dep.js".to_owned(), dep.clone())],
        [(dep_canonical, NodeModuleFormat::EsModule)],
    )
    .expect("pinned resolver");
    let graph = ModuleGraph::build_node(
        &entry,
        resolver,
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("pinned CommonJS graph");
    assert!(
        graph
            .modules()
            .find(|node| node.path.ends_with("dep.js"))
            .is_some_and(|node| node.source == ModuleSource::Source)
    );
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
    let program = compile_module(&kiln_frontend_oxc::parse(
        dependency.to_str().expect("utf8 dependency"),
        "console.log('new'); export const answer = 7;",
    ))
    .expect("compile body edit");

    let patched = graph
        .patch_module(&dependency, program)
        .expect("body-only edit can reuse graph topology");
    assert!(std::ptr::eq(graph.module_map(), patched.module_map()));
    assert!(std::ptr::eq(
        graph.export_resolutions(),
        patched.export_resolutions()
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

    let changed_export = compile_module(&kiln_frontend_oxc::parse(
        dependency.to_str().expect("utf8 dependency"),
        "console.log('new'); export const other = 7;",
    ))
    .expect("compile interface edit");
    assert!(graph.patch_module(&dependency, changed_export).is_none());

    let changed_import = compile_module(&kiln_frontend_oxc::parse(
        dependency.to_str().expect("utf8 dependency"),
        "import './other.js'; export const answer = 7;",
    ))
    .expect("compile import edit");
    assert!(graph.patch_module(&dependency, changed_import).is_none());
}

#[test]
fn replacement_relinks_a_smaller_export_interface() {
    let project = TempProject::new();
    let dependency = project.write(
        "dependency.js",
        "export const answer = 42; export function unused() { return 0; }",
    );
    let entry = project.write(
        "entry.js",
        "import { answer } from './dependency.js'; console.log(answer);",
    );
    let graph = project.graph(&entry).expect("build graph");
    let dependency = fs::canonicalize(dependency).expect("canonical dependency");
    let demanded = BTreeSet::from(["answer".to_owned()]);
    let replacement = compile_module(&kiln_frontend_oxc::parse_dce(
        dependency.to_str().expect("utf8 dependency"),
        &graph.module(&dependency).unwrap().program.program.source,
        &demanded,
    ))
    .expect("compile specialized dependency");

    let specialized = graph
        .replace_programs_and_relink(BTreeMap::from([(dependency.clone(), replacement)]))
        .expect("relink smaller interface");
    assert!(specialized.resolved_export(&dependency, "answer").is_some());
    assert!(specialized.resolved_export(&dependency, "unused").is_none());
    let mut host = TestHost::default();
    specialized
        .execute(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute relinked graph");
    assert_eq!(host.output, ["42"]);
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
    let first_program = compile_module(&kiln_frontend_oxc::parse(
        first.to_str().expect("utf8 first"),
        "console.log('first new'); export const first = 10;",
    ))
    .expect("compile first body edit");
    let second_program = compile_module(&kiln_frontend_oxc::parse(
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
fn reexport_from_star_export_ts() {
    let project = TempProject::new();
    project.write(
        "agent-session-runtime.ts",
        "export class AgentSessionRuntime {}",
    );
    let _sdk = project.write("sdk.ts", "export * from './agent-session-runtime.ts';");
    let entry = project.write(
        "index.ts",
        "export { AgentSessionRuntime } from './sdk.ts';",
    );
    let graph = project.graph(&entry).expect("build graph");
    assert!(
        graph
            .resolved_export(&entry, "AgentSessionRuntime")
            .is_some()
    );
}

#[test]
fn debug_pi_index_ts() {
    let handle = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(|| {
            let index_path = std::path::Path::new(
                "/path/to/pi/packages/coding-agent/src/cli.ts",
            );
            if !index_path.exists() {
                return;
            }
            let project_root = std::path::Path::new("/path/to/pi");
            let res = ModuleGraph::build_node(
                index_path,
                kiln_module_resolver::NodeRuntimeResolver::new(project_root).expect("resolver"),
                |path: &std::path::Path| {
                    fs::read_to_string(path).map_err(|error| error.to_string())
                },
                OxcFrontend,
            );
            if let Ok(graph) = &res {
                let sdk_path = std::path::Path::new(
                    "/path/to/pi/packages/coding-agent/src/core/sdk.ts",
                );
                let resolved = graph.resolved_export(sdk_path, "AgentSessionRuntime");
                eprintln!("DEBUG_PI SDK RESOLVED: {:?}", resolved);
            }
            assert!(res.is_ok(), "failed: {:?}", res.err());
        })
        .unwrap();
    handle.join().unwrap();
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
                debug_call_sites: Vec::new(),
                version: BYTECODE_VERSION,
                source_origin: None,
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
                debug_call_sites: Vec::new(),
                version: BYTECODE_VERSION,
                source_origin: None,
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
        Err(ModuleEvaluationError::Execution {
            error: ExecutionError::StepLimitExceeded { steps: 1, limit: 1 },
            ..
        })
    ));
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
fn reduce_to_live_drops_unrequested_reexports_with_dead_providers() {
    let project = TempProject::new();
    project.write("package.json", r#"{"type":"module","sideEffects":false}"#);
    let used = project.write("used.mjs", "export const answer = 42;\n");
    let dead = project.write("dead.mjs", "export const hidden = 7;\n");
    let barrel = project.write(
        "barrel.mjs",
        "export { answer } from './used.mjs';\nexport { hidden } from './dead.mjs';\n",
    );
    let entry = project.write(
        "main.mjs",
        "import { answer } from './barrel.mjs'; console.log(answer);\n",
    );
    let graph = project.graph(&entry).expect("re-export graph");
    let used_slot = match graph
        .resolved_export(&barrel, "answer")
        .expect("answer re-export")
    {
        ResolvedExport::Local { module, local_slot } => {
            assert_eq!(module, &used);
            *local_slot
        }
        resolution => panic!("expected a local re-export, got {resolution:?}"),
    };
    let demand = LiveDemand {
        live: BTreeSet::from([entry.clone(), barrel.clone(), used.clone()]),
        slots: BTreeMap::from([(used.clone(), BTreeSet::from([used_slot]))]),
        live_imports: BTreeSet::from([(entry.clone(), 0), (barrel.clone(), 0)]),
    };

    let (reduced, _) = graph
        .reduce_to_live(&demand)
        .expect("dead unused re-export is removed without a dangling link");

    assert!(reduced.module(&dead).is_none());
    let reduced_barrel = reduced.module(&barrel).expect("barrel retained");
    assert_eq!(reduced_barrel.program.imports.len(), 1);
    assert_eq!(reduced_barrel.program.exports.len(), 1);
    assert_eq!(reduced_barrel.program.exports[0].exported, "answer");
    assert_eq!(
        reduced.resolved_export(&barrel, "answer"),
        graph.resolved_export(&barrel, "answer")
    );
    assert!(reduced.resolved_export(&barrel, "hidden").is_none());
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
    assert_eq!(
        entry_node.program.dependencies,
        vec![entry_node.program.imports[0].source.clone()],
        "rewritten synthetic import remains a static request in the reduced program"
    );
    assert_eq!(remapped.live_imports, BTreeSet::from([(entry, 0)]));
}

/// Node 22+ syntax detection: an ambiguous `.ts` file (no definitive extension, no explicit
/// `"type"`) carrying static module syntax builds as ESM, so `import fs from "node:fs"`
/// links instead of compiling to an unbound name.
#[test]
fn ambiguous_ts_with_static_imports_builds_as_esm() {
    let project = TempProject::new();
    let entry = project.write(
        "main.ts",
        "import fs from \"node:fs\";\nconsole.log(typeof fs.readFileSync);",
    );
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.root).expect("resolver"),
        |path: &Path| fs::read_to_string(path).map_err(|error| error.to_string()),
        OxcFrontend,
    )
    .expect("ambiguous TS with imports builds as ESM");
    let node = graph.module(&entry).expect("entry node");
    assert_eq!(node.source, ModuleSource::Source);
    assert!(
        node.program
            .imports
            .iter()
            .any(|import| import.source == "node:fs" && import.local == "fs"),
        "the builtin import survives as a linked binding: {:?}",
        node.program.imports
    );
}

#[test]
fn reduce_to_live_retargets_commonjs_default_through_a_dropped_barrel() {
    let project = TempProject::new();
    let origin = project.write("origin.cjs", "module.exports = function FormData() {};\n");
    let barrel = project.write("barrel.mjs", "export { default } from './origin.cjs';\n");
    let entry = project.write(
        "main.mjs",
        "import FormData from './barrel.mjs'; console.log(typeof FormData);\n",
    );
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.root).expect("resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("CommonJS barrel graph");
    let demand = LiveDemand {
        live: graph
            .modules()
            .filter(|node| node.path != barrel)
            .map(|node| node.path.clone())
            .collect(),
        slots: BTreeMap::from([(origin.clone(), BTreeSet::from([0]))]),
        live_imports: BTreeSet::from([(entry.clone(), 0)]),
    };
    let (reduced, _) = graph
        .reduce_to_live(&demand)
        .expect("retarget virtual default");
    assert!(reduced.module(&barrel).is_none());
    let node = reduced.module(&entry).unwrap();
    assert_eq!(node.program.imports[0].imported, "default");
    assert_eq!(node.dependencies[0].target, origin);
    let mut host = kiln_runtime::CaptureHost::new();
    reduced
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute retargeted CommonJS default");
    assert_eq!(host.into_stdout(), "function\n");
}
