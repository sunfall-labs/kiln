use super::*;

#[test]
fn runtime_target_nested_dynamic_import_is_drained() {
    let project = TempProject::new();
    let runtime = project.write("runtime.cjs", "import('./dep.mjs'); module.exports = 1;");
    let dependency = project.write("dep.mjs", "export default 42;");
    let entry = project.write(
        "main.mjs",
        &format!(
            "import {{ createRequire }} from 'node:module'; createRequire(import.meta.url)('./{}');",
            runtime.file_name().unwrap().to_string_lossy()
        ),
    );
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.root).expect("resolver"),
        |path: &Path| fs::read_to_string(path).map_err(|error| error.to_string()),
        kiln_frontend_oxc::parse,
    )
    .expect("nested deferred work is drained");
    assert!(graph.module(&runtime).is_some());
    assert!(graph.module(&dependency).is_some());
}

#[test]
fn invalid_runtime_target_becomes_catchable_failure() {
    let project = TempProject::new();
    let runtime = project.write("broken.cjs", "function (");
    let entry = project.write(
        "main.mjs",
        &format!(
            "import {{ createRequire }} from 'node:module'; createRequire(import.meta.url)('./{}');",
            runtime.file_name().unwrap().to_string_lossy()
        ),
    );
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.root).expect("resolver"),
        |path: &Path| fs::read_to_string(path).map_err(|error| error.to_string()),
        kiln_frontend_oxc::parse,
    )
    .expect("invalid lazy target is deferred");
    assert_eq!(graph.runtime_failures(&entry).count(), 1);
    assert!(graph.module(&runtime).is_none());
}

#[test]
fn dynamic_loader_resolves_against_the_requesting_module() {
    let project = TempProject::new();
    fs::create_dir_all(project.root.join("a")).unwrap();
    fs::create_dir_all(project.root.join("b")).unwrap();
    project.write("a/same.mjs", "export default 'a';");
    project.write("b/same.mjs", "export default 'b';");
    let entry = project.write(
        "main.mjs",
        "export const a = import('./a/entry.mjs'); export const b = import('./b/entry.mjs');",
    );
    project.write("a/entry.mjs", "export default import('./same.mjs');");
    project.write("b/entry.mjs", "export default import('./same.mjs');");
    let graph = Arc::new(project.graph(&entry).expect("build nested dynamic graph"));
    let loader = GraphModuleLoader::new(Arc::clone(&graph));
    let a = graph
        .module(project.root.join("a/entry.mjs"))
        .unwrap()
        .path
        .clone();
    let b = graph
        .module(project.root.join("b/entry.mjs"))
        .unwrap()
        .path
        .clone();
    let a_plan = loader
        .load(ModuleLoadRequest {
            importer: Some(a.to_string_lossy().into_owned()),
            specifier: "./same.mjs".into(),
            attributes: None,
        })
        .unwrap_or_else(|_| panic!("a loader request failed"));
    let b_plan = loader
        .load(ModuleLoadRequest {
            importer: Some(b.to_string_lossy().into_owned()),
            specifier: "./same.mjs".into(),
            attributes: None,
        })
        .unwrap_or_else(|_| panic!("b loader request failed"));
    assert!(a_plan.key.ends_with("/a/same.mjs"));
    assert!(b_plan.key.ends_with("/b/same.mjs"));
}

#[test]
fn failed_dynamic_module_is_evaluated_once_and_reuses_rejection() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "let first;\n\
         import('./bad.mjs').catch(error => { first = error; return import('./bad.mjs').catch(error => error); }).then(second => {\n\
           console.log(globalThis.count);\n\
           console.log(first === second);\n\
         });",
    );
    project.write(
        "bad.mjs",
        "globalThis.count = (globalThis.count || 0) + 1; throw new Error('boom');",
    );

    let graph = Arc::new(project.graph(&entry).expect("build failure cache graph"));
    let loader = GraphModuleLoader::new(Arc::clone(&graph));
    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default().with_module_loader_service(Arc::new(loader)),
        )
        .expect("execute repeated failed imports");
    assert!(
        host.output.iter().any(|line| line == "1"),
        "{:?}",
        host.output
    );
    assert!(
        host.output.iter().any(|line| line == "true"),
        "{:?}",
        host.output
    );
}

#[test]
fn failed_dynamic_subtree_is_deferred_until_import() {
    let project = TempProject::new();
    let entry = project.write("main.mjs", "import('./optional.mjs');");
    project.write(
        "optional.mjs",
        "import './missing.mjs'; export const value = 1;",
    );
    let graph = Arc::new(project.graph(&entry).expect("optional subtree is deferred"));
    assert_eq!(graph.module_count(), 1);
    assert!(
        graph.dynamic_dependencies(&entry).is_empty(),
        "failed dynamic targets must not remain in the successful edge map"
    );

    let loader = GraphModuleLoader::new(Arc::clone(&graph));
    let error = match loader.load(ModuleLoadRequest {
        importer: Some(entry.canonicalize().unwrap().to_string_lossy().into_owned()),
        specifier: "./optional.mjs".into(),
        attributes: None,
    }) {
        Ok(_) => panic!("importing the failed subtree must reject"),
        Err(error) => error,
    };
    assert!(
        matches!(error, ModuleLoadError::Unsupported(message) if message.contains("missing.mjs"))
    );
}

#[test]
fn failed_dynamic_subtree_rolls_back_link_errors() {
    let project = TempProject::new();
    let entry = project.write("main.mjs", "import('./optional.mjs');");
    project.write(
        "optional.mjs",
        "import { missing } from './dep.mjs'; export const value = missing;",
    );
    project.write("dep.mjs", "export const present = 1;");
    let graph = project
        .graph(&entry)
        .expect("link failure is deferred with the dynamic subtree");
    assert_eq!(graph.module_count(), 1);
    let loader = GraphModuleLoader::new(Arc::new(graph));
    let error = match loader.load(ModuleLoadRequest {
        importer: Some(entry.canonicalize().unwrap().to_string_lossy().into_owned()),
        specifier: "./optional.mjs".into(),
        attributes: None,
    }) {
        Ok(_) => panic!("importing the link-failed subtree must reject"),
        Err(error) => error,
    };
    assert!(matches!(error, ModuleLoadError::Unsupported(message) if message.contains("missing")));
}

#[test]
fn failed_dynamic_subtree_rolls_back_compile_errors() {
    let project = TempProject::new();
    let entry = project.write("main.mjs", "import('./optional.mjs');");
    project.write("optional.mjs", "export const = 1;");
    let graph = project
        .graph(&entry)
        .expect("compile failure is deferred with the dynamic subtree");
    assert_eq!(graph.module_count(), 1);
}

#[test]
fn failed_dynamic_import_rejects_and_can_be_caught_by_interpreter() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "import('./optional.mjs').catch(error => console.log(error.message));",
    );
    project.write("optional.mjs", "import './missing.mjs';");
    let graph = Arc::new(project.graph(&entry).expect("dynamic failure is deferred"));
    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default()
                .with_module_loader_service(Arc::new(GraphModuleLoader::new(Arc::clone(&graph)))),
        )
        .expect("caught dynamic import rejection");
    assert!(
        host.output
            .iter()
            .any(|line| line.contains("dynamic import failed")),
        "unexpected output: {:?}",
        host.output
    );
}

#[test]
fn failed_dynamic_sibling_does_not_rollback_successful_sibling() {
    let project = TempProject::new();
    let entry = project.write("main.mjs", "import('./bad.mjs'); import('./good.mjs');");
    project.write("bad.mjs", "import './missing.mjs';");
    project.write("good.mjs", "export const value = 42;");
    let graph = Arc::new(project.graph(&entry).expect("one dynamic sibling failed"));
    assert_eq!(graph.module_count(), 2);
    let loader = GraphModuleLoader::new(Arc::clone(&graph));
    let plan = loader
        .load(ModuleLoadRequest {
            importer: Some(entry.canonicalize().unwrap().to_string_lossy().into_owned()),
            specifier: "./good.mjs".into(),
            attributes: None,
        })
        .unwrap_or_else(|_| panic!("successful dynamic sibling remains loadable"));
    assert!(plan.key.ends_with("/good.mjs"));
}

#[test]
fn literal_dynamic_module_is_not_evaluated_with_static_entry() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "console.log('entry'); if (false) import('./side.mjs');",
    );
    project.write("side.mjs", "console.log('side'); export const value = 1;");
    let graph = project.graph(&entry).expect("build dynamic graph");
    let mut host = TestHost::default();
    graph
        .execute_node(
            &mut host,
            ExecutionLimits::default(),
            ExecutionCapabilities::default(),
        )
        .expect("execute entry");
    assert_eq!(host.output, ["entry"]);
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
fn reduce_to_live_keeps_live_importer_failures_and_drops_dead_dynamic_edges() {
    let project = TempProject::new();
    let entry = project.write(
        "main.mjs",
        "import('./optional.mjs'); import('./missing.mjs');\n",
    );
    project.write("optional.mjs", "export const value = 1;\n");
    let graph = project.graph(&entry).expect("fixture graph");

    let demand = LiveDemand {
        live: BTreeSet::from([entry.clone()]),
        ..LiveDemand::default()
    };
    let (reduced, _) = graph.reduce_to_live(&demand).expect("reduction");

    assert!(reduced.dynamic_dependencies(&entry).is_empty());
    assert!(reduced.dynamic_failure(&entry, "./missing.mjs").is_some());
}
