//! `kiln-facts` - Extract, snapshot, and check whole-program fixpoint facts.
//!
//! Step 1.7 of `docs/architecture/fastest-native-plan.md`.
//! Snapshots inferred types, sites, immutable bindings, and closed-world verdicts
//! across benchmarks and thirdparty bundles into `bench/results/facts-baseline.json`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kiln_backend_c::facts::{FactsBaseline, ProgramFacts};
use kiln_module_graph::ModuleGraph;

struct OxcFrontend;

impl kiln_module_graph::Frontend for OxcFrontend {
    fn parse(&mut self, path: &str, source: &str) -> kiln_hir::Module {
        frontend_oxc::parse(path, source)
    }

    fn parse_commonjs(&mut self, path: &str, source: &str) -> kiln_module_graph::CommonJsParse {
        let (module, static_requires) = frontend_oxc::parse_commonjs_with_requires(path, source);
        kiln_module_graph::CommonJsParse {
            module,
            static_requires,
        }
    }
}

const BENCHMARKS: &[(&str, &str)] = &[
    ("collatz", "bench/cpu/collatz.mjs"),
    ("fib", "bench/cpu/fib.mjs"),
    ("frame", "bench/cpu/frame.mjs"),
    ("loop", "bench/cpu/loop.mjs"),
    ("mandelbrot", "bench/cpu/mandelbrot.mjs"),
    ("nbody", "bench/cpu/nbody.mjs"),
    ("array-functional", "bench/non-io/array-functional.mjs"),
    ("array-sort", "bench/non-io/array-sort.mjs"),
    ("class-poly", "bench/non-io/class-poly.mjs"),
    ("collections", "bench/non-io/collections.mjs"),
    ("json", "bench/non-io/json.mjs"),
    ("object-spread", "bench/non-io/object-spread.mjs"),
    ("regex", "bench/non-io/regex.mjs"),
    ("string-ops", "bench/non-io/string-ops.mjs"),
];

const BUNDLES: &[(&str, &str, bool)] = &[
    ("acorn", "conformance/thirdparty/acorn.bundle.js", false),
    ("ajv", "conformance/thirdparty/ajv.bundle.js", false),
    (
        "async-concurrency",
        "conformance/thirdparty/async-concurrency.bundle.js",
        false,
    ),
    ("chalk", "conformance/thirdparty/chalk.bundle.js", false),
    ("datefns", "conformance/thirdparty/datefns.bundle.js", false),
    ("dayjs", "conformance/thirdparty/dayjs.bundle.js", false),
    ("immer", "conformance/thirdparty/immer.bundle.js", false),
    ("lodash", "conformance/thirdparty/lodash.bundle.js", false),
    ("marked", "conformance/thirdparty/marked.bundle.js", false),
    ("mobx", "conformance/thirdparty/mobx.bundle.js", false),
    ("ms", "conformance/thirdparty/ms.bundle.js", false),
    (
        "mustache",
        "conformance/thirdparty/mustache.bundle.js",
        false,
    ),
    (
        "postcss",
        "conformance/thirdparty/postcss.bundle.js",
        true, // needs Node platform resolver
    ),
    ("rxjs", "conformance/thirdparty/rxjs.bundle.js", false),
    ("semver", "conformance/thirdparty/semver.bundle.js", false),
    ("uuid", "conformance/thirdparty/uuid.bundle.js", false),
    (
        "vue-reactivity",
        "conformance/thirdparty/vue-reactivity.bundle.js",
        false,
    ),
];

fn repo_root() -> PathBuf {
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let p = PathBuf::from(manifest_dir).join("../..");
        if let Ok(canon) = p.canonicalize() {
            return canon;
        }
    }
    let mut current = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    while !current.join(".git").exists() {
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }
    current
}

fn analyze_target(abs_path: &Path, root: &Path, node: bool) -> Result<ProgramFacts, String> {
    let graph = if node {
        let resolver = kiln_module_resolver::NodeRuntimeResolver::new(root)
            .map_err(|e| format!("failed to create NodeRuntimeResolver: {e}"))?;
        ModuleGraph::build_node(
            abs_path,
            resolver,
            |p: &Path| std::fs::read_to_string(p),
            OxcFrontend,
        )
    } else {
        ModuleGraph::build(
            abs_path,
            root,
            |p: &Path| std::fs::read_to_string(p),
            OxcFrontend,
        )
    };

    match graph {
        Ok(graph) => {
            let mut facts = kiln_backend_c::facts::analyze_graph(&graph);
            facts.normalize_paths(root);
            Ok(facts)
        }
        Err(err) => Err(err.to_string()),
    }
}

fn print_usage() {
    eprintln!(
        r#"usage: kiln-facts [options] [targets...]

Options:
  --check               verify current facts against baseline (default)
  --update              regenerate baseline at bench/results/facts-baseline.json
  --baseline <path>     path to baseline JSON file
  --all                 run on all benchmarks and thirdparty bundles
  --node                force Node resolution policy for target(s)
  --explain             print human-readable fixpoint explanation
  --json                output facts as JSON
  -h, --help            print this help text
"#
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = repo_root();
    let mut default_baseline = root.join("bench/results/facts-baseline.json");

    let mut check_mode = false;
    let mut update_mode = false;
    let mut all_targets = false;
    let mut node_flag = false;
    let mut explain_mode = false;
    let mut json_mode = false;
    let mut target_args = Vec::new();

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--check" => check_mode = true,
            "--update" => update_mode = true,
            "--all" => all_targets = true,
            "--node" => node_flag = true,
            "--explain" => explain_mode = true,
            "--json" => json_mode = true,
            "--baseline" => {
                if let Some(val) = iter.next() {
                    default_baseline = PathBuf::from(val);
                } else {
                    eprintln!("kiln-facts: --baseline requires a path argument");
                    return ExitCode::FAILURE;
                }
            }
            "-h" | "--help" => {
                print_usage();
                return ExitCode::SUCCESS;
            }
            other if other.starts_with('-') => {
                eprintln!("kiln-facts: unknown option '{other}'");
                print_usage();
                return ExitCode::FAILURE;
            }
            other => target_args.push(other.to_string()),
        }
    }

    // Default to check mode if neither check nor update was specified and no single target was passed with explain/json
    if !update_mode && !explain_mode && !json_mode && target_args.is_empty() {
        check_mode = true;
        all_targets = true;
    }
    if update_mode || check_mode {
        all_targets = true;
    }

    if !all_targets && !target_args.is_empty() {
        // Single or explicit target analysis
        for target in &target_args {
            let path = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                root.join(target)
            };

            let is_node = node_flag || target.contains("postcss");
            match analyze_target(&path, &root, is_node) {
                Ok(facts) => {
                    if json_mode {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&facts).unwrap_or_default()
                        );
                    } else if explain_mode {
                        print!("{}", facts.format_explain());
                    } else {
                        let world_str = if facts.world.is_closed() {
                            "closed"
                        } else {
                            "open"
                        };
                        println!(
                            "{}: world={}, modules={}, funcs={}, sites={}",
                            target,
                            world_str,
                            facts.modules.len(),
                            facts.function_count(),
                            facts.site_count()
                        );
                    }
                }
                Err(err) => {
                    eprintln!("kiln-facts: failed to analyze {target}: {err}");
                    return ExitCode::FAILURE;
                }
            }
        }
        return ExitCode::SUCCESS;
    }

    // Full baseline mode (all benchmarks and bundles)
    let total_count = BENCHMARKS.len() + BUNDLES.len();
    let mut current_baseline = FactsBaseline::new();
    let mut index = 0;

    println!(
        "analyzing whole-program facts across {} targets ({} benchmarks, {} bundles)...",
        total_count,
        BENCHMARKS.len(),
        BUNDLES.len()
    );

    // 1. Benchmarks
    for (name, rel_path) in BENCHMARKS {
        index += 1;
        let abs_path = root.join(rel_path);
        if !abs_path.exists() {
            eprintln!("[{index:2}/{total_count}] benchmark:{name:<20} missing file {rel_path}");
            return ExitCode::FAILURE;
        }

        match analyze_target(&abs_path, &root, false) {
            Ok(facts) => {
                let world_str = if facts.world.is_closed() {
                    "closed"
                } else {
                    "open"
                };
                println!(
                    "[{index:2}/{total_count}] benchmark:{name:<20} {world_str:<7} {:2} mod(s), {:3} func(s), {:4} site(s)",
                    facts.modules.len(),
                    facts.function_count(),
                    facts.site_count()
                );
                current_baseline
                    .benchmarks
                    .insert((*name).to_string(), facts);
            }
            Err(err) => {
                eprintln!("[{index:2}/{total_count}] benchmark:{name:<20} FAILED: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    // 2. Bundles
    for (name, rel_path, is_node) in BUNDLES {
        index += 1;
        let abs_path = root.join(rel_path);
        if !abs_path.exists() {
            eprintln!("[{index:2}/{total_count}] bundle:{name:<23} missing file {rel_path}");
            return ExitCode::FAILURE;
        }

        match analyze_target(&abs_path, &root, *is_node) {
            Ok(facts) => {
                let world_str = if facts.world.is_closed() {
                    "closed"
                } else {
                    "open"
                };
                println!(
                    "[{index:2}/{total_count}] bundle:{name:<23} {world_str:<7} {:2} mod(s), {:3} func(s), {:4} site(s)",
                    facts.modules.len(),
                    facts.function_count(),
                    facts.site_count()
                );
                current_baseline.bundles.insert((*name).to_string(), facts);
            }
            Err(err) => {
                eprintln!("[{index:2}/{total_count}] bundle:{name:<23} FAILED: {err}");
                return ExitCode::FAILURE;
            }
        }
    }

    if update_mode {
        if let Some(parent) = default_baseline.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = current_baseline
            .to_json_string()
            .expect("serializes baseline");
        if let Err(err) = std::fs::write(&default_baseline, json) {
            eprintln!(
                "kiln-facts: failed to write baseline to {}: {err}",
                default_baseline.display()
            );
            return ExitCode::FAILURE;
        }
        println!(
            "\nwrote {}: {} benchmarks, {} bundles recorded",
            default_baseline.display(),
            current_baseline.benchmarks.len(),
            current_baseline.bundles.len()
        );
        return ExitCode::SUCCESS;
    }

    // Check mode
    if !default_baseline.exists() {
        eprintln!(
            "kiln-facts: baseline file {} does not exist; run with --update to create it",
            default_baseline.display()
        );
        return ExitCode::FAILURE;
    }

    let baseline_content = match std::fs::read_to_string(&default_baseline) {
        Ok(c) => c,
        Err(err) => {
            eprintln!(
                "kiln-facts: failed to read baseline {}: {err}",
                default_baseline.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let committed_baseline = match FactsBaseline::from_json_str(&baseline_content) {
        Ok(b) => b,
        Err(err) => {
            eprintln!(
                "kiln-facts: failed to parse baseline {}: {err}",
                default_baseline.display()
            );
            return ExitCode::FAILURE;
        }
    };

    let discrepancies = current_baseline.compare(&committed_baseline);

    if !discrepancies.is_empty() {
        eprintln!(
            "\n*** FACTS BASELINE REGRESSIONS / DIVERGENCES DETECTED ({} chunks) ***",
            discrepancies.len()
        );
        for (chunk, diffs) in &discrepancies {
            eprintln!("REGRESS  {chunk}:");
            for diff in diffs {
                eprintln!("    - {diff}");
            }
        }
        eprintln!(
            "\n{} chunk(s) diverged from baseline; accept them with --update if intended",
            discrepancies.len()
        );
        return ExitCode::FAILURE;
    }

    println!(
        "\nno regressions across {} target(s) ({} benchmarks, {} bundles)",
        total_count,
        current_baseline.benchmarks.len(),
        current_baseline.bundles.len()
    );
    ExitCode::SUCCESS
}
