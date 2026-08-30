//! Resident conformance coordination: discover once, select deterministically, then run the
//! selected batch without launching a compiler process per test.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::Instant;

use kiln_conformance::{
    CaseResult, ConformanceTest, Outcome, Provenance, ReasonReport, RunMode, VariantResult,
};
use kiln_proto::{
    Diagnostic, TestCaseSummary, TestMode, TestOutcome, TestReasonSummary, TestSummary,
};
use kiln_test_selector::{ChangeSet, FailureState, TestId, TestMetadata, TestTier, select};
use rayon::prelude::*;

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
    // Resolve the selection into a dense list once, in selection order. Every stage below indexes
    // into this list, which is what keeps thread completion order out of the reported summary:
    // the workers only ever answer "slot `i` produced these variants".
    let pending = selection
        .tests
        .iter()
        .filter_map(|selected| {
            tests_by_id
                .get(selected.id.as_str())
                .map(|test| (selected.id.to_string(), test))
        })
        .collect::<Vec<_>>();
    let executed = execute(&pending, run_mode)?;

    let mut results = Vec::new();
    for ((id, test), variants) in pending.iter().zip(executed) {
        for variant in variants {
            let variant_name = variant.variant.name();
            let case_id = match test.provenance() {
                // A manifest expectation records one observed invocation, so its stable manifest
                // ID already names the complete case. Realm suffixes distinguish only the
                // multiple executions required by Test262 frontmatter.
                Provenance::Manifest => id.clone(),
                Provenance::Frontmatter => format!("{id}#{variant_name}"),
            };
            let outcome = wire_outcome(variant.result.outcome);
            count(&mut summary, outcome);
            summary.cases.push(TestCaseSummary {
                id: case_id.clone(),
                path: id.clone(),
                variant: variant_name.to_owned(),
                outcome,
                elapsed_nanos: duration_nanos(variant.result.timing.elapsed),
                diagnostics: variant
                    .result
                    .diagnostics
                    .iter()
                    .map(|diagnostic| Diagnostic {
                        path: Some(id.clone()),
                        message: format!("{}: {}", diagnostic.phase.tag(), diagnostic.message),
                        start: diagnostic.span.map(|span| span.start),
                        end: diagnostic.span.map(|span| span.end),
                    })
                    .collect(),
            });
            results.push((case_id, variant.result));
        }
    }
    summary.reasons = reason_distribution(&results);
    summary.selected = summary.cases.len();
    // Wall clock, which no longer equals the summed case time now that cases run concurrently.
    // That is the intended reading: `docs/design.md` §13 budgets how long a developer waits for
    // the affected tests, not how much CPU the batch consumed. The per-case `elapsed_nanos` is
    // still each case's own duration, so the two no longer add up and are not meant to.
    summary.elapsed_millis = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    Ok(summary)
}

/// Run every selected test, returning each one's variants in the caller's order.
///
/// Rayon's indexed iterator steals uneven work while collecting results in selection order. Each
/// task builds and drops the interpreter's `Rc` graph locally; only owned results cross threads.
fn execute(
    pending: &[(String, &ConformanceTest)],
    mode: RunMode,
) -> Result<Vec<Vec<VariantResult>>, String> {
    pending
        .par_iter()
        .map(|(_, test)| {
            catch_unwind(AssertUnwindSafe(|| test.run(mode)))
                .map_err(|_| "conformance worker panicked".to_owned())
        })
        .collect()
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
        .map(|test| {
            (
                normalize_path(test.source_path()),
                normalize_path(test.path()),
                test.bundle_input_paths()
                    .iter()
                    .map(|path| normalize_path(path))
                    .collect::<Vec<_>>(),
                TestId::new(test.id()),
            )
        })
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
        for (source_path, program_path, bundle_input_paths, test_id) in &test_paths {
            if path_matches(&normalized, source_path)
                || path_matches(&normalized, program_path)
                || bundle_input_paths
                    .iter()
                    .any(|input| path_matches(&normalized, input))
            {
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_CORPUS_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TemporaryCorpus {
        path: PathBuf,
    }

    impl TemporaryCorpus {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "kiln-changed-bundle-test-{}-{}",
                std::process::id(),
                TEMP_CORPUS_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("effect/src")).expect("create authored source directory");
            fs::create_dir_all(path.join("effect/bundle")).expect("create bundle directory");
            let source = b"console.log('source');\n";
            let bundle = b"console.log('bundle');\n";
            let package = b"{\"name\":\"fixture\"}\n";
            let lock = b"fixture lock\n";
            let build = b"#!/bin/sh\n";
            fs::write(path.join("effect/src/one.ts"), source).expect("write authored source");
            fs::write(path.join("effect/bundle/one.js"), bundle).expect("write executable bundle");
            fs::write(path.join("effect/package.json"), package).expect("write package metadata");
            fs::write(path.join("effect/package-lock.json"), lock).expect("write package lock");
            fs::write(path.join("effect/build.sh"), build).expect("write build script");
            let mut provenance = b"kiln-bundle-provenance-v2\n".to_vec();
            for input in [bundle.as_slice(), source, package, lock, build] {
                provenance.extend_from_slice(input.len().to_string().as_bytes());
                provenance.push(b'\n');
                provenance.extend_from_slice(input);
            }
            fs::write(path.join("effect/bundle/one.js.kiln-inputs"), provenance)
                .expect("write bundle provenance");
            fs::write(
                path.join("manifest.toml"),
                r#"[[test]]
id = "effect/one"
source = "effect/src/one.ts"
bundle = "effect/bundle/one.js"
bundle_inputs = ["effect/package.json", "effect/package-lock.json", "effect/build.sh"]
expected_stdout = "bundle\n"
"#,
            )
            .expect("write manifest");
            Self { path }
        }
    }

    impl Drop for TemporaryCorpus {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

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

    /// The point of the index-keyed collection: the summary must not be able to observe which
    /// worker finished first. Running the same corpus repeatedly is the only honest check, since
    /// a single run of a scheduler that happens to stay ordered proves nothing.
    #[test]
    fn parallel_execution_reports_the_same_order_every_time() {
        let first = run_corpus("conformance");
        for _ in 0..4 {
            let again = run_corpus("conformance");
            assert_eq!(
                first.cases.iter().map(|case| &case.id).collect::<Vec<_>>(),
                again.cases.iter().map(|case| &case.id).collect::<Vec<_>>(),
                "case order is selection order, not completion order"
            );
            assert_eq!(first.reasons, again.reasons);
            assert_eq!(
                (first.passed, first.failed, first.unsupported, first.skipped),
                (again.passed, again.failed, again.unsupported, again.skipped)
            );
        }
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
    fn changed_manifest_source_selects_a_bundle_executed_case() {
        let corpus = TemporaryCorpus::new();
        let discovered = kiln_conformance::discover(&corpus.path).expect("discover bundle case");
        let test = &discovered[0];
        assert!(test.source_path().ends_with("effect/src/one.ts"));
        assert!(test.path().ends_with("effect/bundle/one.js"));

        let changes = change_set_for_tests(&["effect/src/one.ts".to_owned()], discovered.iter());
        assert_eq!(changes.changed_tests.len(), 1);
        assert!(changes.changed_tests.contains(&TestId::new("effect/one")));

        let selection = select(
            discovered
                .iter()
                .map(|test| selector_metadata(test, TestMode::Interpreter)),
            &changes,
            &FailureState::new(),
        );
        assert_eq!(selection.tests.len(), 1);
        assert_eq!(selection.tests[0].id, TestId::new("effect/one"));

        for build_input in [
            "effect/package.json",
            "effect/package-lock.json",
            "effect/build.sh",
        ] {
            let changes = change_set_for_tests(&[build_input.to_owned()], discovered.iter());
            assert!(
                changes.changed_tests.contains(&TestId::new("effect/one")),
                "changing shared bundle input {build_input} selects every dependent test"
            );
        }

        fs::write(
            corpus.path.join("effect/src/one.ts"),
            "console.log('edited source');\n",
        )
        .expect("edit authored source without rebuilding");
        let stale = kiln_conformance::discover(&corpus.path).expect("rediscover stale bundle");
        assert!(stale[0].path().ends_with("effect/src/one.ts"));
        let result = stale[0].run(kiln_conformance::RunMode::Parse);
        assert_eq!(result[0].result.outcome, Outcome::Skipped);
        assert!(
            result[0].result.diagnostics[0].message.contains("stale"),
            "a stale bundle is an explicit harness decision"
        );
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

        assert_eq!(summary.discovered, 5);
        assert_eq!(summary.selected, 6, "ordinary scripts have two variants");
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.unsupported, 0);
        assert_eq!(summary.skipped, 2, "ordinary execution needs the harness");
        assert_eq!(summary.passed, 4);
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

    /// The Effect corpus runs its bundles when the working tree has them and skips when it does
    /// not, and never fails for the difference.
    ///
    /// The bundles are not committed -- they are 2.5 MB of near-duplicate esbuild output -- so
    /// which of the two arms a checkout takes depends on whether `conformance/effect/build.sh`
    /// has been run. Asserting one arm would make this test a statement about the machine it
    /// runs on. What is actually invariant is that every case does one or the other, with a
    /// reason when it skips.
    #[test]
    fn typescript_programs_run_from_a_bundle_or_skip_with_a_reason() {
        let summary = run_corpus("conformance/effect");

        assert_eq!(summary.discovered, 12);
        assert_eq!(summary.skipped + summary.passed, 12);
        assert_eq!(
            summary.failed, 0,
            "either kiln ran the bundle and agreed, or it was never asked"
        );
        for case in &summary.cases {
            if matches!(case.outcome, TestOutcome::Pass) {
                continue;
            }
            let reason = &case.diagnostics.first().expect("a skip says why").message;
            assert!(reason.starts_with("harness: "), "{reason}");
            assert!(reason.contains("build.sh"), "{reason}");
        }
    }

    #[test]
    fn one_manifest_serves_each_of_its_groups_and_the_whole_corpus() {
        let whole = run_corpus("conformance");
        assert_eq!(
            whole.discovered, 39,
            "pointing at the corpus root picks up all groups"
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
