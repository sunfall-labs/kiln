use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, OnceLock},
};

use kiln_type_checker::{CheckOptions, SourceSnapshot, TypeClass, TypeRole, check};
use serde_json::Value;

const FIXTURES: &[(&str, &str)] = &[
    (
        "package-exports.json",
        include_str!("../fixtures/differential/package-exports.json"),
    ),
    (
        "indexed-access.json",
        include_str!("../fixtures/differential/indexed-access.json"),
    ),
    (
        "config-extends-paths.json",
        include_str!("../fixtures/differential/config-extends-paths.json"),
    ),
    (
        "union-narrowing.json",
        include_str!("../fixtures/differential/union-narrowing.json"),
    ),
];

#[test]
fn pinned_typescript_differential_fixtures() {
    static LIBRARY_MODULES: OnceLock<BTreeMap<String, Arc<kiln_hir::Module>>> = OnceLock::new();
    let libraries = LIBRARY_MODULES.get_or_init(|| {
        kiln_type_checker::STANDARD_LIBRARIES
            .iter()
            .map(|(name, source)| {
                let path = format!("/__kiln_typescript_lib/{name}");
                (
                    path.clone(),
                    Arc::new(kiln_frontend_oxc::parse(&path, source)),
                )
            })
            .collect()
    });
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/differential");
    let on_disk = std::fs::read_dir(&fixture_dir)
        .unwrap_or_else(|error| panic!("read differential fixture inventory: {error}"))
        .filter_map(|entry| {
            let name = entry.unwrap().file_name();
            name.to_string_lossy()
                .ends_with(".json")
                .then(|| name.to_string_lossy().into_owned())
        })
        .collect::<BTreeSet<_>>();
    let compiled = FIXTURES
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(on_disk, compiled, "fixture inventory must be explicit");

    let mut ids = std::collections::BTreeSet::new();
    for (filename, raw) in FIXTURES {
        let fixture: Value = serde_json::from_str(raw)
            .unwrap_or_else(|error| panic!("invalid fixture {filename}: {error}"));
        let id = fixture["id"].as_str().expect("fixture must name its case");
        assert!(
            ids.insert(id.to_owned()),
            "duplicate differential case id {id}"
        );
        assert_eq!(fixture["oracle"]["version"], "Version 7.0.2", "{id}");

        let mut snapshot = SourceSnapshot {
            modules: libraries.clone(),
            ..SourceSnapshot::default()
        };
        let files = fixture["files"].as_object().expect("fixture files object");
        for (path, contents) in files {
            let contents = contents
                .as_str()
                .expect("fixture file contents are strings");
            if path.ends_with(".ts")
                || path.ends_with(".tsx")
                || path.ends_with(".mts")
                || path.ends_with(".cts")
                || path.ends_with(".d.ts")
                || path.ends_with(".d.mts")
                || path.ends_with(".d.cts")
            {
                snapshot.modules.insert(
                    path.clone(),
                    Arc::new(kiln_frontend_oxc::parse(path, contents)),
                );
            } else {
                snapshot
                    .auxiliary_files
                    .insert(path.clone(), Arc::from(contents.as_bytes()));
            }
        }

        let roots = fixture["roots"]
            .as_array()
            .expect("fixture roots array")
            .iter()
            .map(|root| root.as_str().expect("root path string").to_owned())
            .collect::<Vec<_>>();
        let raw_options = &fixture["options"];
        let options = CheckOptions {
            strict: raw_options["strict"].as_bool().expect("strict bool"),
            no_unchecked_indexed_access: raw_options["no_unchecked_indexed_access"]
                .as_bool()
                .expect("no_unchecked_indexed_access bool"),
            libraries: raw_options["libraries"]
                .as_array()
                .expect("libraries array")
                .iter()
                .map(|value| value.as_str().expect("library string").to_owned())
                .collect(),
            current_directory: raw_options["current_directory"]
                .as_str()
                .expect("current_directory string")
                .to_owned(),
            config_path: raw_options["config_path"].as_str().map(str::to_owned),
            ..CheckOptions::default()
        };
        let result = check(&snapshot, &roots, &options);
        assert_eq!(
            result.complete,
            fixture["expected"]["complete"]
                .as_bool()
                .expect("expected complete bool"),
            "{id}"
        );

        let mut diagnostics = result
            .diagnostics
            .iter()
            .map(|diagnostic| {
                let path = diagnostic.path.as_deref().unwrap_or("");
                let source = files[path]
                    .as_str()
                    .unwrap_or_else(|| panic!("{id}: no fixture source for diagnostic {path}"));
                let expected = fixture["expected"]["diagnostics"]
                    .as_array()
                    .expect("diagnostics array")
                    .iter()
                    .find(|expected| {
                        expected["file"].as_str() == Some(path)
                            && expected["code"].as_u64() == diagnostic.code.map(u64::from)
                    })
                    .unwrap_or_else(|| panic!("{id}: unexpected diagnostic {diagnostic:?}"));
                let (start, end) = needle_span(
                    source,
                    expected["needle"].as_str().expect("diagnostic needle"),
                    expected["occurrence"].as_u64().unwrap_or(0) as usize,
                );
                assert_eq!(
                    diagnostic.span.map(|span| (span.start, span.end)),
                    Some((start, end)),
                    "{id}: {diagnostic:?}"
                );
                (path.to_owned(), diagnostic.code, start, end)
            })
            .collect::<Vec<_>>();
        let mut expected_diagnostics = fixture["expected"]["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .map(|expected| {
                let path = expected["file"].as_str().expect("diagnostic file");
                let source = files[path].as_str().expect("diagnostic source");
                let (start, end) = needle_span(
                    source,
                    expected["needle"].as_str().expect("diagnostic needle"),
                    expected["occurrence"].as_u64().unwrap_or(0) as usize,
                );
                (
                    path.to_owned(),
                    Some(expected["code"].as_u64().expect("diagnostic code") as u32),
                    start,
                    end,
                )
            })
            .collect::<Vec<_>>();
        diagnostics.sort();
        expected_diagnostics.sort();
        assert_eq!(
            diagnostics, expected_diagnostics,
            "{id} diagnostic code/span mismatch"
        );

        let expected_facts = fixture["expected"]["facts"]
            .as_array()
            .expect("facts array");
        let mut fact_rows = BTreeMap::new();
        for expected in expected_facts {
            let path = expected["file"].as_str().expect("fact file");
            let source = files[path].as_str().expect("fact source");
            let (start, end) = needle_span(
                source,
                expected["needle"].as_str().expect("fact needle"),
                expected["occurrence"].as_u64().unwrap_or(0) as usize,
            );
            let role = parse_role(expected["role"].as_str().expect("fact role"));
            let fact = result
                .types
                .iter()
                .find(|fact| {
                    fact.path == path
                        && fact.span.start == start
                        && fact.span.end == end
                        && fact.role == role
                })
                .unwrap_or_else(|| {
                    panic!("{id}: missing selected source fact at {path}:{start}..{end} ({role:?})")
                });
            assert_eq!(
                fact.class,
                parse_class(expected["class"].as_str().expect("fact class")),
                "{id}"
            );
            assert_eq!(
                fact.display,
                expected["display"].as_str().expect("fact display"),
                "{id}"
            );
            fact_rows.insert((path.to_owned(), start, end, format!("{role:?}")), true);
        }
        assert_eq!(
            fact_rows.len(),
            expected_facts.len(),
            "{id}: duplicate selected fact rows"
        );
    }
    assert_eq!(
        ids.len(),
        4,
        "all committed differential cases must be loaded"
    );
}

fn needle_span(source: &str, needle: &str, occurrence: usize) -> (u32, u32) {
    let mut matches = source.match_indices(needle);
    let (start, text) = matches
        .nth(occurrence)
        .unwrap_or_else(|| panic!("expected needle {needle:?} occurrence {occurrence}"));
    (start as u32, (start + text.len()) as u32)
}

fn parse_role(role: &str) -> TypeRole {
    match role {
        "Expression" => TypeRole::Expression,
        "Parameter" => TypeRole::Parameter,
        "Return" => TypeRole::Return,
        "Annotation" => TypeRole::Annotation,
        _ => panic!("unknown type role {role}"),
    }
}

fn parse_class(class: &str) -> TypeClass {
    match class {
        "Number" => TypeClass::Number,
        "String" => TypeClass::String,
        "Boolean" => TypeClass::Boolean,
        "BigInt" => TypeClass::BigInt,
        "Symbol" => TypeClass::Symbol,
        "Nullish" => TypeClass::Nullish,
        "Any" => TypeClass::Any,
        "Unknown" => TypeClass::Unknown,
        "Composite" => TypeClass::Composite,
        _ => panic!("unknown type class {class}"),
    }
}
