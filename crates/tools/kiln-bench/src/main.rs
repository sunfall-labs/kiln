//! kiln-bench: the timing harness for design.md §13.
//!
//! Item 1 of the implementation order, deliberately ahead of the compiler. Iteration speed is
//! an architectural constraint of this project, and an unmeasured constraint is a wish - so the
//! harness exists before there is much to measure, and the compiler grows underneath it.

mod report;
mod scenarios;
mod timing;
mod workload;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use report::{Report, WorkloadInfo};
use scenarios::Budget;
use workload::{ALL, Workload, WorkloadSpec};

const USAGE: &str = "\
kiln-bench - iteration-speed harness (design.md §13)

usage:
  kiln-bench [options]        measure, report, and optionally gate on a baseline
  kiln-bench gen [options]    only (re)generate the reference workloads

options:
  --workload <name|all>   which reference workload to run (default: all)
  --examples-dir <dir>    where the workloads are materialised (default: <repo>/examples)
  --kiln <file>           CLI binary used for daemon startup (default: sibling `kiln` binary)
  --regenerate            rewrite the workloads even if they are already up to date
  --warmup <n>            override the discarded warmup iterations of every scenario
  --iterations <n>        override the measured iterations of every scenario
  --json                  also print the machine-readable report to stdout
  --out <file>            where to write the JSON report (default: <repo>/bench/results/latest.json)
  --compare <file>        compare against a baseline report and fail on a regression
  --threshold <percent>   wall-clock regression threshold for --compare (default: 15)
  -h, --help              this text

exit status:
  0  measured cleanly
  1  --compare found a regression
  2  the harness could not run
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = match Args::parse(&args) {
        Ok(Some(args)) => args,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => {
            eprintln!("kiln-bench: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    match run(&args) {
        Ok(Outcome::Clean) => ExitCode::SUCCESS,
        Ok(Outcome::Regressed(n)) => {
            eprintln!("kiln-bench: {n} regression(s) against the baseline");
            ExitCode::from(1)
        }
        Err(message) => {
            eprintln!("kiln-bench: {message}");
            ExitCode::from(2)
        }
    }
}

enum Outcome {
    Clean,
    Regressed(usize),
}

fn run(args: &Args) -> Result<Outcome, String> {
    let workloads: Vec<Workload> = args.specs.iter().map(|s| Workload::generate(s)).collect();
    for w in &workloads {
        let dir = args.examples_dir.join(w.spec.name);
        if w.materialise(&args.examples_dir, args.regenerate)? {
            eprintln!(
                "kiln-bench: wrote workload \"{}\" to {}",
                w.spec.name,
                dir.display()
            );
        } else if args.generate_only {
            // Silent during a measurement run - it is the normal case and says nothing useful -
            // but `gen` that printed nothing at all would look like it had failed.
            eprintln!(
                "kiln-bench: workload \"{}\" already up to date",
                w.spec.name
            );
        }
    }
    if args.generate_only {
        return Ok(Outcome::Clean);
    }

    let mut report = Report::new(scenarios::WARM_PIPELINE);
    report
        .measurements
        .push(scenarios::daemon_startup(&args.kiln, args.budget)?);
    report
        .measurements
        .extend(scenarios::daemon_warm(args.budget)?);
    for w in &workloads {
        report.workloads.push(WorkloadInfo::of(w));
        report
            .measurements
            .extend(scenarios::run_all(w, args.budget, &args.examples_dir)?);
    }
    report
        .measurements
        .push(scenarios::targeted_conformance(args.budget)?);
    report
        .measurements
        .push(scenarios::targeted_node_net_conformance(args.budget)?);
    if let Some(native) = scenarios::native(args.budget)? {
        report.measurements.extend(native);
    }

    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("serialising the report: {e}"))?;
    if let Some(parent) = args.out.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    fs::write(&args.out, format!("{json}\n"))
        .map_err(|e| format!("writing {}: {e}", args.out.display()))?;

    if args.json {
        println!("{json}");
    } else {
        print!("{}", report::render_human(&report));
        println!("json report: {}", args.out.display());
    }

    let Some(baseline_path) = &args.compare else {
        return Ok(Outcome::Clean);
    };
    let baseline = load_baseline(baseline_path)?;
    let (text, regressions) = report::compare(&report, &baseline, args.threshold);
    if args.json {
        eprint!("{text}");
    } else {
        print!("\n{text}");
    }
    for r in &regressions {
        eprintln!("kiln-bench: regression in {}: {}", r.id, r.detail);
    }
    if regressions.is_empty() {
        Ok(Outcome::Clean)
    } else {
        Ok(Outcome::Regressed(regressions.len()))
    }
}

fn load_baseline(path: &Path) -> Result<Report, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let baseline: Report =
        serde_json::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    if baseline.schema != report::SCHEMA_VERSION {
        return Err(format!(
            "{} uses report schema {}, this build writes schema {}; re-record the baseline",
            path.display(),
            baseline.schema,
            report::SCHEMA_VERSION
        ));
    }
    Ok(baseline)
}

struct Args {
    generate_only: bool,
    regenerate: bool,
    json: bool,
    specs: Vec<&'static WorkloadSpec>,
    examples_dir: PathBuf,
    kiln: PathBuf,
    out: PathBuf,
    compare: Option<PathBuf>,
    /// Fractional, e.g. 0.15 for the default 15%.
    threshold: f64,
    budget: Budget,
}

impl Args {
    /// `Ok(None)` means help was requested.
    fn parse(argv: &[String]) -> Result<Option<Args>, String> {
        let root = repo_root();
        let mut args = Args {
            generate_only: false,
            regenerate: false,
            json: false,
            specs: ALL.to_vec(),
            examples_dir: root.join("examples"),
            kiln: sibling_kiln_binary()?,
            out: root.join("bench/results/latest.json"),
            compare: None,
            threshold: 0.15,
            budget: Budget::default(),
        };

        let mut it = argv.iter().enumerate();
        while let Some((i, arg)) = it.next() {
            match arg.as_str() {
                "gen" if i == 0 => args.generate_only = true,
                "--regenerate" => args.regenerate = true,
                "--json" => args.json = true,
                "-h" | "--help" => return Ok(None),
                "--workload" => {
                    let value = next(&mut it, arg)?;
                    args.specs = if value == "all" {
                        ALL.to_vec()
                    } else {
                        vec![workload::spec_by_name(value).ok_or_else(|| {
                            format!("unknown workload {value:?}; try one of: all, small, medium")
                        })?]
                    };
                }
                "--examples-dir" => args.examples_dir = PathBuf::from(next(&mut it, arg)?),
                "--kiln" => args.kiln = PathBuf::from(next(&mut it, arg)?),
                "--out" => args.out = PathBuf::from(next(&mut it, arg)?),
                "--compare" => args.compare = Some(PathBuf::from(next(&mut it, arg)?)),
                "--threshold" => args.threshold = number(next(&mut it, arg)?, arg)? / 100.0,
                "--warmup" => args.budget.warmup = Some(count(next(&mut it, arg)?, arg)?),
                "--iterations" => args.budget.iterations = Some(count(next(&mut it, arg)?, arg)?),
                other => return Err(format!("unrecognised argument {other:?}")),
            }
        }
        Ok(Some(args))
    }
}

fn next<'a>(
    it: &mut impl Iterator<Item = (usize, &'a String)>,
    flag: &str,
) -> Result<&'a str, String> {
    it.next()
        .map(|(_, v)| v.as_str())
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn number(value: &str, flag: &str) -> Result<f64, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects a number, got {value:?}"))
}

fn count(value: &str, flag: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects a whole number, got {value:?}"))
}

/// The workspace root, resolved at compile time so the harness behaves the same however it is
/// launched. Defaults that point into the repo are only defaults; `--out` and `--examples-dir`
/// override them.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

/// The performance scripts build `kiln-bench` and `kiln` into the same Cargo profile directory.
/// Cargo test binaries live one level deeper under `deps`, which keeps the same derivation useful
/// for unit tests that explicitly build the CLI first.
fn sibling_kiln_binary() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("locating the benchmark executable: {error}"))?;
    let mut directory = executable.parent().ok_or_else(|| {
        format!(
            "benchmark executable has no parent: {}",
            executable.display()
        )
    })?;
    if directory.file_name().is_some_and(|name| name == "deps") {
        directory = directory.parent().ok_or_else(|| {
            format!(
                "benchmark dependency directory has no parent: {}",
                executable.display()
            )
        })?;
    }
    Ok(directory.join(format!("kiln{}", std::env::consts::EXE_SUFFIX)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Result<Option<Args>, String> {
        let owned: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        Args::parse(&owned)
    }

    #[test]
    fn defaults_run_every_workload_and_write_into_the_repo() {
        let args = parse(&[]).expect("parse").expect("not help");
        assert_eq!(args.specs.len(), ALL.len());
        assert!(args.out.ends_with("bench/results/latest.json"));
        assert!(args.examples_dir.ends_with("examples"));
        assert_eq!(
            args.kiln.file_name().and_then(|name| name.to_str()),
            Some(if cfg!(windows) { "kiln.exe" } else { "kiln" })
        );
        assert!(!args.generate_only);
    }

    #[test]
    fn threshold_is_given_in_percent_and_stored_as_a_fraction() {
        let args = parse(&["--threshold", "25"])
            .expect("parse")
            .expect("not help");
        assert!((args.threshold - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn a_single_workload_can_be_selected_by_name() {
        let args = parse(&["--workload", "medium"])
            .expect("parse")
            .expect("not help");
        assert_eq!(args.specs.len(), 1);
        assert_eq!(args.specs[0].name, "medium");
    }

    #[test]
    fn an_explicit_cli_binary_overrides_the_sibling_default() {
        let args = parse(&["--kiln", "/tmp/custom-kiln"])
            .expect("parse")
            .expect("not help");
        assert_eq!(args.kiln, Path::new("/tmp/custom-kiln"));
    }

    #[test]
    fn gen_is_only_a_subcommand_in_first_position() {
        assert!(
            parse(&["gen"])
                .expect("parse")
                .expect("not help")
                .generate_only
        );
        assert!(parse(&["--json", "gen"]).is_err());
    }

    #[test]
    fn bad_input_is_rejected_rather_than_defaulted() {
        assert!(parse(&["--workload", "enormous"]).is_err());
        assert!(parse(&["--iterations"]).is_err());
        assert!(parse(&["--iterations", "lots"]).is_err());
        assert!(parse(&["--kiln"]).is_err());
        assert!(parse(&["--nope"]).is_err());
    }

    #[test]
    fn help_short_circuits() {
        assert!(parse(&["--help"]).expect("parse").is_none());
    }
}
