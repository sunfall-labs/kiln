use std::path::{Path, PathBuf};
use std::process::Command;

use kiln_backend_c::facts::FactsBaseline;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root is reachable from crate manifest")
}

#[test]
fn committed_facts_baseline_is_valid_and_complete() {
    let root = repo_root();
    let baseline_path = root.join("bench/results/facts-baseline.json");
    assert!(
        baseline_path.exists(),
        "bench/results/facts-baseline.json must exist"
    );

    let content = std::fs::read_to_string(&baseline_path).expect("read facts-baseline.json");
    let baseline = FactsBaseline::from_json_str(&content).expect("valid facts baseline json");

    assert_eq!(baseline.schema, 1);

    // Benchmarks
    let expected_benchmarks = [
        "collatz",
        "fib",
        "frame",
        "loop",
        "mandelbrot",
        "nbody",
        "array-functional",
        "array-sort",
        "class-poly",
        "collections",
        "json",
        "object-spread",
        "regex",
        "string-ops",
    ];
    for name in &expected_benchmarks {
        assert!(
            baseline.benchmarks.contains_key(*name),
            "baseline missing benchmark {name}"
        );
        let facts = &baseline.benchmarks[*name];
        assert!(
            facts.world.is_closed(),
            "benchmark {name} should be closed world"
        );
        assert!(
            !facts.modules.is_empty(),
            "benchmark {name} must have modules"
        );
        assert!(
            facts.function_count() > 0,
            "benchmark {name} must have functions"
        );
        assert!(facts.site_count() > 0, "benchmark {name} must have sites");
    }

    // Bundles
    let expected_bundles = [
        "acorn",
        "ajv",
        "async-concurrency",
        "chalk",
        "datefns",
        "dayjs",
        "immer",
        "lodash",
        "marked",
        "mobx",
        "ms",
        "mustache",
        "postcss",
        "rxjs",
        "semver",
        "uuid",
        "vue-reactivity",
    ];
    for name in &expected_bundles {
        assert!(
            baseline.bundles.contains_key(*name),
            "baseline missing bundle {name}"
        );
        let facts = &baseline.bundles[*name];
        assert!(!facts.modules.is_empty(), "bundle {name} must have modules");
        assert!(
            facts.function_count() > 0,
            "bundle {name} must have functions"
        );
        assert!(facts.site_count() > 0, "bundle {name} must have sites");
    }

    // Known open-world bundles
    assert!(
        !baseline.bundles["ajv"].world.is_closed(),
        "ajv requires dynamic eval, must be open world"
    );
    assert!(
        !baseline.bundles["lodash"].world.is_closed(),
        "lodash requires dynamic code gen, must be open world"
    );
}

#[test]
fn facts_baseline_check_passes() {
    let root = repo_root();
    let kiln_facts_bin = env!("CARGO_BIN_EXE_kiln-facts");
    let output = Command::new(kiln_facts_bin)
        .arg("--check")
        .current_dir(&root)
        .output()
        .expect("run kiln-facts --check");

    assert!(
        output.status.success(),
        "kiln-facts --check failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
