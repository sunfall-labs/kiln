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

/// Do the coercion shapes actually appear, and often enough to be coverage?
///
/// THIS IS THE THIRD ENTRY IN THIS FILE'S REASON FOR EXISTING. The first two were `Vec: Arbitrary`
/// averaging one element and depth being truncated after it was paid for; both left every target
/// green while the corpus measured nothing. A grammar that *can* emit
/// `Number({valueOf: () => 42})` once in ten thousand programs is the same failure in a third
/// costume -- the variant is there, the test suite is green, and the shape is not being covered.
///
/// So the assertion is a *rate*, per shape, and it is deliberately per shape rather than in
/// aggregate: `Coercion` has ten variants and they fail differently, and one popular variant can
/// carry an aggregate number while nine others never appear at all.
///
/// The rates themselves are measured, not chosen. Observed over 4,000 programs: 3,785 carry at
/// least one coercion object, the least common of the ten shapes appears in 408 per thousand, the
/// least common of the twenty-four consumers in 165, and the least common single-argument builtin
/// in 312. The floors below sit at roughly a third of each, which leaves room for the generator to
/// be retuned without this becoming a test that fails whenever anything changes -- and no room at
/// all for a shape to quietly disappear.
#[test]
fn coercion_shapes_appear_often_enough_to_be_coverage() {
    use kiln_fuzz::js::{BuiltinCall, Census, Coercion, Sink};

    const CASES: usize = 4_000;
    /// Floors, per thousand programs. Three numbers rather than one because the three families
    /// are drawn at different rates by construction: a shape is one of ten, a consumer one of
    /// twenty-four, and a builtin competes with every other expression variant.
    const SHAPE_FLOOR: u64 = 130;
    const SINK_FLOOR: u64 = 50;
    const BUILTIN_FLOOR: u64 = 100;

    let total = Counter::new(0);
    let census = std::sync::Mutex::new(Census::default());
    let with_a_coercion = Counter::new(0);
    bolero::check!()
        .with_arbitrary::<kiln_fuzz::Roomy<JsProgram>>()
        .with_iterations(CASES)
        .for_each(|kiln_fuzz::Roomy(program): &kiln_fuzz::Roomy<JsProgram>| {
            let (_, one) = program.render_counted();
            total.fetch_add(1, Relaxed);
            if one.coercions.iter().sum::<u32>() > 0 {
                with_a_coercion.fetch_add(1, Relaxed);
            }
            census.lock().expect("census").absorb(&one);
        });

    let total = total.load(Relaxed) as u64;
    let census = census.lock().expect("census").clone();
    let per_thousand = |count: u32| u64::from(count) * 1_000 / total.max(1);

    eprintln!(
        "coercion: {total} programs, {} carry at least one coercion object",
        with_a_coercion.load(Relaxed)
    );
    for (shape, count) in Coercion::ALL.iter().zip(census.coercions) {
        eprintln!("  {:>6}/1000  {}", per_thousand(count), shape.name());
    }
    for (sink, count) in Sink::ALL.iter().zip(census.sinks) {
        eprintln!(
            "  {:>6}/1000  consumed by {}",
            per_thousand(count),
            sink.name()
        );
    }
    for (call, count) in BuiltinCall::ALL.iter().zip(census.builtins) {
        eprintln!("  {:>6}/1000  {}(e)", per_thousand(count), call.name());
    }

    // A coercion object in at least a third of programs. Below that the shape is present but the
    // corpus is still mostly the corpus that missed `Math.abs({valueOf: () => 42})`.
    let carrying = with_a_coercion.load(Relaxed) as u64 * 1_000 / total.max(1);
    assert!(
        carrying >= 300,
        "only {carrying}/1000 programs contain a coercion object"
    );

    for (shape, count) in Coercion::ALL.iter().zip(census.coercions) {
        assert!(
            per_thousand(count) >= SHAPE_FLOOR,
            "{} appears {}/1000 times, which is not coverage",
            shape.name(),
            per_thousand(count)
        );
    }
    // Every consumer, including the three that must *not* coerce: a ToPrimitive that runs where
    // the specification says it should not is as much a defect as one that fails to run, and it is
    // only observable if `Boolean(o)`, `JSON.stringify(o)` and `Object.keys(o)` are generated too.
    for (sink, count) in Sink::ALL.iter().zip(census.sinks) {
        assert!(
            per_thousand(count) >= SINK_FLOOR,
            "coercion objects reach {} {}/1000 times",
            sink.name(),
            per_thousand(count)
        );
    }
    // `parseInt`/`parseFloat` are called out by name because they are the string-hint path, which
    // is the one that takes an extra step nothing else takes -- and the one both engines get wrong
    // today. A grammar that stopped emitting them would stop measuring that.
    for (call, count) in BuiltinCall::ALL.iter().zip(census.builtins) {
        assert!(
            per_thousand(count) >= BUILTIN_FLOOR,
            "{}(e) appears {}/1000 times",
            call.name(),
            per_thousand(count)
        );
    }
}
