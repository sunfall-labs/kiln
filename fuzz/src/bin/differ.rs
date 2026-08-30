//! Run one JavaScript file through node, the interpreter and the native backend, and print what
//! each said.
//!
//! What you reach for when a soak reports a divergence: paste the generated program into a file,
//! cut it down, and re-run until it is one line. It is the same comparison the soak makes -- the
//! same `engines.rs` for the two kiln engines and the same `triage.rs` for the three-way verdict --
//! so a reduction that still reports a defect here is still the same finding.
//!
//! ```sh
//! cd fuzz && cargo run --release --bin differ -- findings/symbol-to-primitive-is-ignored.js
//! ```
//!
//! node is consulted whenever it is on `PATH`, and skipped with a line saying so when it is not.
//! The C toolchain is the same: a reduction is worth printing even when only two of the three
//! answers can be obtained, as long as the report says which two.

use kiln_fuzz::engines::{self, Cost, Interpreted, Native};
use kiln_fuzz::node::Node;
use kiln_fuzz::triage::{self, NativeAnswer};

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: differ <file.js>");
        std::process::exit(2);
    };
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("differ: cannot read {path}: {error}");
            std::process::exit(2);
        }
    };

    let program = match engines::lower(&source) {
        Ok(program) => program,
        Err(reason) => {
            println!("not-lowered: {reason}");
            std::process::exit(1);
        }
    };
    let interpreted = engines::interpret(&program);

    // The native side is optional and says so. `observe` rather than `compare`, because a
    // three-way report needs the bytes and not a verdict about them.
    let mut cost = Cost::default();
    let native = match Native::discover() {
        Err(reason) => {
            println!("no native answer: {reason}");
            NativeAnswer::NotAsked
        }
        Ok(builder) => match engines::observe(&builder, &program, "differ", &mut cost) {
            Err(outcome) => NativeAnswer::Refused(format!("{}: {outcome:?}", outcome.label())),
            Ok(run) if run.exit_code.is_none() => {
                NativeAnswer::Refused("killed by a signal".to_owned())
            }
            Ok(run) => {
                if run.exit_code != Some(0) {
                    println!(
                        "native stopped: exit {:?}, stderr {:?}",
                        run.exit_code,
                        run.stderr.lines().next().unwrap_or("(silence)")
                    );
                }
                NativeAnswer::Ran {
                    completed: run.exit_code == Some(0),
                    stdout: run.stdout,
                }
            }
        },
    };

    let node = match Node::discover() {
        Ok(node) => node,
        Err(reason) => {
            // Without node this is the two-engine comparison and nothing more, and saying so is
            // the point: a report that quietly dropped its reference would read like agreement.
            println!("SKIP: no reference implementation ({reason})");
            print_two_engines(&interpreted, &native);
            return;
        }
    };
    let run = match node.run(&source, "differ") {
        Ok(run) => run,
        Err(reason) => {
            println!("SKIP: node could not run this ({reason})");
            print_two_engines(&interpreted, &native);
            return;
        }
    };

    let verdict = triage::triage(&source, &run, &interpreted, &native);
    println!("{}", verdict.class.label());
    if !verdict.reason.is_empty() {
        println!("  {}", verdict.reason);
    }
    println!("--- node ---\n{}", verdict.node);
    if !verdict.node_completed {
        println!("<node stopped: {}>", verdict.node_diagnostic);
    }
    println!("--- interpreter ---\n{}", verdict.interpreter);
    match &verdict.native {
        Some(native) => println!("--- native ---\n{native}"),
        None => println!("--- native ---\n<no native answer was obtained>"),
    }
    if verdict.class.is_defect() {
        std::process::exit(1);
    }
}

fn print_two_engines(interpreted: &Interpreted, native: &NativeAnswer) {
    let printed = match interpreted {
        Interpreted::Completed(stdout) | Interpreted::Stopped { stdout, .. } => stdout.as_str(),
        Interpreted::OverBudget => "<over the screening budget>",
    };
    println!("--- interpreter ---\n{printed}");
    match native {
        NativeAnswer::Ran { stdout, .. } => println!("--- native ---\n{stdout}"),
        NativeAnswer::Refused(reason) => println!("--- native ---\n<refused: {reason}>"),
        NativeAnswer::NotAsked => println!("--- native ---\n<not asked>"),
    }
}
