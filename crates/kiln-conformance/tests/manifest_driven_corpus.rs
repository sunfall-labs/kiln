//! Behavioural tests for the manifest-driven half of conformance discovery.
//!
//! These go through the public API a coordinator uses — discover a directory, run what comes
//! back — because the paths that matter here are the ones a stale or wrong manifest takes, and
//! those are only reachable from outside.  The corpora are built on disk rather than mocked: a
//! manifest whose entry points at nothing is exactly the case a fake filesystem would hide.

use std::fs;
use std::path::{Path, PathBuf};

use kiln_conformance::{
    CaseResult, DiagnosticPhase, Outcome, Provenance, ReasonReport, RunMode, discover,
};

/// Build a corpus directory of its own, so a failure names one path rather than a shared tree.
fn corpus(name: &str, manifest: &str, programs: &[(&str, &str)]) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("scratch corpus is writable");
    fs::write(directory.join("manifest.toml"), manifest).expect("manifest is writable");
    for (file, source) in programs {
        let path = directory.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("program directory is writable");
        }
        fs::write(path, source).expect("program is writable");
    }
    directory
}

fn entry(id: &str, source: &str, expected_stdout: &str) -> String {
    format!(
        "[[test]]\nid = \"{id}\"\nsource = \"{source}\"\nexpected_stdout = \"\"\"\n{expected_stdout}\"\"\"\n\n"
    )
}

/// Run one corpus and return `(id, result)` pairs in discovery order.
fn run_all(root: &Path) -> Vec<(String, CaseResult)> {
    discover(root)
        .expect("the corpus is discoverable")
        .into_iter()
        .flat_map(|test| {
            let id = test.id().to_owned();
            test.run(RunMode::Interpreter)
                .into_iter()
                .map(move |variant| (id.clone(), variant.result))
        })
        .collect()
}

fn result_for<'a>(results: &'a [(String, CaseResult)], id: &str) -> &'a CaseResult {
    &results
        .iter()
        .find(|(name, _)| name == id)
        .unwrap_or_else(|| panic!("{id} was discovered"))
        .1
}

#[test]
fn a_program_passes_only_when_it_runs_and_prints_exactly_what_was_recorded() {
    let root = corpus(
        "exactness",
        &format!(
            "{}{}{}",
            entry("agrees", "agrees.js", "hello\n"),
            entry("disagrees", "disagrees.js", "hello\n"),
            entry("silent", "silent.js", "hello\n"),
        ),
        &[
            ("agrees.js", "console.log('hello');"),
            ("disagrees.js", "console.log('goodbye');"),
            ("silent.js", "let unused = 1;"),
        ],
    );

    let results = run_all(&root);
    assert_eq!(results.len(), 3);
    assert_eq!(result_for(&results, "agrees").outcome, Outcome::Pass);

    let disagrees = result_for(&results, "disagrees");
    assert_eq!(disagrees.outcome, Outcome::Fail);
    assert_eq!(
        disagrees.stdout, "goodbye\n",
        "what the program actually printed is kept, not just the verdict"
    );
    let reason = disagrees
        .primary_diagnostic()
        .expect("a mismatch records why");
    assert_eq!(reason.phase, DiagnosticPhase::Expectation);
    assert!(reason.message.contains("hello"), "{}", reason.message);
    assert!(reason.message.contains("goodbye"), "{}", reason.message);

    assert_eq!(
        result_for(&results, "silent").outcome,
        Outcome::Fail,
        "printing nothing is a mismatch, not a pass"
    );
}

#[test]
fn a_manifest_entry_whose_file_is_missing_fails_and_names_it() {
    let root = corpus(
        "stale-entry",
        &entry("gone", "gone.js", "anything\n"),
        &[("present.js", "console.log('present');")],
    );

    let results = run_all(&root);
    assert_eq!(results.len(), 1, "the entry is still a test, not a no-op");
    let result = result_for(&results, "gone");
    assert_eq!(
        result.outcome,
        Outcome::Fail,
        "a corpus must not be able to lose a test by letting its path go stale"
    );
    let message = &result
        .primary_diagnostic()
        .expect("a missing file records why")
        .message;
    assert!(message.contains("gone.js"), "{message}");
}

#[test]
fn a_bundled_entry_whose_authored_source_is_missing_still_fails() {
    let root = corpus(
        "missing-bundled-source",
        r#"[[test]]
id = "gone"
source = "gone.js"
bundle = "bundle.js"
expected_stdout = "anything\n"
"#,
        &[("bundle.js", "console.log('anything');")],
    );

    let result = &run_all(&root)[0].1;
    assert_eq!(
        result.outcome,
        Outcome::Fail,
        "bundle freshness must not turn a broken corpus entry into an unavailable build"
    );
    let message = &result
        .primary_diagnostic()
        .expect("a missing authored source records why")
        .message;
    assert!(message.contains("gone.js"), "{message}");
}

#[test]
fn a_stale_javascript_bundle_skips_instead_of_falling_back_to_its_source() {
    let root = corpus(
        "stale-javascript-bundle",
        r#"[[test]]
id = "stale"
source = "source.js"
bundle = "bundle.js"
expected_stdout = "source\n"
"#,
        &[
            ("source.js", "console.log('source');"),
            ("bundle.js", "console.log('bundle');"),
        ],
    );

    let result = &run_all(&root)[0].1;
    assert_eq!(
        result.outcome,
        Outcome::Skipped,
        "a runnable source extension must not turn an unverified generated artifact into fallback permission"
    );
    let diagnostic = result
        .primary_diagnostic()
        .expect("the rejected bundle records why");
    assert_eq!(diagnostic.phase, DiagnosticPhase::Harness);
    assert!(
        diagnostic.message.contains("stale"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn a_program_the_compiler_refuses_fails_with_the_refusal_as_its_reason() {
    // The construct here only has to be one the compiler still refuses. It has moved five times
    // as coverage grew, most recently off `with`. What is being tested is that a refusal is
    // reported as a refusal, never as a pass, so when a class static block lands, move it again.
    let root = corpus(
        "refused",
        &entry("refused", "refused.js", "unreachable\n"),
        &[(
            "refused.js",
            "class C { static { console.log('unreachable'); } }",
        )],
    );

    let result = &run_all(&root)[0].1;
    assert_eq!(result.outcome, Outcome::Fail);
    let diagnostic = result.primary_diagnostic().expect("a refusal records why");
    assert_eq!(diagnostic.phase, DiagnosticPhase::Compile);
    assert!(
        diagnostic.message.contains("unsupported"),
        "the refusal is preserved verbatim as a backlog entry: {}",
        diagnostic.message
    );
}

#[test]
fn a_malformed_manifest_stops_the_run_instead_of_yielding_no_tests() {
    // Both ways a manifest can be wrong, because both would otherwise fail the same silent way:
    // a corpus that reports nothing looks exactly like a corpus that has nothing in it.
    let malformed = [
        (
            "malformed-syntax",
            "[[test]]\nid = \"a\"\nsource = \"a.js\"\nexpected_stdout = 42\n",
            "manifest.toml:4",
            "must be a string",
        ),
        (
            "malformed-toml",
            "[[test]]\nid = \"a\nsource = \"a.js\"\n",
            "manifest.toml:2",
            "unterminated string",
        ),
        (
            "malformed-schema",
            "[[test]]\nid = \"a\"\nsource = \"a.js\"\n",
            "manifest.toml:1",
            "missing required key `expected_stdout`",
        ),
    ];

    for (name, manifest, location, needle) in malformed {
        let root = corpus(name, manifest, &[("a.js", "console.log('a');")]);
        let message = discover(&root)
            .expect_err("a manifest outside the subset is not silently skipped")
            .to_string();
        assert!(message.contains(location), "{message}");
        assert!(message.contains(needle), "{message}");
    }
}

#[test]
fn a_manifest_that_says_nothing_about_a_directory_does_not_claim_it() {
    let root = corpus(
        "frontmatter-fallback",
        &entry("elsewhere", "elsewhere.js", "x\n"),
        &[
            ("elsewhere.js", "console.log('x');"),
            (
                "nested/own.js",
                "/*---\ndescription: declares itself\nflags: [raw]\nfeatures: [kiln-smoke]\n---*/\nconsole.log('own');\n",
            ),
        ],
    );

    let nested = discover(&root.join("nested")).expect("the nested tree is discoverable");
    assert_eq!(nested.len(), 1);
    assert_eq!(
        nested[0].provenance(),
        Provenance::Frontmatter,
        "a manifest that declares nothing here must not suppress the frontmatter reader"
    );
    assert_eq!(nested[0].id(), "own.js");
    assert_eq!(nested[0].features(), ["kiln-smoke"]);
}

#[test]
fn reasons_group_across_a_corpus_and_keep_skips_apart_from_passes() {
    let root = corpus(
        "distribution",
        &format!(
            "{}{}{}{}",
            entry("passes", "passes.js", "ok\n"),
            entry("refused-one", "refused-one.js", "\n"),
            entry("refused-two", "refused-two.js", "\n"),
            entry("needs-bundle", "needs-bundle.ts", "\n"),
        ),
        &[
            ("passes.js", "console.log('ok');"),
            ("refused-one.js", "console.log(typeof 1);"),
            ("refused-two.js", "console.log(typeof 2);"),
            ("needs-bundle.ts", "console.log('never run');"),
        ],
    );

    let results = run_all(&root);
    let report =
        ReasonReport::from_results(results.iter().map(|(id, result)| (id.as_str(), result)));

    let groups = report.groups();
    assert_eq!(groups.len(), 2, "a pass contributes no reason");
    assert_eq!(groups[0].count(), 2);
    assert_eq!(groups[0].outcome, Outcome::Fail);
    assert_eq!(groups[0].tests, ["refused-one", "refused-two"]);
    assert_eq!(groups[1].outcome, Outcome::Skipped);
    assert_eq!(groups[1].tests, ["needs-bundle"]);
    assert!(
        groups[1].reason.contains("build.sh"),
        "{}",
        groups[1].reason
    );
}

/// The corpus this whole mechanism exists for.  Deliberately asserts that every probe is measured
/// and that none is silently dropped, and deliberately does not assert how many pass: that number
/// is the measurement, and freezing it here would make an interpreter improvement look like a
/// broken test.
#[test]
fn the_repositorys_guard_corpus_is_measured_rather_than_assumed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../conformance/guard");
    let results = run_all(&root);

    assert_eq!(results.len(), 20);
    assert!(
        results
            .iter()
            .all(|(_, result)| !matches!(result.outcome, Outcome::Skipped | Outcome::Unsupported)),
        "the probes are plain JavaScript: kiln is asked, and the answer is recorded"
    );
    for (id, result) in &results {
        if result.outcome != Outcome::Pass {
            assert!(
                result.primary_diagnostic().is_some(),
                "{id} failed without a reason, which is the one thing this must never do"
            );
        }
    }
}
