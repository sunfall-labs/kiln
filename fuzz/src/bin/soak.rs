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
use kiln_fuzz::engines::{self, Cost, Native, Outcome};
use kiln_fuzz::js::JsProgram;

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

fn main() {
    let mut cases = 200_u64;
    let mut seed = 1_u64;
    let mut lower_only = false;
    let mut interpret_only = false;
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
