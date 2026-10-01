use super::*;

#[test]
fn native_import_meta_reparse_remaps_export_demand_after_slot_compaction() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-import-meta-demand-remap");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create project");
    let entry = project.join("main.mjs");
    fs::write(
        &entry,
        "import { plugin } from './provider.mjs'; console.log(plugin());",
    )
    .expect("write entry");
    let provider = project.join("provider.mjs");
    fs::write(
        &provider,
        "import { unused } from './unused.mjs'; import { live } from './live.mjs'; if (false) console.log(unused); export function plugin() { return live + import.meta.url; }",
    )
    .expect("write provider");
    fs::write(project.join("unused.mjs"), "export const unused = 1;").expect("write unused");
    fs::write(project.join("live.mjs"), "export const live = 2;").expect("write live");

    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build graph");
    let demand =
        kiln_liveness::executable_demand(&graph, &kiln_liveness::SelectionOptions::development())
            .expect("compute demand");
    let old_slot = *demand
        .slots
        .get(&provider)
        .and_then(|slots| slots.iter().next())
        .expect("provider export is demanded");
    let old_live_imports = demand
        .live_imports
        .iter()
        .filter_map(|(path, index)| (path == &provider).then_some(*index))
        .collect::<BTreeSet<_>>();
    assert_eq!(old_live_imports, BTreeSet::from([0, 1]));

    let (reparsed, remapped) = native_import_meta_reparse(&graph, &demand);
    let new_slot = reparsed
        .module(&provider)
        .and_then(|node| {
            node.program
                .exports
                .iter()
                .find(|export| export.exported == "plugin")
        })
        .map(|export| export.local_slot)
        .expect("reparsed provider retains its export");
    assert_ne!(
        old_slot, new_slot,
        "the fixture must exercise slot renumbering"
    );
    assert_eq!(
        remapped.slots.get(&provider),
        Some(&BTreeSet::from([new_slot]))
    );
    let reparsed_provider = reparsed.module(&provider).expect("reparsed provider");
    assert_eq!(reparsed_provider.program.imports.len(), 1);
    assert_eq!(reparsed_provider.program.imports[0].local, "live");
    assert_eq!(
        remapped
            .live_imports
            .iter()
            .filter_map(|(path, index)| (path == &provider).then_some(*index))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([0])
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
    let profile = NativeProfile::Dev;
    let cache_path = project.path.join("emission-cache");
    let cache = ArtifactCache::new(&cache_path);
    let (_, effective_policy) = source_module_input_id(node, "main.js", &policy, profile);
    let expected = graph_source_emitter::emit(node, "main.js", &effective_policy, false).unwrap();
    let (first, first_hit) = cached_graph_module(
        &cache,
        "test-target",
        node,
        "main.js",
        &policy,
        profile,
        false,
        || panic!("eligible Source emission must use the fingerprinted emitter"),
    )
    .unwrap();
    assert!(!first_hit);
    assert_eq!(first, expected);
    // A fresh cache object must hit without entering the emitter at all.
    let reopened = ArtifactCache::new(&cache_path);
    let (cached, cache_hit) = cached_graph_module(
        &reopened,
        "test-target",
        node,
        "main.js",
        &policy,
        profile,
        false,
        || panic!("unchanged module was emitted again"),
    )
    .unwrap();
    assert!(cache_hit);
    assert_eq!(cached, expected);
    let (_, other_target_hit) = cached_graph_module(
        &reopened,
        "other-target",
        node,
        "main.js",
        &policy,
        profile,
        false,
        || panic!("target-distinct cold cache entry must emit"),
    )
    .unwrap();
    assert!(
        !other_target_hit,
        "target identity must remain part of the cache key"
    );

    let baseline = source_module_input_id(node, "main.js", &policy, profile).0;
    let mut identities = vec![baseline];
    let mut changed = node.clone();
    changed.program.program.constants[0] = Constant::Number(2.0_f64.to_bits());
    identities.push(source_module_input_id(&changed, "main.js", &policy, profile).0);
    let mut changed = node.clone();
    changed.program.program.source = "// changed observable source".into();
    identities.push(source_module_input_id(&changed, "main.js", &policy, profile).0);
    let mut changed = node.clone();
    changed.program.strictness.body = !changed.program.strictness.body;
    identities.push(source_module_input_id(&changed, "main.js", &policy, profile).0);
    let mut changed = node.clone();
    changed.program.exports[0].exported = "renamed".into();
    identities.push(source_module_input_id(&changed, "main.js", &policy, profile).0);
    let mut changed = node.clone();
    changed
        .program
        .link_facts
        .immutable_bindings
        .push("value".into());
    identities.push(source_module_input_id(&changed, "main.js", &policy, profile).0);
    let mut changed = node.clone();
    changed.source = ModuleSource::CommonJs;
    identities.push(source_module_input_id(&changed, "main.js", &policy, profile).0);
    identities.push(source_module_input_id(node, "other.js", &policy, profile).0);
    for change in 0..9 {
        let mut changed = policy.clone();
        match change {
            0 => changed.host_clock = true,
            1 => changed.entropy = kiln_backend_c::EmissionEntropyPolicy::Seeded(3),
            2 => changed.timezone = kiln_backend_c::EmissionStringPolicy::Named("UTC".into()),
            3 => changed.locale = kiln_backend_c::EmissionStringPolicy::Host,
            4 => changed.type_profile_record = true,
            5 => changed.type_profile = Some(Arc::new(Default::default())),
            6 => changed.live_module_slots = Some(Arc::new(BTreeSet::new())),
            7 => changed.live_module_slots = Some(Arc::new(BTreeSet::from([0]))),
            8 => changed.generic_only = true,
            _ => unreachable!(),
        }
        identities.push(source_module_input_id(node, "main.js", &changed, profile).0);
    }
    identities.push(source_module_input_id(node, "main.js", &policy, NativeProfile::Check).0);
    identities.push(source_module_input_id(node, "main.js", &policy, NativeProfile::Release).0);
    let unique = identities
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(
        unique.len(),
        identities.len(),
        "an input class reused a cache identity"
    );

    let mut embedded = policy.clone();
    embedded.decoupled_prelude = false;
    let calls = std::cell::Cell::new(0);
    for _ in 0..2 {
        cached_graph_module(
            &cache,
            "test-target",
            node,
            "main.js",
            &embedded,
            profile,
            false,
            || {
                calls.set(calls.get() + 1);
                Ok(expected.clone())
            },
        )
        .unwrap();
    }
    assert_eq!(
        calls.get(),
        2,
        "embedded runtime sources must bypass this cache"
    );
}

#[test]
fn bootstrap_emission_cache_reopens_and_separates_surface_and_policy() {
    let project = TestDirectory::new("kiln-bootstrap-emission-cache");
    let cache_path = project.path.join("bootstrap-cache");
    let policy = EmissionPolicy {
        decoupled_prelude: true,
        compact: true,
        ..Default::default()
    };
    let cache = ArtifactCache::new(&cache_path);
    let (first, program, capabilities, hit) = native_builtin_bootstrap_module(
        &cache,
        "test-target",
        &policy,
        false,
        false,
        kiln_builtins::native_bootstrap_family::ALL,
        None,
    )
    .unwrap();
    assert!(!hit);
    let reopened = ArtifactCache::new(&cache_path);
    let (again, again_program, again_capabilities, hit) = native_builtin_bootstrap_module(
        &reopened,
        "test-target",
        &policy,
        false,
        false,
        kiln_builtins::native_bootstrap_family::ALL,
        None,
    )
    .unwrap();
    assert!(hit);
    assert_eq!(first, again);
    assert_eq!(program, again_program);
    assert!(std::sync::Arc::ptr_eq(&program, &again_program));
    assert_eq!(capabilities, again_capabilities);
    // A surface change and an emission-policy change must not return the earlier bootstrap.
    for (policy, url) in [
        (policy.clone(), true),
        (
            EmissionPolicy {
                no_kiln_extensions: true,
                ..policy.clone()
            },
            false,
        ),
    ] {
        let (_, _, _, hit) = native_builtin_bootstrap_module(
            &reopened,
            "test-target",
            &policy,
            false,
            url,
            kiln_builtins::native_bootstrap_family::ALL,
            None,
        )
        .unwrap();
        assert!(!hit);
    }
    let (_, _, _, selected_hit) =
        native_builtin_bootstrap_module(&reopened, "test-target", &policy, false, false, 0, None)
            .unwrap();
    assert!(
        !selected_hit,
        "selected source needs its own emission record"
    );
    // Issue 17: a release build's specialized bootstrap is its own entry, split into units.
    let (specialized, _, _, hit) = native_builtin_bootstrap_module(
        &reopened,
        "test-target",
        &policy,
        false,
        false,
        kiln_builtins::native_bootstrap_family::ALL,
        Some(1 << 20),
    )
    .unwrap();
    assert!(!hit);
    assert!(first.generic_only && first.chunks.is_empty());
    assert!(!specialized.generic_only);
    assert!(
        specialized.chunks.len() > 1,
        "split into units: {}",
        specialized.chunks.len()
    );
}

#[test]
fn native_bootstrap_family_selection_closes_over_dependencies_and_open_world() {
    use kiln_builtins::native_bootstrap_family as family;
    let mut members = kiln_graph::DemandedMembers::default();
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES
    );
    members.names.insert("toLocaleLowerCase".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES | family::STRING_LOCALE
    );
    members.names.clear();
    members.builtins.insert("Date".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES | family::DATE
    );
    members.builtins.clear();
    members.builtins.insert("Uint8Array".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES | family::TYPED_ARRAY | family::ARRAY_BUFFER
    );
    members.builtins.clear();
    members.names.clear();
    members.builtins.insert("Math".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES
    );
    members.names.insert("floor".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES
    );
    members.names.clear();
    members.names.insert("getOwnPropertyNames".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES
            | family::NUMERIC_MATH
            | family::TYPED_ARRAY
            | family::ARRAY_BUFFER
    );
    members.names.clear();
    members.builtins.clear();
    members.names.insert("f16round".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES
            | family::NUMERIC_MATH
            | family::TYPED_ARRAY
            | family::ARRAY_BUFFER
    );
    members.names.clear();
    members.builtins.insert("BigInt".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES | family::NUMERIC_BIGINT
    );
    members.builtins.clear();
    members.names.insert("asUintN".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::WEB_BOOTSTRAP_DEPENDENCIES | family::NUMERIC_BIGINT
    );
    members.names.clear();
    members.has_computed_access = true;
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::ALL
    );
    members.has_computed_access = false;
    members.builtins.insert("GlobalThis".to_owned());
    assert_eq!(
        native_bootstrap_families(&members, false, kiln_graph::CORE_METHOD_GROUPS),
        family::ALL
    );
}

/// Realms issue 08: in a unit emitted with `shared_region` (the runtime bootstrap), a function
/// reads the bindings of the top level, and of a function the top level calls once, through the
/// realm's unit block instead of capturing their cells -- so the closures made there capture
/// nothing and are made in the shared immutable region. A binding made afresh per loop iteration
/// keeps its capture, and its closures stay the realm's own.
#[test]
fn a_shared_region_unit_reads_once_made_cells_from_the_realm_block() {
    let source = r#"
const shared = { n: 1 };
let counter = 0;
globalThis.top = function top() { counter += 1; return shared.n + counter; };
(function install() {
    const local = 2;
    globalThis.inner = function inner() { return local + shared.n; };
    for (let i = 0; i < 2; i++) {
        globalThis["loop" + i] = function perIteration() { return i + local; };
    }
})();
"#;
    let program =
        kiln_ir::compile_module(&kiln_frontend_oxc::parse("region.js", source)).expect("compiles");
    let policy = EmissionPolicy {
        decoupled_prelude: true,
        compact: true,
        shared_region: true,
        ..Default::default()
    };
    let emitted =
        kiln_backend_c::emit_module_with_logical_path_and_policy(&program, "region.js", policy)
            .expect("emits");
    let c = &emitted.source;
    // A body can be emitted more than once (its generic and typed entries); count the functions.
    let protos = |text: &str, call: &str| {
        text.lines()
            .filter_map(|line| {
                let at = line.find(call).filter(|_| !line.starts_with("#define"))?;
                let args = &line[at + call.len()..];
                Some(args.split(", ").nth(1)?.to_owned())
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    // `top`, `install` and `inner` capture nothing now; `perIteration` still captures `i`.
    assert_eq!(protos(c, "KU_REGION_CLOSURE(").len(), 3, "{c}");
    assert!(c.contains("ku_region_sites[3];"), "{c}");
    assert_eq!(protos(c, "KU_CLOSURE(").len(), 1, "{c}");
    // `install` publishes the cell of `local` to the block; `inner` reads through it.
    assert!(c.contains("kiln_mg["), "{c}");
    assert!(c.contains("] = locals["), "{c}");
    assert!(c.contains("kiln_c99_val_make_unit_cell("), "{c}");
    // Without the policy nothing changes: every closure captures its cells.
    let plain = kiln_backend_c::emit_module_with_logical_path_and_policy(
        &program,
        "region.js",
        EmissionPolicy {
            decoupled_prelude: true,
            compact: true,
            ..Default::default()
        },
    )
    .expect("emits");
    assert!(!plain.source.contains("KU_REGION_CLOSURE("));
    assert!(!plain.source.contains("kiln_c99_val_make_unit_cell("));
    assert_eq!(protos(&plain.source, "KU_CLOSURE(").len(), 4);

    // An exported binding can cross to another generated unit as a cell pointer, so that
    // module must retain GC-backed cells even when its closures qualify for the region.
    let exported = kiln_ir::compile_module(&kiln_frontend_oxc::parse(
        "exported-region.js",
        &source.replace("let counter = 0;", "export let counter = 0;"),
    ))
    .expect("compiles export");
    let guarded = kiln_backend_c::emit_module_with_logical_path_and_policy(
        &exported,
        "exported-region.js",
        EmissionPolicy {
            decoupled_prelude: true,
            compact: true,
            shared_region: true,
            ..Default::default()
        },
    )
    .expect("emits export");
    assert!(!guarded.source.contains("kiln_c99_val_make_unit_cell("));
}

/// Realms issue 08: nearly every closure the core bootstrap makes is a region closure.
#[test]
fn the_core_bootstraps_closures_are_region_closures() {
    let root = TestDirectory::new("kiln-bootstrap-region");
    let policy = EmissionPolicy {
        decoupled_prelude: true,
        compact: true,
        ..Default::default()
    };
    let (bootstrap, _, _, _) = native_builtin_bootstrap_module(
        &root.cache(),
        "test-target",
        &policy,
        false,
        false,
        kiln_builtins::native_bootstrap_family::ALL,
        None,
    )
    .expect("builtin bootstrap emits");
    let region = bootstrap.source.matches("KU_REGION_CLOSURE(").count();
    let captured = bootstrap.source.matches("KU_CLOSURE(").count();
    assert!(
        region >= 300 && captured <= 40,
        "region closure sites {region}, capturing sites {captured}"
    );
}

/// Issue 17: a specialized unit split by `unit_chunk_bytes` -- here one function per unit --
/// compiles unit by unit and links without a duplicate or missing definition, including the
/// receiver caches a method's `this` reads share with callers in other units.
#[test]
fn a_specialized_unit_split_by_bytes_compiles_and_links_as_units() {
    let source = r#"
class Point {
  constructor(x, y) { this.x = x; this.y = y; }
  norm() { return Math.sqrt(this.x * this.x + this.y * this.y); }
  scaled(k) { return new Point(this.x * k, this.y * k); }
}
function total(points) { let s = 0; for (const p of points) s += p.norm(); return s; }
function build(n) { const out = []; for (let i = 0; i < n; i++) out.push(new Point(i, i + 1).scaled(2)); return out; }
export const answer = total(build(10));
// This closure lands in a later C chunk; its loader call must be declared there too.
export function loadLater() { return import('./later.js'); }
"#;
    let hir = kiln_frontend_oxc::parse("split.js", source);
    let program = kiln_ir::compile_module(&hir).expect("compiles");
    let policy = EmissionPolicy {
        decoupled_prelude: true,
        compact: true,
        unit_chunk_bytes: Some(1),
        ..Default::default()
    };
    let module =
        kiln_backend_c::emit_module_with_logical_path_and_policy(&program, "split.js", policy)
            .expect("emits");
    assert!(!module.generic_only);
    assert!(
        module.chunks.len() > 2,
        "one unit per function: {}",
        module.chunks.len()
    );
    let root = TestDirectory::new("kiln-native-split-units");
    fs::write(
        root.path.join(kiln_backend_c::RUNTIME_PRELUDE_HEADER_NAME),
        kiln_backend_c::runtime_prelude_header(),
    )
    .unwrap();
    let mut objects = Vec::new();
    for (index, chunk) in module.chunks.iter().enumerate() {
        let name = format!("unit_{index}.c");
        fs::write(root.path.join(&name), chunk).unwrap();
        let object = format!("unit_{index}.o");
        let compile = std::process::Command::new("cc")
            .current_dir(&root.path)
            .args([
                "-std=c11",
                "-O0",
                "-fno-common",
                "-Werror=implicit-function-declaration",
                "-c",
                &name,
                "-o",
                &object,
            ])
            .output()
            .expect("a C compiler is available for native tests");
        assert!(
            compile.status.success(),
            "unit {index} did not compile:\n{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        objects.push(object);
    }
    // A relocatable link of every unit: a symbol defined twice fails here.
    let link = std::process::Command::new("cc")
        .current_dir(&root.path)
        .args(["-r", "-nostdlib", "-o", "all.o"])
        .args(&objects)
        .output()
        .expect("a linker is available for native tests");
    assert!(
        link.status.success(),
        "units did not link:\n{}",
        String::from_utf8_lossy(&link.stderr)
    );
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native scheduling graph runs");
    assert_eq!(
        run.stdout,
        b"entry body\ndependency microtask\ndependency timer\n"
    );
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
                ..NativeBuildOptions::default()
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
    const kiln_string *str = (const kiln_string *)arguments[0].ptr;
    for (size_t i = 0; i < arguments[0].len; ++i) {{
        if (fputc((int)kiln_string_unit_at(str, i), stdout) == EOF) return KILN_C99_HOST_ERROR;
    }}
    if (fputc('\n', stdout) == EOF) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
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

#[test]
fn embedded_shared_graph_dynamic_import_keeps_context_after_body_returns() {
    use kiln_module_resolver::NodeRuntimeResolver;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping delayed shared dynamic import on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping delayed shared dynamic import: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-shared-delayed-dynamic");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create delayed dynamic project");
    let entry = project.join("main.mjs");
    fs::write(
        &entry,
        "import * as staticNs from './target.mjs';\nPromise.resolve().then(() => import('./target.mjs')).then(ns => console.log(`${ns.x}:${ns === staticNs}`));\n",
    )
    .expect("write delayed dynamic entry");
    fs::write(project.join("target.mjs"), "export const x = 99;\n")
        .expect("write delayed dynamic target");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build delayed dynamic graph");
    let library = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone())
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("build delayed dynamic shared graph");
    let header_name = library
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("delayed dynamic header");
    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <stdio.h>
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != KILN_HOST_OP_WRITE_STDOUT || argc != 1 || arguments == NULL ||
        result == NULL || arguments[0].tag != KILN_VALUE_STRING) return KILN_C99_HOST_ERROR;
    if (arguments[0].len != 0 && arguments[0].ptr == NULL) return KILN_C99_HOST_ERROR;
    const kiln_string *str = (const kiln_string *)arguments[0].ptr;
    for (size_t i = 0; i < arguments[0].len; ++i) {{
        if (fputc((int)kiln_string_unit_at(str, i), stdout) == EOF) return KILN_C99_HOST_ERROR;
    }}
    if (fputc('\n', stdout) == EOF) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
    return KILN_C99_OK;
}}
int main(void) {{
    kiln_host host = {{ host_call, NULL }};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 1;
}}
"#,
            symbol = library.module_symbol,
        ),
    )
    .expect("write delayed dynamic host");
    let host_object = root.path.join("host.o");
    let mut compile = CompileOptions::default();
    compile.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile)
        .expect("compile delayed dynamic host");
    let host_executable = root.path.join("host");
    let loader_directory = library.loader.parent().expect("loader directory");
    let mut link = LinkOptions::default();
    link.runtime_libraries.push(library.output.clone());
    link.extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link)
        .expect("link delayed dynamic host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run delayed dynamic host");
    assert!(
        output.status.success(),
        "delayed dynamic host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"99:true\n");
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
            .matches("kiln_realm_enter(realm, kiln_graph_enter, graph)")
            .count(),
        1
    );
    assert!(!launcher.contains("kiln_graph_enter_module"));
    let (bootstrap, _, _, _) = native_builtin_bootstrap_module(
        &root.cache(),
        "test-target",
        &NativeRealmPolicy::default().emission(),
        false,
        true,
        kiln_builtins::native_bootstrap_family::ALL,
        None,
    )
    .expect("builtin bootstrap emits");
    let bootstrapped = graph_launcher_source_with_imports(
        &graph,
        &modules,
        &NativeRealmPolicy::default(),
        None,
        None,
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
        .find("kiln_c99_builtin_bootstrap_done();")
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
                .find("kiln_realm_enter(realm, kiln_graph_enter, graph)")
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native shared star graph links");
}

#[test]
fn native_commonjs_cycle_observes_current_module_exports_assignment() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-cycle-module-exports");
    fs::write(root.path.join("package.json"), r#"{"type":"commonjs"}"#)
        .expect("write package type");
    fs::write(
        root.path.join("range.js"),
        "class Range {} module.exports = Range; const Comparator = require('./comparator.js'); Range.Comparator = Comparator;",
    )
    .expect("write Range module");
    fs::write(
        root.path.join("comparator.js"),
        "const Range = require('./range.js'); class Comparator {} Comparator.Range = Range; module.exports = Comparator;",
    )
    .expect("write Comparator module");
    let entry = root.path.join("main.js");
    fs::write(
        &entry,
        "const Range = require('./range.js'); console.log(Range.Comparator.Range === Range);",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build cyclic CommonJS graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS-cycle execution: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native cyclic CommonJS graph builds and runs");
    assert_eq!(report.stdout, b"true\n");
}

#[test]
fn native_commonjs_require_resolve_returns_path_without_evaluating_target() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-require-resolve");
    fs::write(root.path.join("package.json"), r#"{"type":"commonjs"}"#)
        .expect("write package type");
    fs::write(
        root.path.join("target.cjs"),
        "globalThis.__requireResolveTargetEvaluated = true; module.exports = 42;",
    )
    .expect("write resolve target");
    let entry = root.path.join("main.cjs");
    fs::write(
        &entry,
        "const resolve = require.resolve; const builtin = resolve('node:path'); const local = resolve('./target.cjs'); console.log(builtin === 'node:path', local.endsWith('/target.cjs'), globalThis.__requireResolveTargetEvaluated === undefined);",
    )
    .expect("write require.resolve entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS require.resolve graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native require.resolve execution: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native require.resolve graph builds and runs");
    assert_eq!(report.stdout, b"true true true\n");
}

#[test]
fn native_commonjs_require_main_points_to_the_entry_module() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-require-main");
    fs::write(root.path.join("package.json"), r#"{"type":"commonjs"}"#)
        .expect("write package type");
    fs::write(
        root.path.join("dep.js"),
        "module.exports = require.main === module;",
    )
    .expect("write dependency");
    let entry = root.path.join("main.js");
    fs::write(
        &entry,
        "const dep = require('./dep.js'); console.log(require.main === module, dep);",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS require.main graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS require.main execution: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native CommonJS require.main graph builds and runs");
    assert_eq!(report.stdout, b"true false\n");
}

#[test]
fn native_graph_cycles_instantiate_live_export_cells_before_evaluation() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-esm-cycle-live-cell");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write package type");
    fs::write(
        root.path.join("a.mjs"),
        "import { readB } from './b.mjs'; export let value = 'a'; export function readA() { return value; } export function change() { value = 'changed'; } console.log(readB()); change(); console.log(readB());",
    )
    .expect("write cycle entry");
    fs::write(
        root.path.join("b.mjs"),
        "import { readA, value } from './a.mjs'; export function readB() { return readA() + ':' + value; }",
    )
    .expect("write cycle dependency");
    let entry = root.path.join("a.mjs");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build cyclic ESM graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native ESM-cycle execution: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native cyclic ESM graph builds and runs");
    if report.exit_code != Some(0) || report.stdout != b"a:a\nchanged:changed\n" {
        eprintln!("exit_code: {:?}", report.exit_code);
        eprintln!("result: {:?}", report.result);
        eprintln!("stderr: {}", String::from_utf8_lossy(&report.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&report.stdout));
    }
    assert_eq!(
        report.exit_code,
        Some(0),
        "native ESM cycle must exit successfully"
    );
    assert_eq!(report.stdout, b"a:a\nchanged:changed\n");
}

#[test]
fn native_graph_commonjs_require_of_esm_returns_the_module_namespace() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-cjs-requires-esm-namespace");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write package type");
    fs::write(
        root.path.join("main.cjs"),
        "const dependency = require('./dependency');\n\
         console.log(typeof dependency, dependency.answer);\n",
    )
    .expect("write CommonJS entry");
    fs::write(
        root.path.join("dependency.js"),
        "export const answer = 42;\n",
    )
    .expect("write ESM dependency");
    let entry = root.path.join("main.cjs");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS to ESM graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS-to-ESM execution: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("CommonJS can synchronously require an ESM dependency");
    if report.exit_code != Some(0) || report.stdout != b"object 42\n" {
        eprintln!("exit_code: {:?}", report.exit_code);
        eprintln!("stderr: {}", String::from_utf8_lossy(&report.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&report.stdout));
    }
    assert_eq!(report.exit_code, Some(0));
    assert_eq!(report.stdout, b"object 42\n");
}

#[test]
fn native_graph_sync_require_rejects_transitive_top_level_await_with_node_code() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-cjs-requires-transitive-tla");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write package type");
    fs::write(
        root.path.join("main.cjs"),
        "try { require('./middle.js'); } catch (error) { console.log(error.code); }\n",
    )
    .expect("write CommonJS entry");
    fs::write(
        root.path.join("middle.js"),
        "import './async-leaf.js'; export const ready = true;\n",
    )
    .expect("write intermediate ESM module");
    fs::write(
        root.path.join("async-leaf.js"),
        "await Promise.resolve(); export const ready = true;\n",
    )
    .expect("write transitive top-level-await module");
    let entry = root.path.join("main.cjs");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS to transitive async ESM graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native transitive TLA require check: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("transitive TLA graph builds and runs");
    if report.exit_code != Some(0) || report.stdout != b"ERR_REQUIRE_ASYNC_MODULE\n" {
        eprintln!("exit_code: {:?}", report.exit_code);
        eprintln!("stderr: {}", String::from_utf8_lossy(&report.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&report.stdout));
    }
    assert_eq!(report.exit_code, Some(0));
    assert_eq!(report.stdout, b"ERR_REQUIRE_ASYNC_MODULE\n");
}

#[test]
fn native_graph_commonjs_loop_reassigns_a_local_function_binding() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-commonjs-loop-current-binding");
    fs::write(
        root.path.join("main.cjs"),
        r#"
function series(onDone, count) {
  let current = onDone;
  for (let j = count - 1; j >= 0; j--) {
    const done = current;
    current = () => done();
  }
  return current();
}
console.log(series(() => 'done', 2));
"#,
    )
    .expect("write CommonJS local-binding regression");
    let entry = root.path.join("main.cjs");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build CommonJS local-binding graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native CommonJS local-binding check: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("CommonJS local-binding graph builds and runs");
    if report.exit_code != Some(0) || report.stdout != b"done\n" {
        eprintln!("exit_code: {:?}", report.exit_code);
        eprintln!("stderr: {}", String::from_utf8_lossy(&report.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&report.stdout));
    }
    assert_eq!(report.exit_code, Some(0));
    assert_eq!(report.stdout, b"done\n");
}

#[test]
fn native_graph_cycle_keeps_builtin_closure_and_namespace_bindings_live() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-esm-cycle-namespace-builtin");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write package type");
    fs::write(
        root.path.join("a.mjs"),
        "import { before, read, describe } from './b.mjs';\n\
         console.log(before());\n\
         export let value = 'first';\n\
         console.log(read());\n\
         value = 'second';\n\
         console.log(read());\n\
         console.log(describe());\n",
    )
    .expect("write cycle entry");
    fs::write(
        root.path.join("b.mjs"),
        "import { basename } from 'node:path';\n\
         import * as a from './a.mjs';\n\
         export function before() {\n\
           let direct = false; let descriptor = false;\n\
           try { a.value; } catch (error) { direct = error instanceof ReferenceError; }\n\
           try { Object.getOwnPropertyDescriptor(a, 'value'); } catch (error) { descriptor = error instanceof ReferenceError; }\n\
           return `tdz:${direct}:${descriptor}`;\n\
         }\n\
         export function read() { return `${basename('/tmp/item.txt')}:${a.value}`; }\n\
         export function describe() { const d = Object.getOwnPropertyDescriptor(a, 'value'); return `descriptor:${d.value}:${typeof d.value}:${d.writable}`; }\n",
    )
    .expect("write cycle dependency");
    let entry = root.path.join("a.mjs");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build cyclic ESM graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native ESM namespace-cycle execution: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native cyclic namespace graph builds and runs");
    if report.exit_code != Some(0)
        || report.stdout
            != b"tdz:true:true\nitem.txt:first\nitem.txt:second\ndescriptor:second:string:true\n"
    {
        eprintln!("exit_code: {:?}", report.exit_code);
        eprintln!("result: {:?}", report.result);
        eprintln!("stderr: {}", String::from_utf8_lossy(&report.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&report.stdout));
    }
    assert_eq!(
        report.exit_code,
        Some(0),
        "native namespace cycle must exit successfully"
    );
    assert_eq!(
        report.stdout,
        b"tdz:true:true\nitem.txt:first\nitem.txt:second\ndescriptor:second:string:true\n"
    );
}

#[test]
fn native_graph_cycle_supports_mutually_reexported_namespaces() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-esm-cycle-namespace-reexports");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write package type");
    fs::write(
        root.path.join("a.mjs"),
        "import { peer as a } from './b.mjs';\n\
         export let label = 'A';\n\
         export * as peer from './b.mjs';\n\
         console.log(`${a.label}:${a.peer.label}`);\n\
         label = 'A2';\n\
         console.log(`${a.label}:${a.peer.label}`);\n",
    )
    .expect("write namespace cycle entry");
    fs::write(
        root.path.join("b.mjs"),
        "export const label = 'B';\n\
         export * as peer from './a.mjs';\n",
    )
    .expect("write namespace cycle dependency");
    let entry = root.path.join("a.mjs");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build mutually re-exported namespace graph");
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping native namespace-re-export cycle execution: no C compiler found");
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native mutually re-exported namespace graph builds and runs");
    if report.exit_code != Some(0) || report.stdout != b"A:B\nA2:B\n" {
        eprintln!("exit_code: {:?}", report.exit_code);
        eprintln!("result: {:?}", report.result);
        eprintln!("stderr: {}", String::from_utf8_lossy(&report.stderr));
        eprintln!("stdout: {}", String::from_utf8_lossy(&report.stdout));
    }
    assert_eq!(
        report.exit_code,
        Some(0),
        "namespace re-export cycle exits successfully"
    );
    assert_eq!(report.stdout, b"A:B\nA2:B\n");
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
    assert!(launcher.contains("    kiln_value imports_0[5];"));
    assert!(launcher.contains("#define kiln_imports_0 (graph->imports_0)"));
    assert!(launcher.contains("_commonjs_wrapper(kiln_imports_0, (size_t)5, "));
    assert!(launcher.contains(&format!(
        "kiln_commonjs_require_{commonjs_index}, kiln_commonjs_resolve_{commonjs_index}, graph, kiln_graph_gc_visit_roots, graph)"
    )));
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
    assert!(wrapper.contains("commonjs_graph->visit_roots = kiln_commonjs_graph_gc_visit_roots;"));
    assert!(wrapper.contains("commonjs_graph->root_context = gc_context;"));
    assert!(
        wrapper
            .contains("commonjs_graph, commonjs_graph->visit_roots, commonjs_graph->root_context)")
    );

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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("native repeated CommonJS require graph builds and runs");
    assert_eq!(cached.stdout, b"true 1\n42\n");

    fs::write(
        root.path.join("pkg/index.js"),
        "if (false) require('./dep.js'); console.log('lazy'); module.exports = 42;",
    )
    .expect("write conditional CommonJS require");
    fs::write(
        root.path.join("pkg/dep.js"),
        "throw new Error('eager require');",
    )
    .expect("write dependency that must stay unevaluated");
    let lazy_graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build conditional CommonJS graph");
    let lazy = builder
        .run_graph(
            &lazy_graph,
            &NativeBuildOptions {
                output: root.path.join("lazy-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("untaken CommonJS require must not evaluate its module");
    assert_eq!(lazy.stdout, b"lazy\n42\n");
}

#[test]
fn native_declared_worker_starts_lazily_and_undeclared_sibling_stays_unavailable() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-declared-worker");
    let entry = project.path.join("main.cjs");
    let worker = project.path.join("worker.cjs");
    let sibling = project.path.join("sibling.cjs");
    fs::write(
        &entry,
        concat!(
            "const { Worker } = require('node:worker_threads');\n",
            "console.log('parent');\n",
            "try { require(process.argv[3]); } catch (error) { console.log('sibling', error.code); }\n",
            "const worker = new Worker(process.argv[2]);\n",
            "worker.on('message', value => console.log('worker', value));\n",
            "worker.on('error', error => { console.error(error); process.exitCode = 1; });\n",
        ),
    )
    .expect("write parent entry");
    fs::write(
        &worker,
        "const { parentPort } = require('node:worker_threads'); parentPort.postMessage(42); parentPort.close();",
    )
    .expect("write worker target");
    fs::write(
        &sibling,
        "console.log('sibling evaluated'); module.exports = 99;",
    )
    .expect("write undeclared sibling");
    let graph = ModuleGraph::build_node_with_includes(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
        vec![(worker.clone(), ModuleSource::CommonJs)],
    )
    .expect("build graph with declared worker target");
    assert!(graph.module(&worker).is_some());
    assert!(graph.module(&sibling).is_none());

    let built = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("program"), NativeProfile::Dev)
                .with_node(true),
        )
        .expect("build native graph with lazy worker target");
    let output = Command::new(&built.output)
        .arg(&worker)
        .arg(&sibling)
        .output_when_launchable()
        .expect("run native graph with worker path");
    assert!(
        output.status.success(),
        "worker process failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"parent\nsibling MODULE_NOT_FOUND\nworker 42\n"
    );
}

#[test]
fn native_module_addressed_actor_calls_and_transfers() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Ok(toolchain) = Toolchain::discover() else {
        return;
    };
    let project = TestDirectory::new("kiln-native-actor");
    let entry = project.path.join("main.mjs");
    let actor = project.path.join("actor.mjs");
    fs::write(
        &actor,
        "import { Actor, transfer } from 'kiln:actors'; export class Counter extends Actor { constructor() { super(); this.n = 1; } add(x) { this.n += x; return this.n; } bounce(value) { return transfer(value); } fail() { throw new RangeError('remote problem'); } }",
    )
    .expect("write actor module");
    fs::write(
        &entry,
        "import { spawnActor, connectActor, transfer } from 'kiln:actors'; const owner = await spawnActor({ module: new URL('./actor.mjs', import.meta.url), export: 'Counter', mailboxCapacity: 1 }); const other = connectActor(structuredClone(owner.ref.address)); console.log(await owner.ref.call('add', [2]), await other.call('add', [3])); const bytes = new ArrayBuffer(2); new Uint8Array(bytes)[0] = 23; const bounced = await other.call('bounce', [transfer(bytes)]); console.log(bytes.byteLength, new Uint8Array(bounced)[0]); try { await other.call('fail', []); } catch (error) { console.log(error.name, error.remoteName); } const controller = new AbortController(); controller.abort(); const retained = new ArrayBuffer(2); try { await other.call('bounce', [transfer(retained)], { signal: controller.signal }); } catch (error) { console.log(error.name, retained.byteLength); } await owner.stop(); try { await other.call('add', [1]); } catch (error) { console.log(error.name); }",
    )
    .expect("write actor entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("Node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build actor graph");
    let built = project
        .builder(toolchain)
        .build_graph(
            &graph,
            &NativeBuildOptions::new(project.path.join("program"), NativeProfile::Dev)
                .with_node(true),
        )
        .expect("build native actor graph");
    let output = Command::new(&built.output)
        .output_when_launchable()
        .expect("run native actor graph");
    assert!(
        output.status.success(),
        "actor process failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"3 6\n0 23\nActorRemoteError RangeError\nAbortError 2\nActorClosedError\n"
    );
    let disabled = Command::new(&built.output)
        .env("KILN_EXTENSIONS", "0")
        .output_when_launchable()
        .expect("run actor with extensions disabled");
    assert!(!disabled.status.success());
    assert!(
        String::from_utf8_lossy(&disabled.stderr)
            .contains("kiln:actors requires enabled Kiln extensions"),
        "{}",
        String::from_utf8_lossy(&disabled.stderr)
    );
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
        format!("*result = kiln_exports_{util_index}[{default_index}]; return KILN_C99_OK;");
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
            },
        )
        .expect("shared graph builds");
}

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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
fn native_commonjs_real_package_callable_executes_after_require() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
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
                    ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                    ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
fn native_deferred_dynamic_import_failures_are_catchable() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping deferred dynamic failure integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-deferred-dynamic-failures");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::write(root.path.join("bad-syntax.mjs"), "export const = ;\n")
        .expect("write syntax failure");
    fs::write(
        root.path.join("bad-export.mjs"),
        "import { missing } from './dep.mjs'; export const value = missing;\n",
    )
    .expect("write missing export failure");
    fs::write(root.path.join("never-called.mjs"), "export const = ;\n")
        .expect("write never-called syntax failure");
    fs::write(root.path.join("dep.mjs"), "export const answer = 42;\n").expect("write dependency");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "console.log('start');\n\
         try { await import('./missing.mjs'); } catch (error) { console.log(error instanceof TypeError); }\n\
         try { await import('./missing-builtin.mjs'); } catch (error) { console.log(error instanceof TypeError); }\n\
         try { await import('./bad-syntax.mjs'); } catch (error) { console.log(error instanceof TypeError); }\n\
         try { await import('./bad-export.mjs'); } catch (error) { console.log(error instanceof TypeError); }\n\
         function neverCalled() { return import('./never-called.mjs'); }\n\
         const ns = await import('./dep.mjs'); console.log(ns.answer);\n",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build deferred failure graph");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("deferred failure graph builds and runs");
    assert_eq!(report.stdout, b"start\ntrue\ntrue\ntrue\ntrue\n42\n");
}

#[test]
fn native_dynamic_import_static_tla_dependency_waits_for_completion() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping dynamic static-TLA integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-static-tla");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::write(
        root.path.join("dep.mjs"),
        "globalThis.dynamicStaticTlaCount = (globalThis.dynamicStaticTlaCount || 0) + 1;\nexport const x = await new Promise(resolve => setTimeout(() => resolve(42), 1));\n",
    )
    .expect("write TLA dependency");
    fs::write(
        root.path.join("target.mjs"),
        "import { x } from './dep.mjs'; export const answer = x + 1;\n",
    )
    .expect("write dynamic target");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "const a = import('./target.mjs'); const b = import('./target.mjs'); const [na, nb] = await Promise.all([a, b]); console.log(na.answer, nb.answer, na === nb, globalThis.dynamicStaticTlaCount);\n",
    )
    .expect("write entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build dynamic static-TLA graph");
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("dynamic static-TLA graph builds and runs");
    assert_eq!(report.exit_code, Some(0), "stderr={:?}", report.stderr);
    assert_eq!(report.stdout, b"43 43 true 1\n");
}

#[test]
fn native_dynamic_import_static_tla_target_rejection_is_cached() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Some(toolchain) = Toolchain::discover().ok() else {
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-static-tla-reject");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(
        root.path.join("dep.mjs"),
        "export const x = await Promise.resolve(42);\n",
    )
    .unwrap();
    fs::write(root.path.join("target.mjs"), "import { x } from './dep.mjs'; globalThis.targetEffect = (globalThis.targetEffect || 0) + 1; throw globalThis.sentinel;\n").unwrap();
    fs::write(root.path.join("target-undefined.mjs"), "import { x } from './dep.mjs'; globalThis.targetUndefinedEffect = (globalThis.targetUndefinedEffect || 0) + 1; throw undefined;\n").unwrap();
    let entry = root.path.join("main.mjs");
    fs::write(&entry, "const sentinel = { marker: 9 }; globalThis.sentinel = sentinel; const a = import('./target.mjs').catch(error => error); const b = import('./target.mjs').catch(error => error); const u1 = import('./target-undefined.mjs').catch(error => error); const u2 = import('./target-undefined.mjs').catch(error => error); const [ea, eb, eu1, eu2] = await Promise.all([a, b, u1, u2]); const ec = await import('./target.mjs').catch(error => error); console.log(typeof ea, ea === sentinel, ea === eb, ea === ec, globalThis.targetEffect, eu1 === undefined, eu2 === undefined, globalThis.targetUndefinedEffect);\n").unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .unwrap();
    assert_eq!(report.exit_code, Some(0), "stderr={:?}", report.stderr);
    assert_eq!(report.stdout, b"object true true true 1 true true 1\n");
}

#[test]
fn native_dynamic_import_rejected_static_dependency_skips_target_body() {
    use kiln_module_resolver::NodeRuntimeResolver;
    let Some(toolchain) = Toolchain::discover().ok() else {
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-static-dep-reject");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).unwrap();
    fs::write(
        root.path.join("dep.mjs"),
        "await Promise.resolve(); throw globalThis.sentinel;\n",
    )
    .unwrap();
    fs::write(root.path.join("target.mjs"), "import './dep.mjs'; globalThis.targetEffect = (globalThis.targetEffect || 0) + 1; export const answer = 42;\n").unwrap();
    let entry = root.path.join("main.mjs");
    fs::write(&entry, "const sentinel = { marker: 8 }; globalThis.sentinel = sentinel; const a = import('./target.mjs').catch(error => error); const b = import('./target.mjs').catch(error => error); const [ea, eb] = await Promise.all([a, b]); console.log(ea === sentinel, eb === sentinel, ea === eb, globalThis.targetEffect || 0);\n").unwrap();
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).unwrap(),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .unwrap();
    let report = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain)
        .run_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join("program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .unwrap();
    assert_eq!(report.exit_code, Some(0), "stderr={:?}", report.stderr);
    assert_eq!(report.stdout, b"true true true 0\n");
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
         import * as staticTimers from 'node:timers';\n\
         const dynamicTimers = await import('node:timers');\n\
         console.log(await loadA(), await loadB(), staticTimers === dynamicTimers);\n",
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
        .expect("dynamic importer graph builds and runs");
        assert_eq!(report.stdout, b"A B true\n", "profile {name}");
    }
}

#[test]
fn native_dynamic_import_initializes_once_and_stays_lazy() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping dynamic lifecycle integration: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-dynamic-import-lifecycle");
    fs::write(root.path.join("package.json"), r#"{"type":"module"}"#).expect("write root package");
    fs::write(
        root.path.join("static.mjs"),
        "globalThis.staticCount = 1;\n",
    )
    .expect("write static dependency");
    fs::write(
        root.path.join("static2.mjs"),
        "globalThis.staticOther = 2;\n",
    )
    .expect("write second static dependency");
    fs::write(
        root.path.join("used.mjs"),
        "globalThis.usedCount = (globalThis.usedCount || 0) + 1; export const value = 42;\n",
    )
    .expect("write used dynamic module");
    fs::write(
        root.path.join("unused.mjs"),
        "globalThis.unusedCount = (globalThis.unusedCount || 0) + 1; export const value = 7;\n",
    )
    .expect("write unused dynamic module");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "import './static.mjs';\n\
         import './static2.mjs';\n\
         function unused() { return import('./unused.mjs'); }\n\
         if (false) unused();\n\
         const first = await import('./used.mjs');\n\
         const second = await import('./used.mjs');\n\
         console.log(globalThis.staticCount, globalThis.usedCount, globalThis.unusedCount || 0, first === second, first.value);\n",
    )
    .expect("write lifecycle entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("dynamic lifecycle graph compiles");
    assert!(
        graph
            .dynamic_dependencies(graph.entry())
            .iter()
            .any(|dependency| dependency.specifier == "./unused.mjs"),
        "{:?}",
        graph.dynamic_dependencies(graph.entry())
    );
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
        .expect("dynamic lifecycle graph builds and runs");
        assert_eq!(report.stdout, b"1 1 0 true 42\n", "profile {name}");
    }
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
        source_origin: None,
        constants: vec![
            Constant::String("embedded\0tail=".into()),
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
                ..NativeBuildOptions::default()
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
        ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
    const kiln_string *str = (const kiln_string *)arguments[0].ptr;
    for (size_t i = 0; i < arguments[0].len; ++i) {
        capture->bytes[i] = (char)kiln_string_unit_at(str, i);
    }
    capture->length = arguments[0].len;
    capture->bytes[capture->length++] = '\n';
    capture->calls += 1;
    *result = (kiln_value){ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 };
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
            source_origin: None,
            constants: vec![Constant::String("thrown\0value".into())],
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
                ..NativeBuildOptions::default()
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
        value.len < 12 || value.ptr == NULL || !kiln_string_equals_ascii((const kiln_string *)value.ptr, "thrown\0value", 12))
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
            source_origin: None,
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
            source_origin: None,
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
    // The linkage arrays are the realm's graph context's fields (realms issue 04 step 5).
    assert!(launcher[..main].contains("    kiln_value exports_"));
    assert!(launcher[..main].contains("    kiln_value imports_"));
    assert!(!launcher.contains("static kiln_value kiln_exports_"));
    assert!(!launcher.contains("static kiln_value kiln_imports_"));
    assert!(!launcher[main..].contains("kiln_value exports_"));
    assert!(!launcher[main..].contains("kiln_value imports_"));
    let builder = NativeBuilder::new(ArtifactCache::new(root.join("cache")), toolchain);
    let first = builder
        .build_graph(
            &first_graph,
            &NativeBuildOptions {
                output: root.join("first-program"),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
            source_origin: None,
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
                ..NativeBuildOptions::default()
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
        let topology = graph_shared_topology_digest(&graph, &modules, None, None);
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
fn graph_topology_identity_includes_runtime_edge_base() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let root = TestDirectory::new("kiln-native-runtime-topology-identity");
    let first = root.path.join("first");
    let second = root.path.join("second");
    for project in [&first, &second] {
        fs::create_dir_all(project).expect("create runtime project");
        fs::write(project.join("dep.cjs"), "module.exports = 1;").expect("write dependency");
        let base_name = if project == &second { "b.cjs" } else { "a.cjs" };
        fs::write(
            project.join("main.mjs"),
            &format!(
                "import {{ createRequire }} from 'node:module'; createRequire('/virtual/{base_name}')('./dep.cjs');"
            ),
        )
        .expect("write runtime entry");
    }
    let identity = |project: &Path| {
        let entry = project.join("main.mjs");
        let graph = ModuleGraph::build_node(
            &entry,
            NodeRuntimeResolver::new(project).expect("node resolver"),
            |path: &Path| fs::read_to_string(path),
            OxcFrontend,
        )
        .expect("build runtime graph");
        let modules = graph
            .modules()
            .map(|node| {
                let logical = logical_module_path(graph.project_root(), &node.path)
                    .expect("logical module path");
                (
                    node,
                    emit_native_graph_module(node, &logical, &NativeRealmPolicy::default())
                        .expect("emit runtime module"),
                )
            })
            .collect::<Vec<_>>();
        graph_shared_topology_digest(&graph, &modules, None, None)
    };
    assert_ne!(identity(&first), identity(&second));
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
            source_origin: None,
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
            source_origin: None,
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
    let mut emission = NativeRealmPolicy::default().emission();
    emission.decoupled_prelude = true;
    let modules = graph
        .modules()
        .map(|node| {
            (
                node,
                emit_module_with_logical_path_and_policy(
                    &node.program,
                    &logical_module_path(graph.project_root(), &node.path)
                        .expect("logical module path"),
                    emission.clone(),
                )
                .expect("module emits"),
            )
        })
        .collect::<Vec<_>>();
    let wrapper_symbol = format!(
        "kiln_shared_graph_{}",
        graph_shared_topology_digest(&graph, &modules, None, None).to_hex()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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
    materialize_prelude_header(&root.path).expect("write prelude header");
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
    let prelude_source = root.path.join("prelude.c");
    let prelude_object = root.path.join("prelude.o");
    fs::write(
        &prelude_source,
        kiln_backend_c::runtime_prelude_source_with_policy(&emission),
    )
    .expect("write prelude source");
    toolchain
        .compile_c(&prelude_source, &prelude_object, &compile_options)
        .expect("compile prelude source");
    objects.push(prelude_object);
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
            source_origin: None,
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
            source_origin: None,
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
                ..NativeBuildOptions::default()
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
    kiln_string string;
}} host_context;

static int host_call(void *opaque, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    host_context *context = (host_context *)opaque;
    if (context == NULL || operation != KILN_HOST_OP_WRITE_STDOUT || result == NULL)
        return KILN_C99_HOST_ERROR;
    if (argc == 0) {{
        if (context->mode == 2) return 7;
        if (context->mode == 3) {{
            *result = (kiln_value){{ .tag = KILN_VALUE_STRING, .flags = 0, .len = 3, .bits = 0 }};
            return KILN_C99_OK;
        }}
        if (context->mode == 1) {{
            *result = (kiln_value){{ .tag = KILN_VALUE_STRING, .flags = 0, .len = 0, .bits = 0 }};
            return KILN_C99_OK;
        }}
        memcpy(context->storage, "dep", 3);
        context->string.unit_len = 3;
        context->string.flags = KILN_STRING_LATIN1;
        context->string.data = context->storage;
        *result = (kiln_value){{ .tag = KILN_VALUE_STRING, .flags = KILN_VALUE_FLAG_OWNED_STRING, .len = 3, .bits = 0 }};
        result->ptr = (const char *)&context->string;
        return KILN_C99_OK;
    }}
    if (argc != 1 || arguments == NULL || arguments[0].tag != KILN_VALUE_STRING)
        return KILN_C99_HOST_ERROR;
    if (context->mode == 1) {{
        if (arguments[0].flags != 0 || arguments[0].len != 0 || arguments[0].ptr == NULL)
            return KILN_C99_HOST_ERROR;
    }} else if (arguments[0].flags != 0 || arguments[0].len != 3 || arguments[0].ptr == NULL ||
               arguments[0].ptr == (const char *)&context->string ||
               !kiln_string_equals_ascii((const kiln_string *)arguments[0].ptr, "dep", 3)) {{
        return KILN_C99_HOST_ERROR;
    }}
    if (context->mode == 0) {{
        *result = (kiln_value){{ .tag = KILN_VALUE_STRING, .flags = KILN_VALUE_FLAG_OWNED_STRING, .len = 3, .bits = 0 }};
        result->ptr = (const char *)&context->string;
    }} else {{
        *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
    }}
    return KILN_C99_OK;
}}

int main(void) {{
    host_context context = {{0, {{0}}, {{0, 0, NULL}}}};
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
                result.len != 3 || result.ptr == NULL || !kiln_string_equals_ascii((const kiln_string *)result.ptr, "dep", 3))
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
        source_origin: None,
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
                ..NativeBuildOptions::default()
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
                ..NativeBuildOptions::default()
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

    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable modules");
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

    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable modules");
    assert_eq!(selected.modules.len(), 2);
    assert!(
        selected
            .modules
            .iter()
            .any(|node| node.path.ends_with("node_modules/example/index.js"))
    );
}

#[test]
fn acyclic_namespace_providers_are_preinstantiated_for_live_exports() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-acyclic-live-namespace");
    fs::write(project.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let provider = project.path.join("provider.mjs");
    fs::write(
        &provider,
        "export let value = 1; export function update(next) { value = next; }",
    )
    .expect("write namespace provider");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import * as ns from './provider.mjs'; export { ns };",
    )
    .expect("write namespace importer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build namespace graph");

    let preinstantiated = graph::preinstantiated_module_paths(&graph);
    let provider = fs::canonicalize(provider).expect("canonical provider path");
    let entry = fs::canonicalize(entry).expect("canonical entry path");
    assert!(preinstantiated.contains(&provider));
    assert!(!preinstantiated.contains(&entry));
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

    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable graph");
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

    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable graph");
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

    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable graph");
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
fn executable_graph_namespace_reexport_demand_keeps_provider_exports() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-full-namespace-reexport-demand");
    fs::write(project.path.join("package.json"), r#"{"type":"module"}"#)
        .expect("write package metadata");
    let provider = project.path.join("provider.js");
    fs::write(
        &provider,
        "export function used() { return 42; } export function otherwiseUnused() { return 7; }",
    )
    .expect("write namespace provider");
    fs::write(
        project.path.join("barrel.js"),
        "export * as helpers from './provider.js';",
    )
    .expect("write namespace re-export");
    let entry = project.path.join("main.cjs");
    fs::write(
        &entry,
        "const { helpers } = require('./barrel.js'); console.log(helpers.used());",
    )
    .expect("write CommonJS entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable graph");
    let provider = fs::canonicalize(provider).expect("canonical provider");
    let provider_node = graph.module(&provider).expect("provider node");
    let barrel = fs::canonicalize(project.path.join("barrel.js")).expect("canonical barrel");
    let barrel_node = graph.module(&barrel).expect("barrel node");
    let namespace_import = barrel_node
        .program
        .imports
        .iter()
        .position(|import| import.imported == kiln_ir::NAMESPACE_IMPORT)
        .expect("barrel namespace import");
    assert!(
        selected
            .live_imports
            .contains(&(barrel.clone(), namespace_import)),
        "the namespace import that backs the re-export must be linked"
    );
    let demanded = selected
        .live_export_slots
        .get(&provider)
        .expect("namespace re-export requires the provider namespace");
    for export in &provider_node.program.exports {
        assert!(
            demanded.contains(&export.local_slot),
            "namespace member {} was not retained",
            export.exported
        );
    }
    assert!(!selected.partial_namespace_members.contains_key(&provider));
}

#[test]
fn executable_graph_named_reexport_demand_keeps_backing_imports() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-named-reexport-import-demand");
    fs::write(
        project.path.join("package.json"),
        r#"{"type":"module","sideEffects":false}"#,
    )
    .expect("write package metadata");
    let provider = project.path.join("provider.js");
    fs::write(
        &provider,
        "export function used() { return 42; } export function otherwiseUnused() { return 7; }",
    )
    .expect("write named export provider");
    let first_barrel = project.path.join("first.js");
    fs::write(
        &first_barrel,
        "export { used as first } from './provider.js';",
    )
    .expect("write first named re-export");
    let second_barrel = project.path.join("second.js");
    fs::write(
        &second_barrel,
        "export { first as selected } from './first.js';",
    )
    .expect("write second named re-export");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import { selected } from './second.js'; console.log(selected());",
    )
    .expect("write named re-export consumer");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");

    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable graph");
    for barrel in [first_barrel, second_barrel] {
        let barrel = fs::canonicalize(barrel).expect("canonical barrel");
        let barrel_node = graph.module(&barrel).expect("barrel node");
        let backing_import = barrel_node
            .program
            .imports
            .iter()
            .position(|import| import.local_slot == barrel_node.program.exports[0].local_slot)
            .expect("re-export has a backing import");
        assert!(
            selected.live_imports.contains(&(barrel.clone(), backing_import)),
            "named re-export backing import must be linked"
        );
        assert!(
            selected.modules.iter().any(|module| module.path == barrel),
            "named re-export owner must stay in the selected graph"
        );
    }
    let provider = fs::canonicalize(provider).expect("canonical provider");
    let provider_node = graph.module(&provider).expect("provider node");
    let demanded = selected
        .live_export_slots
        .get(&provider)
        .expect("named re-export demands provider export");
    let used = provider_node
        .program
        .exports
        .iter()
        .find(|export| export.exported == "used")
        .expect("used export");
    assert!(demanded.contains(&used.local_slot));
}

#[test]
fn executable_graph_commonjs_named_reexport_keeps_chained_backing_imports() {
    use kiln_module_resolver::NodeRuntimeResolver;

    let project = TestDirectory::new("kiln-native-commonjs-named-reexport-demand");
    fs::write(
        project.path.join("package.json"),
        r#"{"type":"module","sideEffects":false}"#,
    )
    .expect("write side-effect-free root package metadata");
    fs::create_dir(project.path.join("provider")).expect("create CommonJS provider directory");
    fs::write(
        project.path.join("provider/package.json"),
        r#"{"type":"commonjs"}"#,
    )
    .expect("write CommonJS package metadata");
    fs::write(
        project.path.join("provider/index.js"),
        "module.exports = { answer: 42 };",
    )
    .expect("write CommonJS provider");
    let first_barrel = project.path.join("first.mjs");
    fs::write(
        &first_barrel,
        "export { answer as first } from './provider/index.js';",
    )
    .expect("write first ESM barrel");
    let second_barrel = project.path.join("second.mjs");
    fs::write(
        &second_barrel,
        "export { first as selected } from './first.mjs';",
    )
    .expect("write second ESM barrel");
    let entry = project.path.join("main.mjs");
    fs::write(
        &entry,
        "import { selected } from './second.mjs'; console.log(selected);",
    )
    .expect("write named CommonJS re-export consumer");

    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("source graph compiles");
    let selected = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select executable graph");

    for barrel in [first_barrel, second_barrel] {
        let barrel = fs::canonicalize(barrel).expect("canonical barrel");
        let barrel_node = graph.module(&barrel).expect("barrel node");
        let backing_import = barrel_node
            .program
            .imports
            .iter()
            .position(|import| import.local_slot == barrel_node.program.exports[0].local_slot)
            .expect("re-export has a backing import");
        assert!(
            selected
                .live_imports
                .contains(&(barrel.clone(), backing_import)),
            "CommonJS named re-export backing import must be linked"
        );
        assert!(
            selected.modules.iter().any(|module| module.path == barrel),
            "CommonJS named re-export owner must stay in the selected graph"
        );
    }
    let provider = fs::canonicalize(project.path.join("provider/index.js"))
        .expect("canonical CommonJS provider");
    assert!(
        selected
            .modules
            .iter()
            .any(|module| module.path == provider)
    );
    assert!(
        selected
            .live_export_slots
            .get(&provider)
            .is_some_and(|slots| slots.contains(&0))
    );
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

    let release = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::production(),
    )
    .expect("select release graph");
    let release_names = release
        .modules
        .iter()
        .filter_map(|node| node.path.file_name())
        .collect::<BTreeSet<_>>();
    assert!(release_names.contains(std::ffi::OsStr::new("production.cjs")));
    assert!(!release_names.contains(std::ffi::OsStr::new("development.cjs")));
    assert_eq!(release.inactive_commonjs_dependencies.len(), 1);

    let dev = kiln_liveness::select_executable_modules(
        &graph,
        &kiln_liveness::SelectionOptions::development(),
    )
    .expect("select dev graph");
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
fn embedded_shared_graph_dynamic_import_rejects_unsupported_builtin() {
    use kiln_module_resolver::NodeRuntimeResolver;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("skipping delayed shared dynamic import on unsupported platform");
        return;
    }
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping delayed shared dynamic import: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-shared-delayed-dynamic");
    let project = root.path.join("project");
    fs::create_dir_all(&project).expect("create delayed dynamic project");
    let entry = project.join("main.mjs");
    fs::write(
        &entry,
        "console.log('start');\nPromise.resolve().then(() => import('./missing-dynamic.mjs')).catch(error => console.log(error instanceof TypeError));\n",
    )
    .expect("write delayed dynamic entry");
    fs::write(project.join("target.mjs"), "export const x = 99;\n")
        .expect("write delayed dynamic target");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&project).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build delayed dynamic graph");
    let library = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain.clone())
        .build_shared_graph(
            &graph,
            &NativeBuildOptions {
                output: root.path.join(shared_library_name()),
                profile: NativeProfile::Dev,
                realm: NativeRealmPolicy::default(),
                march_native: false,
                ..NativeBuildOptions::default()
            },
        )
        .expect("build delayed dynamic shared graph");
    let header_name = library
        .header
        .file_name()
        .and_then(|name| name.to_str())
        .expect("delayed dynamic header");
    let host_source = root.path.join("host.c");
    fs::write(
        &host_source,
        format!(
            r#"#include "{header_name}"
#include <stdio.h>
static int host_call(void *context, uint16_t operation, uint16_t argc,
                     const kiln_value *arguments, kiln_value *result) {{
    (void)context;
    if (operation != KILN_HOST_OP_WRITE_STDOUT || argc != 1 || arguments == NULL || result == NULL)
        return KILN_C99_HOST_ERROR;
    if (arguments[0].tag == KILN_VALUE_BOOLEAN) {{
        if (fputs(arguments[0].bits ? "true" : "false", stdout) == EOF) return KILN_C99_HOST_ERROR;
    }} else if (arguments[0].tag != KILN_VALUE_STRING) return KILN_C99_HOST_ERROR;
    if (arguments[0].len != 0 && arguments[0].ptr == NULL) return KILN_C99_HOST_ERROR;
    const kiln_string *str = (const kiln_string *)arguments[0].ptr;
    for (size_t i = 0; i < arguments[0].len; ++i) {{
        if (fputc((int)kiln_string_unit_at(str, i), stdout) == EOF) return KILN_C99_HOST_ERROR;
    }}
    if (fputc('\n', stdout) == EOF) return KILN_C99_HOST_ERROR;
    *result = (kiln_value){{ .tag = KILN_VALUE_UNDEFINED, .flags = 0, .len = 0, .bits = 0 }};
    return KILN_C99_OK;
}}
int main(void) {{
    kiln_host host = {{ host_call, NULL }};
    kiln_value result;
    return {symbol}(&host, &result) == KILN_C99_OK ? 0 : 1;
}}
"#,
            symbol = library.module_symbol,
        ),
    )
    .expect("write delayed dynamic host");
    let host_object = root.path.join("host.o");
    let mut compile = CompileOptions::default();
    compile.include_dirs.push(root.path.clone());
    toolchain
        .compile_c(&host_source, &host_object, &compile)
        .expect("compile delayed dynamic host");
    let host_executable = root.path.join("host");
    let loader_directory = library.loader.parent().expect("loader directory");
    let mut link = LinkOptions::default();
    link.runtime_libraries.push(library.output.clone());
    link.extra_args
        .push(format!("-Wl,-rpath,{}", loader_directory.display()).into());
    toolchain
        .link_executable(&[host_object], &host_executable, &link)
        .expect("link delayed dynamic host");
    let mut command = Command::new(&host_executable);
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", loader_directory);
    #[cfg(target_os = "macos")]
    command.env("DYLD_LIBRARY_PATH", loader_directory);
    let output = command.output().expect("run delayed dynamic host");
    assert!(
        output.status.success(),
        "delayed dynamic host failed with {:?}: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"start\ntrue\n");
}

/// `kiln check --explain-parallel` asks `parallel_sites_graph`, which stops the build after
/// emission. Its answer must be the build's, and it must not compile or link anything.
#[test]
fn parallel_sites_without_compiling_are_the_builds_own() {
    use kiln_backend_c::parallel::Reason;
    use kiln_module_resolver::NodeRuntimeResolver;
    let Some(toolchain) = Toolchain::discover().ok() else {
        eprintln!("skipping parallel-site planning: no C compiler found");
        return;
    };
    let root = TestDirectory::new("kiln-native-parallel-sites-only");
    let entry = root.path.join("main.mjs");
    fs::write(
        &entry,
        "const xs = [1, 2, 3, 4];\nlet bump = 1;\nbump = 2;\nconsole.log(xs.parallelMap(x => x * 2 + 1).join(','));\nconsole.log(xs.parallelMap(x => x + bump).join(','));\n",
    )
    .expect("write parallel-sites entry");
    let graph = ModuleGraph::build_node(
        &entry,
        NodeRuntimeResolver::new(&root.path).expect("node resolver"),
        |path: &Path| fs::read_to_string(path),
        OxcFrontend,
    )
    .expect("build parallel-sites graph");
    let builder = NativeBuilder::new(test_cache(root.path.join("cache")), toolchain);
    let options = NativeBuildOptions {
        output: root.path.join("program"),
        profile: NativeProfile::Release,
        realm: NativeRealmPolicy::default(),
        ..NativeBuildOptions::default()
    };
    let planned = builder
        .parallel_sites_graph(&graph, &options)
        .expect("plan parallel sites");
    assert!(
        !options.output.exists(),
        "planning must not link an executable"
    );
    let sites: Vec<_> = planned.iter().flat_map(|(_, sites)| sites).collect();
    assert_eq!(sites.len(), 2, "{planned:?}");
    assert!(sites[0].refusal.is_none(), "{planned:?}");
    assert_eq!(
        sites[1].refusal.as_ref().map(|refusal| refusal.reason),
        Some(Reason::CapturesReassigned),
        "{planned:?}"
    );
    let built = builder
        .build_graph(&graph, &options)
        .expect("build parallel-sites graph");
    assert_eq!(planned, built.parallel_sites);
}
