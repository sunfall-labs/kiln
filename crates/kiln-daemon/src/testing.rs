//! Resident conformance coordination: discover once, select deterministically, then run the
//! selected batch without launching a compiler process per test.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use kiln_conformance::{CaseResult, ConformanceTest, Outcome, Provenance, ReasonReport, RunMode};
use kiln_proto::{
    Diagnostic, TestCaseSummary, TestMode, TestOutcome, TestReasonSummary, TestSummary,
};
use kiln_test_selector::{ChangeSet, FailureState, TestId, TestMetadata, TestTier, select};

pub fn run(
    root: &str,
    mode: TestMode,
    changed_paths: Option<Vec<String>>,
    previously_failed: Vec<String>,
) -> Result<TestSummary, String> {
    let started = Instant::now();
    let root = Path::new(root);
    // Discovery reads whichever convention the corpus declares itself in - Test262 frontmatter or
    // a manifest beside the files - and hands back one shape, so nothing below here branches on it.
    let discovered = kiln_conformance::discover(root).map_err(|error| error.to_string())?;
    let run_mode = match mode {
        TestMode::Parse => RunMode::Parse,
        TestMode::Interpreter => RunMode::Interpreter,
    };

    let mut tests_by_id = BTreeMap::new();
    let mut metadata = Vec::with_capacity(discovered.len());
    for test in discovered {
        metadata.push(selector_metadata(&test, mode));
        tests_by_id.insert(test.id().to_owned(), test);
    }

    let changes = changed_paths
        .as_deref()
        .map(|paths| change_set_for_tests(paths, tests_by_id.values()))
        .unwrap_or_else(ChangeSet::full_run);
    let failures =
        FailureState::from_failing(previously_failed.iter().map(|id| TestId::new(file_id(id))));
    let selection = select(metadata, &changes, &failures);

    let mut summary = TestSummary {
        discovered: tests_by_id.len(),
        ..TestSummary::default()
    };
    let mut results = Vec::new();
    for selected in selection.tests {
        let Some(test) = tests_by_id.get(selected.id.as_str()) else {
            continue;
        };
        for variant in test.run(run_mode) {
            let variant_name = variant.variant.name();
            let id = match test.provenance() {
                // A manifest expectation records one observed invocation, so its stable manifest
                // ID already names the complete case. Realm suffixes distinguish only the
                // multiple executions required by Test262 frontmatter.
                Provenance::Manifest => selected.id.to_string(),
                Provenance::Frontmatter => format!("{}#{variant_name}", selected.id),
            };
            let outcome = wire_outcome(variant.result.outcome);
            count(&mut summary, outcome);
            summary.cases.push(TestCaseSummary {
                id: id.clone(),
                path: selected.id.to_string(),
                variant: variant_name.to_owned(),
                outcome,
                elapsed_nanos: duration_nanos(variant.result.timing.elapsed),
                diagnostics: variant
                    .result
                    .diagnostics
                    .iter()
                    .map(|diagnostic| Diagnostic {
                        message: format!("{}: {}", diagnostic.phase.tag(), diagnostic.message),
                        start: diagnostic.span.map(|span| span.start),
                        end: diagnostic.span.map(|span| span.end),
                    })
                    .collect(),
            });
            results.push((id, variant.result));
        }
    }
    summary.reasons = reason_distribution(&results);
    summary.selected = summary.cases.len();
    summary.elapsed_millis = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    Ok(summary)
}

/// Group every non-passing result by reason, so the summary carries the backlog and not only its
/// size.  The grouping itself lives in `kiln-conformance` beside the runners that produce the
/// reasons; this only puts it on the wire.
fn reason_distribution(results: &[(String, CaseResult)]) -> Vec<TestReasonSummary> {
    ReasonReport::from_results(results.iter().map(|(id, result)| (id.as_str(), result)))
        .groups()
        .iter()
        .map(|group| TestReasonSummary {
            count: group.count(),
            outcome: wire_outcome(group.outcome),
            reason: group.reason.clone(),
        })
        .collect()
}

fn selector_metadata(test: &ConformanceTest, mode: TestMode) -> TestMetadata {
    let id = test.id();
    let mut metadata = TestMetadata::new(id)
        .with_tier(TestTier::Tier2)
        .compiler_crate("frontend-oxc")
        .compiler_crate("kiln-hir")
        .with_smoke(
            id.split('/').any(|component| component == "smoke")
                || test
                    .features()
                    .iter()
                    .any(|feature| feature == "kiln-smoke"),
        );
    if matches!(mode, TestMode::Interpreter) {
        for compiler_crate in ["kiln-ir", "kiln-interpreter", "kiln-runtime"] {
            metadata.add_compiler_crate(compiler_crate);
        }
    }
    for feature in test.features() {
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

fn change_set_for_tests<'a, I>(paths: &[String], tests: I) -> ChangeSet
where
    I: IntoIterator<Item = &'a ConformanceTest>,
{
    let mut changes = ChangeSet::new();
    let test_paths = tests
        .into_iter()
        .map(|test| (normalize_path(test.path()), TestId::new(test.id())))
        .collect::<Vec<_>>();
    for path in paths {
        let normalized = normalize_path(Path::new(path));
        let components = normalized.split('/').collect::<Vec<_>>();
        if matches!(
            normalized.as_str(),
            "Cargo.toml" | "Cargo.lock" | "docs/design.md"
        ) {
            changes.force_full_run();
        }
        if normalized.rsplit('/').next() == Some("manifest.toml") {
            // The manifest owns corpus IDs, expectations, and metadata. A change anywhere in a
            // manifest can therefore affect every discovered test, even when no source path moved.
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
        for (test_path, test_id) in &test_paths {
            if path_matches(&normalized, test_path) {
                changes.add_changed_test(test_id.clone());
            }
        }
    }
    changes
}

/// Normalize a repository path lexically. Changed paths can come from git on a different host,
/// so this deliberately does not canonicalize or access the filesystem (a deleted path is still a
/// valid change). Leading roots are ignored because matching is by repository-relative suffix.
fn normalize_path(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component),
        }
    }
    components.join("/")
}

fn path_matches(changed: &str, test_path: &str) -> bool {
    !changed.is_empty() && (test_path == changed || test_path.ends_with(&format!("/{changed}")))
}

fn file_id(id: &str) -> &str {
    id.rsplit_once('#').map_or(id, |(path, _)| path)
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
        let changes = change_set_for_tests(
            &[
                "crates/kiln-ir/src/lib.rs".to_owned(),
                "tests/features/BigInt.js".to_owned(),
            ],
            std::iter::empty::<&ConformanceTest>(),
        );
        assert!(changes.compiler_crates.contains("kiln-ir"));
        assert!(changes.features.contains("BigInt"));
        assert!(!changes.full_run);

        assert!(
            change_set_for_tests(
                &["Cargo.toml".to_owned()],
                std::iter::empty::<&ConformanceTest>()
            )
            .full_run
        );
    }

    #[test]
    fn persisted_variant_failures_select_their_source_file() {
        assert_eq!(file_id("language/a.js#strict"), "language/a.js");
        assert_eq!(file_id("language/a.js"), "language/a.js");
    }

    #[test]
    fn changed_conformance_path_selects_one_test_and_not_a_basename_prefix() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let corpus = repo.join("conformance/guard");
        let discovered = kiln_conformance::discover(&corpus).expect("fixture corpus is valid");
        let target = discovered
            .iter()
            .find(|test| test.id() == "guard/g01-spread-symbols")
            .expect("fixture target exists");
        let changed = target
            .path()
            .strip_prefix(&repo)
            .expect("fixture is under repository root")
            .to_string_lossy()
            .replace('\\', "/");

        let changes = change_set_for_tests(&[changed], discovered.iter());
        assert_eq!(changes.changed_tests.len(), 1);
        assert!(changes.changed_tests.contains(&TestId::new(target.id())));

        let windows_changed = target
            .path()
            .strip_prefix(&repo)
            .expect("fixture is under repository root")
            .to_string_lossy()
            .replace('/', "\\");
        let windows_changes = change_set_for_tests(&[windows_changed], discovered.iter());
        assert!(
            windows_changes
                .changed_tests
                .contains(&TestId::new(target.id()))
        );

        let selection = select(
            discovered
                .iter()
                .map(|test| selector_metadata(test, TestMode::Interpreter)),
            &changes,
            &FailureState::new(),
        );
        assert_eq!(selection.tests.len(), 1);
        assert_eq!(selection.tests[0].id, TestId::new(target.id()));

        let basename = target
            .path()
            .file_name()
            .expect("fixture target has a basename")
            .to_string_lossy();
        let prefixed = target.path().with_file_name(format!("not-{basename}"));
        assert!(!path_matches(basename.as_ref(), &normalize_path(&prefixed)));
    }

    #[test]
    fn changed_manifest_source_runs_only_its_stable_test_id() {
        let summary = run(
            &corpus("conformance/guard"),
            TestMode::Parse,
            Some(vec!["conformance/guard/g01-spread-symbols.js".to_owned()]),
            Vec::new(),
        )
        .expect("the changed manifest test is runnable");

        assert_eq!(summary.discovered, 20);
        assert_eq!(summary.selected, 1);
        assert_eq!(summary.cases.len(), 1);
        assert_eq!(summary.cases[0].path, "guard/g01-spread-symbols");
    }

    #[test]
    fn changed_manifest_runs_every_guard_test() {
        let summary = run(
            &corpus("conformance/guard"),
            TestMode::Parse,
            Some(vec!["conformance/manifest.toml".to_owned()]),
            Vec::new(),
        )
        .expect("the manifest change is runnable");

        assert_eq!(summary.discovered, 20);
        assert_eq!(summary.selected, 20);
    }

    #[test]
    fn changed_baseline_does_not_select_ordinary_guard_tests() {
        let summary = run(
            &corpus("conformance/guard"),
            TestMode::Parse,
            Some(vec!["conformance/results/baseline.json".to_owned()]),
            Vec::new(),
        )
        .expect("the baseline change is runnable");

        assert_eq!(summary.discovered, 20);
        assert_eq!(summary.selected, 0);
        assert!(summary.cases.is_empty());
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

    fn corpus(relative: &str) -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative)
            .to_str()
            .expect("fixture path is UTF-8")
            .to_owned()
    }

    fn run_corpus(relative: &str) -> TestSummary {
        run(&corpus(relative), TestMode::Interpreter, None, Vec::new())
            .expect("the corpus is discoverable")
    }

    #[test]
    fn the_manifest_corpus_runs_and_every_non_pass_keeps_a_reason() {
        let summary = run_corpus("conformance/guard");

        assert_eq!(summary.discovered, 20);
        assert_eq!(
            summary.selected, 20,
            "a recorded stdout is one observation, so a manifest test has one variant"
        );
        assert_eq!(summary.skipped, 0, "the probes are plain JavaScript");
        assert_eq!(
            summary.passed + summary.failed,
            20,
            "every probe is either measured as passing or measured as failing"
        );
        // Deliberately not asserting the pass count. It is a measurement, and pinning it here
        // would make an interpreter improvement look like a broken test.
        for case in summary
            .cases
            .iter()
            .filter(|case| !matches!(case.outcome, TestOutcome::Pass))
        {
            assert!(
                case.diagnostics
                    .first()
                    .is_some_and(|diagnostic| { diagnostic.message.contains(':') }),
                "{} failed without a phase-tagged reason",
                case.id
            );
        }
        assert!(
            summary
                .cases
                .iter()
                .all(|case| case.id == case.path && !case.id.contains('#')),
            "manifest IDs already describe their single observed invocation"
        );
    }

    #[test]
    fn typescript_programs_are_skipped_with_a_reason_rather_than_failed() {
        let summary = run_corpus("conformance/effect");

        assert_eq!(summary.discovered, 12);
        assert_eq!(summary.skipped, 12);
        assert_eq!(summary.failed, 0, "kiln was never asked to run these");
        assert_eq!(
            summary.passed, 0,
            "a skip is not a pass; a corpus that did not run has not passed"
        );
        for case in &summary.cases {
            let reason = &case.diagnostics.first().expect("a skip says why").message;
            assert!(reason.starts_with("harness: "), "{reason}");
            assert!(reason.contains("build.sh"), "{reason}");
        }
    }

    #[test]
    fn one_manifest_serves_each_of_its_groups_and_the_whole_corpus() {
        let whole = run_corpus("conformance");
        assert_eq!(
            whole.discovered, 32,
            "pointing at the corpus root picks up both groups"
        );
        assert!(
            whole
                .cases
                .iter()
                .any(|case| case.path == "guard/g01-spread-symbols"),
            "manifest ids do not move when a run is pointed at a different directory"
        );
    }
}
