//! Does the generator actually generate anything?
//!
//! A fuzz target that passes because every case was empty is the same hollow green
//! `conformance/README.md` refuses elsewhere: "true arithmetic and a false claim when nothing
//! executed". `arbitrary` gives a `Vec` whatever length the driver's remaining bytes allow, and
//! how many bytes that is depends on the engine -- so it is a property of the setup, not of the
//! type, and it has to be measured rather than assumed.

use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// `AtomicUsize` rather than `Cell`, and not because anything here is concurrent: a bolero
/// closure runs inside `catch_unwind`, so everything it captures must be `RefUnwindSafe`, and
/// `Cell` is not.
type Counter = AtomicUsize;

use kiln_fuzz::ir::IrProgram;
use kiln_fuzz::js::JsProgram;
use kiln_fuzz::oracle::History;

#[test]
fn generated_histories_are_not_mostly_empty() {
    let total = Counter::new(0);
    let steps = Counter::new(0);
    let longest = Counter::new(0);
    bolero::check!()
        .with_arbitrary::<kiln_fuzz::Roomy<History>>()
        .with_iterations(2_000)
        .for_each(|kiln_fuzz::Roomy(history): &kiln_fuzz::Roomy<History>| {
            total.fetch_add(1, Relaxed);
            steps.fetch_add(history.0.len(), Relaxed);
            longest.fetch_max(history.0.len(), Relaxed);
        });
    let mean = steps.load(Relaxed) as f64 / total.load(Relaxed).max(1) as f64;
    eprintln!(
        "history: {} cases, mean {mean:.1} ops, longest {}",
        total.load(Relaxed),
        longest.load(Relaxed)
    );
    assert!(
        mean >= 2.0,
        "histories average {mean:.1} steps, which is not a history"
    );
}

#[test]
fn generated_programs_are_not_mostly_empty() {
    let total = Counter::new(0);
    let bytes = Counter::new(0);
    let longest = Counter::new(0);
    bolero::check!()
        .with_arbitrary::<kiln_fuzz::Roomy<JsProgram>>()
        .with_iterations(2_000)
        .for_each(|kiln_fuzz::Roomy(program): &kiln_fuzz::Roomy<JsProgram>| {
            let source = program.render();
            total.fetch_add(1, Relaxed);
            bytes.fetch_add(source.len(), Relaxed);
            longest.fetch_max(source.len(), Relaxed);
            if std::env::var_os("KILN_FUZZ_SAMPLES").is_some() && total.load(Relaxed) <= 3 {
                eprintln!("--- sample ---\n{source}");
            }
        });
    let mean = bytes.load(Relaxed) as f64 / total.load(Relaxed).max(1) as f64;
    eprintln!(
        "js: {} cases, mean {mean:.0} bytes, longest {}",
        total.load(Relaxed),
        longest.load(Relaxed)
    );
    // The fixed preamble and epilogue are about 150 bytes on their own, so anything near that
    // means the generated half is empty.
    assert!(mean >= 450.0, "generated programs average {mean:.0} bytes");
}

#[test]
fn generated_bytecode_is_not_mostly_empty() {
    let total = Counter::new(0);
    let instructions = Counter::new(0);
    let longest = Counter::new(0);
    bolero::check!()
        .with_arbitrary::<kiln_fuzz::Roomy<IrProgram>>()
        .with_iterations(2_000)
        .for_each(
            |kiln_fuzz::Roomy(generated): &kiln_fuzz::Roomy<IrProgram>| {
                let (well_formed, _) = generated.build();
                total.fetch_add(1, Relaxed);
                instructions.fetch_add(well_formed.code.len(), Relaxed);
                longest.fetch_max(well_formed.code.len(), Relaxed);
            },
        );
    let mean = instructions.load(Relaxed) as f64 / total.load(Relaxed).max(1) as f64;
    eprintln!(
        "ir: {} cases, mean {mean:.1} instructions, longest {}",
        total.load(Relaxed),
        longest.load(Relaxed)
    );
    // Two instructions is `Const; Return`, the empty program.
    assert!(
        mean >= 14.0,
        "generated bodies average {mean:.1} instructions"
    );
}
