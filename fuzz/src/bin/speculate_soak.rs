//! Differential fuzzing of the optimizer across Node, Kiln interpreter,
//! and Kiln native under KILN_SPECULATE=0, 1, and exit.

use kiln_fuzz::engines::Native;
use kiln_fuzz::node::Node;
use kiln_fuzz::speculate_runner::{SpeculateVerdict, compare_all_modes};
use kiln_fuzz::typed_js::TypedProgram;
use std::time::Instant;

fn main() {
    let mut cases = 20_usize;
    let mut seed = 42_u64;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cases" => {
                if let Some(n) = args.next().and_then(|v| v.parse().ok()) {
                    cases = n;
                }
            }
            "--seed" => {
                if let Some(s) = args.next().and_then(|v| v.parse().ok()) {
                    seed = s;
                }
            }
            "--help" | "-h" => {
                println!("Usage: speculate_soak [--cases N] [--seed S]");
                return;
            }
            _ => {}
        }
    }

    println!("speculate_soak: initializing native toolchain and node reference...");
    let native = match Native::discover() {
        Ok(n) => n,
        Err(err) => {
            eprintln!("SKIP: C toolchain not available ({err})");
            return;
        }
    };
    let node = match Node::discover() {
        Ok(n) => n,
        Err(err) => {
            eprintln!("SKIP: node not available ({err})");
            return;
        }
    };

    println!("speculate_soak: running {cases} cases starting from seed {seed}...");
    let started = Instant::now();
    let mut matches = 0;
    let mut skipped = 0;

    for i in 0..cases {
        let current_seed = seed.wrapping_add(i as u64);
        let source = TypedProgram::generate_seeded(current_seed);
        let tag = format!("case_{i}");

        let verdict = compare_all_modes(&native, &node, &source, &tag);
        match verdict {
            SpeculateVerdict::Match => {
                matches += 1;
                print!(".");
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            SpeculateVerdict::Diverged {
                source,
                node,
                interp,
                spec0,
                spec1,
                specexit,
                divergence_reason,
            } => {
                println!("\n\n[DIVERGENCE FOUND] case {i} (seed {current_seed}):");
                println!("Reason: {divergence_reason}");
                println!("--- Source ---\n{source}");
                println!("--- Node stdout ---\n{node}");
                println!("--- Interpreter stdout ---\n{interp}");
                println!("--- Native KILN_SPECULATE=0 stdout ---\n{spec0}");
                println!("--- Native KILN_SPECULATE=1 stdout ---\n{spec1}");
                println!("--- Native KILN_SPECULATE=exit stdout ---\n{specexit}");
                std::process::exit(1);
            }
            SpeculateVerdict::Crashed(err) => {
                println!("\n\n[CRASH/FAILURE] case {i} (seed {current_seed}): {err}");
                std::process::exit(1);
            }
            SpeculateVerdict::Refused(reason) => {
                skipped += 1;
                print!("s");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let _ = reason;
            }
            SpeculateVerdict::NotLowered(reason) => {
                skipped += 1;
                print!("n");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let _ = reason;
            }
        }
    }

    println!(
        "\nspeculate_soak: finished {cases} cases in {:.1}s: {matches} matched, {skipped} skipped/refused, 0 diverged.",
        started.elapsed().as_secs_f64()
    );
}
