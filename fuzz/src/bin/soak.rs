//! A long-running differential soak, and the instrument that measured this crate's own cost.
//!
//! `cargo bolero` and `cargo test` both want a bounded, quiet run. The native target is neither:
//! it is one C compilation per case, so what it wants is to be pointed at a machine and left
//! there. This binary is that mode. It is also how `README.md`'s per-iteration figures were
//! produced -- they are measured here, not estimated.
//!
//! Generation is `arbitrary` over a seeded xorshift stream rather than a bolero engine, for one
//! reason: at ~200 ms per case, coverage feedback has nothing to work with. A libfuzzer engine
//! learns from thousands of executions per second; at five per second it is a slower random
//! search. Coverage guidance is worth having on the microsecond targets and is honestly not
//! worth having on this one -- see README.md, "Is bolero the right tool here?".

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use arbitrary::{Arbitrary, Unstructured};
use kiln_fuzz::engines::{self, Cost, Interpreted, Native, Outcome};
use kiln_fuzz::js::JsProgram;
use kiln_fuzz::node::{self, Node, NodeRun, NodeStatus};
use kiln_fuzz::triage::{self, NativeAnswer, Verdict};
use kiln_ir::Program;

/// Entropy per case, matched to bolero's own default driver length so a soak and a `cargo test`
/// run generate the same size of program. Measured: at 512 bytes the generator ran out part way
/// through and every program ended with the same four constant assignments.
const ENTROPY: usize = 4096;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*. Not cryptographic, and it does not need to be: its only job is to make a
        // run reproducible from its seed.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(8) {
            let value = self.next().to_le_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
    }
}

/// Which references a run consults, and what it is willing to pay for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The original: the interpreter against the native backend, with no third opinion. Finds the
    /// class where the two engines disagree, and is silent on the class where they agree and are
    /// both wrong -- which is the class `src/triage.rs` exists for.
    TwoEngines,
    /// All three, every case. The most thorough and the most expensive: a C compilation is paid
    /// for whether or not anything suggested it was worth paying for.
    ThreeWay,
    /// node and the interpreter on every case, and a native build **only** where the two already
    /// disagree.
    ///
    /// This is the cost argument, and the run prints the numbers rather than asserting them. The
    /// native path is ~446 ms per case and the node-plus-interpreter screen is about a
    /// millisecond, so the screen buys two orders of magnitude more programs per hour. What it
    /// gives up is the interpreter-versus-native comparison on cases where nothing looked wrong --
    /// which is exactly what `Mode::TwoEngines` already runs, and runs better.
    NodeScreen,
}

fn main() {
    let mut cases = 200_u64;
    let mut seed = 1_u64;
    let mut lower_only = false;
    let mut interpret_only = false;
    let mut mode = Mode::TwoEngines;
    let mut findings_directory: Option<std::path::PathBuf> = None;
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--cases" => {
                cases = arguments
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(cases)
            }
            "--seed" => {
                seed = arguments
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(seed)
            }
            // node as a third reference. `src/triage.rs` says why two engines agreeing is not
            // evidence; `Mode` above says what each of these costs.
            "--node" => mode = Mode::ThreeWay,
            "--node-screen" => mode = Mode::NodeScreen,
            // Where to write one whole generated program per distinct finding, as a starting
            // point for reducing it by hand.
            "--findings" => findings_directory = arguments.next().map(std::path::PathBuf::from),
            // Skips the native half entirely. Useful for measuring how much of the generated
            // corpus even reaches the comparison, at ten thousand cases a second.
            "--lower-only" => lower_only = true,
            // Everything except the C toolchain. Ten thousand cases a second, which is what
            // makes it possible to tune the generator's completion rate at all -- at 800 ms a
            // case the feedback loop is too slow to tune anything.
            "--interpret-only" => interpret_only = true,
            other => {
                eprintln!("soak: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    if mode != Mode::TwoEngines && !lower_only && !interpret_only {
        soak_with_node(cases, seed, mode, findings_directory.as_deref());
        return;
    }

    let native = if lower_only || interpret_only {
        None
    } else {
        match Native::discover() {
            Ok(native) => {
                println!("toolchain: {}", native.describe());
                Some(native)
            }
            Err(reason) => {
                // A missing C toolchain is a skip, not a failure. The one dependency this check
                // cannot fake is the platform compiler, and pretending to have run it would be
                // worse than saying it did not -- the same rule `scripts/differential.sh`
                // applies to a missing `node`.
                println!("SKIP: {reason}");
                return;
            }
        }
    };

    let mut rng = Rng(seed | 1);
    let mut bytes = vec![0_u8; ENTROPY];
    let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
    // The reason distribution, which the conformance README argues is the more useful half of
    // any such run: "the implementation backlog ordered by how many tests each entry unblocks".
    let mut reasons: BTreeMap<String, u64> = BTreeMap::new();
    let mut defects: Vec<Outcome> = Vec::new();
    let mut compared = 0_u64;
    let mut cost = Cost::default();
    let started = Instant::now();
    let mut in_comparison = Duration::ZERO;

    for case in 0..cases {
        rng.fill(&mut bytes);
        let Ok(program) = JsProgram::arbitrary_take_rest(Unstructured::new(&bytes)) else {
            *tally.entry("no-entropy").or_default() += 1;
            continue;
        };
        let source = program.render();

        let outcome = match &native {
            None if interpret_only => match engines::lower(&source) {
                Err(reason) => Outcome::NotLowered(reason),
                Ok(program) => match engines::interpret(&program) {
                    engines::Interpreted::Completed(_) => Outcome::Match,
                    engines::Interpreted::Stopped { reason, .. } => {
                        Outcome::InterpreterStopped(reason)
                    }
                    engines::Interpreted::OverBudget => Outcome::TooExpensive,
                },
            },
            None => match engines::lower(&source) {
                Ok(_) => Outcome::Match,
                Err(reason) => {
                    if std::env::var_os("KILN_FUZZ_DUMP").is_some_and(|filter| {
                        reason.contains(&filter.to_string_lossy().into_owned())
                    }) {
                        println!("--- case {case}: {reason}\n{source}---");
                    }
                    Outcome::NotLowered(reason)
                }
            },
            Some(native) => {
                let at = Instant::now();
                let outcome = engines::compare_recording(native, &source, "soak", &mut cost);
                // Only cases that actually reached the C toolchain belong in the per-iteration
                // average. Averaging in the ones that were refused before emission would report
                // a cost nobody pays.
                if !matches!(
                    outcome,
                    Outcome::NotLowered(_) | Outcome::TooExpensive | Outcome::Unavailable(_)
                ) {
                    in_comparison += at.elapsed();
                    compared += 1;
                }
                outcome
            }
        };

        *tally.entry(outcome.label()).or_default() += 1;
        if let Some(reason) = outcome.reason() {
            *reasons
                .entry(format!("{:<20} {reason}", outcome.label()))
                .or_default() += 1;
        }
        if outcome.is_defect() {
            eprintln!("\n=== DEFECT at case {case} (seed {seed}) ===\n{source}\n{outcome:#?}\n");
            defects.push(outcome);
        }
    }

    let elapsed = started.elapsed();
    println!("\n{cases} cases in {elapsed:.2?}");
    if cost.cases > 0 {
        println!("{}", cost.report());
    }
    if compared > 0 {
        println!(
            "{compared} reached the native pipeline, {:.1?} each",
            in_comparison / u32::try_from(compared).unwrap_or(u32::MAX)
        );
    }
    for (label, count) in &tally {
        println!("  {count:>6}  {label}");
    }
    if !reasons.is_empty() {
        let mut ranked: Vec<_> = reasons.iter().collect();
        ranked.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
        println!("\nwhy (the backlog, ordered by how many cases each entry unblocks):");
        for (reason, count) in ranked.iter().take(25) {
            println!("  {count:>6}  {reason}");
        }
    }
    if !defects.is_empty() {
        println!("\n{} DEFECTS -- see above.", defects.len());
        std::process::exit(1);
    }
}

/// The three-way soak: node, the interpreter, and -- depending on `mode` -- the native backend.
///
/// Chunked rather than one case at a time for one reason: node costs ~30 ms per process and ~1 ms
/// per program inside one process, and at 30 ms a case node would *be* the run. See
/// `src/node_batch.js` for how that is done without giving up node's own `console.log` formatting,
/// which is the thing that would make a faster reference worthless.
///
/// The interpreter still goes first, and that ordering is a safety property rather than a
/// preference: its step budget is what bounds everything downstream, and a program it could not
/// finish is a program neither node nor a native binary should be handed.
fn soak_with_node(cases: u64, seed: u64, mode: Mode, findings_directory: Option<&std::path::Path>) {
    let node = match Node::discover() {
        Ok(node) => {
            println!("reference: {}", node.describe());
            node
        }
        Err(reason) => {
            // A missing reference implementation is a skip and never a pass, for the reason
            // `scripts/differential.sh` gives: it is the one dependency this cannot fake.
            println!("SKIP: {reason}");
            return;
        }
    };
    let native = match Native::discover() {
        Ok(native) => {
            println!("toolchain: {}", native.describe());
            Some(native)
        }
        Err(reason) => {
            println!("no C toolchain ({reason}); node and the interpreter only");
            None
        }
    };

    let mut rng = Rng(seed | 1);
    let mut bytes = vec![0_u8; ENTROPY];
    let mut tally: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut reasons: BTreeMap<String, u64> = BTreeMap::new();
    // One entry per distinct finding, not one per case. A soak of twenty thousand programs reaches
    // one root cause several thousand times, and several thousand copies of one bug is a report
    // nobody reads -- so the count is kept and the cases are not.
    let mut findings: BTreeMap<String, (u64, Verdict)> = BTreeMap::new();
    let mut cost = Cost::default();
    let mut node_time = Duration::ZERO;
    let mut node_cases = 0_u64;
    let mut native_builds = 0_u64;
    let started = Instant::now();

    let mut case = 0_u64;
    while case < cases {
        let chunk = node::BATCH.min(usize::try_from(cases - case).unwrap_or(node::BATCH));
        case += u64::try_from(chunk).unwrap_or(0);

        // 1. Generate, lower, and screen on the cheap engine.
        let mut screened: Vec<(String, Program, Interpreted)> = Vec::with_capacity(chunk);
        for _ in 0..chunk {
            rng.fill(&mut bytes);
            let Ok(generated) = JsProgram::arbitrary_take_rest(Unstructured::new(&bytes)) else {
                *tally.entry("no-entropy").or_default() += 1;
                continue;
            };
            let source = generated.render();
            match engines::lower(&source) {
                Err(reason) => {
                    *tally.entry("not-lowered").or_default() += 1;
                    let reason = reason.split('`').next().unwrap_or(&reason).trim_end();
                    *reasons
                        .entry(format!("{:<22} {reason}", "not-lowered"))
                        .or_default() += 1;
                }
                Ok(program) => {
                    let interpreted = engines::interpret(&program);
                    screened.push((source, program, interpreted));
                }
            }
        }

        // 2. One node process for the whole chunk.
        let sources: Vec<String> = screened.iter().map(|(source, ..)| source.clone()).collect();
        let at = Instant::now();
        let runs = match node.run_batch(&sources) {
            Ok(runs) => runs,
            Err(reason) => {
                eprintln!("soak: node is unusable: {reason}");
                return;
            }
        };
        node_time += at.elapsed();
        node_cases += u64::try_from(runs.len()).unwrap_or(0);

        // 3. Judge, paying for a native build where the mode says it is worth paying.
        for ((source, program, interpreted), run) in screened.iter().zip(&runs) {
            let worth_a_build = match mode {
                Mode::ThreeWay => true,
                // The escalation. A case where node and the interpreter already agree has nothing
                // for the native side to settle that a plain `--cases N` run would not settle more
                // cheaply, so it does not get a C compilation.
                Mode::NodeScreen => disagrees(run, interpreted),
                Mode::TwoEngines => false,
            };
            let answer = match (&native, worth_a_build) {
                (Some(builder), true) => {
                    native_builds += 1;
                    native_answer(builder, program, &mut cost)
                }
                _ => NativeAnswer::NotAsked,
            };

            let verdict = triage::triage(source, run, interpreted, &answer);
            *tally.entry(verdict.class.label()).or_default() += 1;
            if !verdict.reason.is_empty() {
                *reasons
                    .entry(format!("{:<22} {}", verdict.class.label(), verdict.reason))
                    .or_default() += 1;
            }
            if verdict.class.is_defect() || verdict.class.is_generator_bug() {
                findings
                    .entry(verdict.signature())
                    .or_insert_with(|| (0, verdict.clone()))
                    .0 += 1;
            }
        }
    }

    report(&Run {
        elapsed: started.elapsed(),
        cases,
        tally,
        reasons,
        findings,
        cost,
        node_time,
        node_cases,
        native_builds,
        findings_directory,
    });
}

/// Whether node and the interpreter already disagree, which is what earns a case a C compilation
/// in [`Mode::NodeScreen`].
///
/// Deliberately the same rule `triage.rs` judges by, and deliberately *permissive*: it decides
/// what to spend money on, so a false yes costs 446 ms and a false no loses a finding.
fn disagrees(run: &NodeRun, interpreted: &Interpreted) -> bool {
    if !matches!(run.status, NodeStatus::Ok | NodeStatus::Threw) {
        return false;
    }
    match interpreted {
        Interpreted::Completed(stdout) => !run.completed() || *stdout != run.stdout,
        Interpreted::Stopped { stdout, .. } => !run.stdout.starts_with(stdout.as_str()),
        Interpreted::OverBudget => false,
    }
}

/// Build and run one program natively, as an answer rather than as a verdict.
///
/// `engines::observe` and not `engines::compare`: the two-engine comparison collapses a match into
/// "the same bytes as the interpreter" and does not keep them, and a third reference needs the
/// bytes themselves. Claiming `BOTH-ENGINES-vs-NODE` on a native output nobody kept would be the
/// harness inventing its own headline.
fn native_answer(builder: &Native, program: &Program, cost: &mut Cost) -> NativeAnswer {
    match engines::observe(builder, program, "soak-node", cost) {
        Err(Outcome::NativeRefused(reason)) => NativeAnswer::Refused(reason),
        Err(other) => NativeAnswer::Refused(format!("{}: {other:?}", other.label())),
        // A signal has no exit code, and a binary that died is not a binary that answered.
        Ok(run) if run.exit_code.is_none() => {
            NativeAnswer::Refused("killed by a signal".to_owned())
        }
        Ok(run) => NativeAnswer::Ran {
            completed: run.exit_code == Some(0),
            stdout: run.stdout,
        },
    }
}

/// Everything one run measured, so the report is one argument and not eleven.
struct Run<'a> {
    elapsed: Duration,
    cases: u64,
    tally: BTreeMap<&'static str, u64>,
    reasons: BTreeMap<String, u64>,
    findings: BTreeMap<String, (u64, Verdict)>,
    cost: Cost,
    node_time: Duration,
    node_cases: u64,
    native_builds: u64,
    findings_directory: Option<&'a std::path::Path>,
}

fn report(run: &Run<'_>) {
    let per = |total: Duration, count: u64| total / u32::try_from(count.max(1)).unwrap_or(u32::MAX);
    println!("\n{} cases in {:.2?}", run.cases, run.elapsed);
    if run.node_cases > 0 {
        println!(
            "node:    {} programs, {:.2?} each",
            run.node_cases,
            per(run.node_time, run.node_cases)
        );
    }
    if run.native_builds > 0 {
        println!(
            "native:  {} builds -- {}",
            run.native_builds,
            run.cost.report()
        );
    }
    println!(
        "overall: {:.2?} per generated case",
        per(run.elapsed, run.cases)
    );
    for (label, count) in &run.tally {
        println!("  {count:>7}  {label}");
    }
    if !run.reasons.is_empty() {
        let mut ranked: Vec<_> = run.reasons.iter().collect();
        ranked.sort_by(|left, right| right.1.cmp(left.1).then(left.0.cmp(right.0)));
        println!("\nwhy (the backlog, ordered by how many cases each entry unblocks):");
        for (reason, count) in ranked.iter().take(25) {
            println!("  {count:>7}  {reason}");
        }
    }

    if run.findings.is_empty() {
        return;
    }
    println!("\n{} DISTINCT FINDINGS:", run.findings.len());
    let mut ranked: Vec<_> = run.findings.values().collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (index, (count, verdict)) in ranked.iter().enumerate() {
        println!("\n--- {} x{count}", verdict.signature());
        println!("  node        {}", excerpt(&verdict.node));
        println!("  interpreter {}", excerpt(&verdict.interpreter));
        match &verdict.native {
            Some(native) => println!("  native      {}", excerpt(native)),
            None => println!("  native      (no native binary was built for this case)"),
        }
        if let Some(directory) = run.findings_directory {
            // A whole generated program, which is a *lead* and not a finding: `findings/` holds
            // reduced cases, and an unreduced one has not been shown to be about anything yet.
            let _ = std::fs::create_dir_all(directory);
            let path = directory.join(format!("lead-{index:03}.js"));
            if std::fs::write(&path, &verdict.source).is_ok() {
                println!("  lead        {}", path.display());
            }
        }
    }
    if ranked.iter().any(|(_, verdict)| verdict.class.is_defect()) {
        std::process::exit(1);
    }
}

/// A multi-line output on one line, elided, so a report with one entry per finding stays readable.
fn excerpt(text: &str) -> String {
    let flattened = text.replace('\n', "\\n");
    let mut end = 160.min(flattened.len());
    while !flattened.is_char_boundary(end) {
        end -= 1;
    }
    if end == flattened.len() {
        format!("{flattened:?}")
    } else {
        format!("{:?}...", &flattened[..end])
    }
}
