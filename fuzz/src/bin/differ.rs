//! Run one JavaScript file through both engines and print what each said.
//!
//! What you reach for when the soak reports a divergence: paste the generated program into a
//! file, cut it down, and re-run. It is the same comparison `engines::compare` makes, so a
//! reduction that still reports `DIVERGED` here is still the same finding.
//!
//! ```sh
//! cd fuzz && cargo run --release --bin differ -- /tmp/case.js
//! ```

use kiln_fuzz::engines::{self, Native, Outcome};

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
    let native = match Native::discover() {
        Ok(native) => native,
        Err(reason) => {
            eprintln!("SKIP: {reason}");
            return;
        }
    };
    let outcome = engines::compare(&native, &source, "differ");
    match &outcome {
        Outcome::Diverged {
            interpreter,
            native,
            native_exit,
            native_stderr,
            ..
        } => {
            println!("DIVERGED");
            println!("--- interpreter ---\n{interpreter}");
            println!("--- native (exit {native_exit:?}) ---\n{native}");
            if !native_stderr.is_empty() {
                println!("--- native stderr ---\n{native_stderr}");
            }
            std::process::exit(1);
        }
        other => println!("{}: {other:#?}", other.label()),
    }
}
