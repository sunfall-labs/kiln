//! Resident conformance coordination: discover once, select deterministically, then run the
//! selected batch without launching a compiler process per test.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use kiln_conformance::{Outcome, RunMode, Variant, run_case_variants};
use kiln_proto::{Diagnostic, TestCaseSummary, TestMode, TestOutcome, TestSummary};
use kiln_test_selector::{ChangeSet, FailureState, TestId, TestMetadata, TestTier, select};
use kiln_test262::TestCase;

pub fn run(
    root: &str,
    mode: TestMode,
    changed_paths: Option<Vec<String>>,
    previously_failed: Vec<String>,
) -> Result<TestSummary, String> {
    let started = Instant::now();
    let root = Path::new(root);
    let cases = kiln_test262::discover(root).map_err(|error| error.to_string())?;
    let run_mode = match mode {
        TestMode::Parse => RunMode::Parse,
        TestMode::Interpreter => RunMode::Interpreter,
    };

    let mut cases_by_id = BTreeMap::new();
    let mut metadata = Vec::with_capacity(cases.len());
    for case in cases {
        let id = relative_id(root, &case);
        metadata.push(selector_metadata(&id, &case, mode));
        cases_by_id.insert(id, case);
    }

    let changes = changed_paths
        .as_deref()
        .map(change_set)
        .unwrap_or_else(ChangeSet::full_run);
    let failures =
        FailureState::from_failing(previously_failed.iter().map(|id| TestId::new(file_id(id))));
    let selection = select(metadata, &changes, &failures);

    let mut summary = TestSummary {
        discovered: cases_by_id.len(),
        ..TestSummary::default()
    };
    for selected in selection.tests {
        let Some(case) = cases_by_id.get(selected.id.as_str()) else {
            continue;
        };
        for variant in run_case_variants(case, run_mode) {
            let variant_name = variant_name(variant.variant);
            let id = format!("{}#{variant_name}", selected.id);
            let outcome = wire_outcome(variant.result.outcome);
            count(&mut summary, outcome);
            summary.cases.push(TestCaseSummary {
                id,
                path: selected.id.to_string(),
                variant: variant_name.to_owned(),
                outcome,
                elapsed_nanos: duration_nanos(variant.result.timing.elapsed),
                diagnostics: variant
                    .result
                    .diagnostics
                    .into_iter()
                    .map(|diagnostic| Diagnostic {
                        message: format!("{:?}: {}", diagnostic.phase, diagnostic.message),
                        start: diagnostic.span.map(|span| span.start),
                        end: diagnostic.span.map(|span| span.end),
                    })
                    .collect(),
            });
        }
    }
    summary.selected = summary.cases.len();
    summary.elapsed_millis = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    Ok(summary)
}

fn relative_id(root: &Path, case: &TestCase) -> String {
    case.path
        .strip_prefix(root)
        .unwrap_or(&case.path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn selector_metadata(id: &str, case: &TestCase, mode: TestMode) -> TestMetadata {
    let mut metadata = TestMetadata::new(id)
        .with_tier(TestTier::Tier2)
        .compiler_crate("frontend-oxc")
        .compiler_crate("kiln-hir")
        .with_smoke(
            id.split('/').any(|component| component == "smoke")
                || case
                    .metadata
                    .features
                    .iter()
                    .any(|feature| feature == "kiln-smoke"),
        );
    if matches!(mode, TestMode::Interpreter) {
        for compiler_crate in ["kiln-ir", "kiln-interpreter", "kiln-runtime"] {
            metadata.add_compiler_crate(compiler_crate);
        }
    }
    for feature in &case.metadata.features {
        metadata.add_feature(feature);
        if let Some(module) = feature.strip_prefix("builtin-") {
            metadata.add_builtin_module(module);
        }
        if feature == "console-log" {
            metadata.add_host_op("console.log");
        }
    }
    metadata
}

fn change_set(paths: &[String]) -> ChangeSet {
    let mut changes = ChangeSet::new();
    for path in paths {
        let normalized = path.replace('\\', "/");
        let components = normalized.split('/').collect::<Vec<_>>();
        if matches!(
            normalized.as_str(),
            "Cargo.toml" | "Cargo.lock" | "docs/design.md"
        ) {
            changes.force_full_run();
        }
        if components.first() == Some(&"crates")
            && let Some(compiler_crate) = components.get(1)
        {
            changes.add_compiler_crate(*compiler_crate);
        }
        if let Some(index) = components
            .iter()
            .position(|component| *component == "features")
            && let Some(feature) = components.get(index + 1)
        {
            changes.add_feature(feature.trim_end_matches(".js"));
        }
        if let Some(index) = components
            .iter()
            .position(|component| *component == "builtins")
            && let Some(module) = components.get(index + 1)
        {
            changes.add_builtin_module(module.trim_end_matches(".js"));
        }
        if normalized.contains("host-ops") || normalized.contains("host_ops") {
            changes.add_host_op("console.log");
        }
    }
    changes
}

fn file_id(id: &str) -> &str {
    id.rsplit_once('#').map_or(id, |(path, _)| path)
}

fn variant_name(variant: Variant) -> &'static str {
    match variant {
        Variant::Sloppy => "sloppy",
        Variant::Strict => "strict",
        Variant::Module => "module",
    }
}

fn wire_outcome(outcome: Outcome) -> TestOutcome {
    match outcome {
        Outcome::Pass => TestOutcome::Pass,
        Outcome::Fail => TestOutcome::Fail,
        Outcome::Unsupported => TestOutcome::Unsupported,
        Outcome::Skipped => TestOutcome::Skipped,
    }
}

fn count(summary: &mut TestSummary, outcome: TestOutcome) {
    match outcome {
        TestOutcome::Pass => summary.passed += 1,
        TestOutcome::Fail => summary.failed += 1,
        TestOutcome::Unsupported => summary.unsupported += 1,
        TestOutcome::Skipped => summary.skipped += 1,
    }
}

fn duration_nanos(duration: std::time::Duration) -> u64 {
    duration.as_nanos().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_crates_features_and_global_inputs_map_to_selector_tags() {
        let changes = change_set(&[
            "crates/kiln-ir/src/lib.rs".to_owned(),
            "tests/features/BigInt.js".to_owned(),
        ]);
        assert!(changes.compiler_crates.contains("kiln-ir"));
        assert!(changes.features.contains("BigInt"));
        assert!(!changes.full_run);

        assert!(change_set(&["Cargo.toml".to_owned()]).full_run);
    }

    #[test]
    fn persisted_variant_failures_select_their_source_file() {
        assert_eq!(file_id("language/a.js#strict"), "language/a.js");
        assert_eq!(file_id("language/a.js"), "language/a.js");
    }

    #[test]
    fn local_smoke_corpus_runs_as_one_classified_batch() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance/smoke");
        let summary = run(
            root.to_str().expect("fixture path is UTF-8"),
            TestMode::Interpreter,
            None,
            Vec::new(),
        )
        .expect("local corpus is valid");

        assert_eq!(summary.discovered, 4);
        assert_eq!(summary.selected, 5, "ordinary scripts have two variants");
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.unsupported, 1);
        assert_eq!(summary.skipped, 2, "ordinary execution needs the harness");
        assert_eq!(summary.passed, 2);
    }
}
