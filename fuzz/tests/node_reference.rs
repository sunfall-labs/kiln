//! Is node a reference this harness may trust, and does it still see the bugs it found?
//!
//! Two jobs, and they pull in opposite directions, which is why they are in one file.
//!
//! The first is that the *batched* node reference must be node. `src/node_batch.js` runs sixty-four
//! generated programs in one process to get the per-case cost from ~30 ms down to ~1 ms, and a
//! faster reference that is not the reference is worse than no reference at all: `console.log` does
//! not print values, it inspects them -- `[ 1, 2 ]`, `{ p0: 'x' }`, `<1 empty item>` -- and a
//! driver whose formatting drifted by one space would report a divergence on every program that
//! logged an array. So the two paths are pinned against each other byte for byte.
//!
//! The second is the convention `tests/structured_ir.rs` already set: **a finding is pinned as a
//! test that fails when it is fixed.** Two of the three original wrong answers are now fixed and
//! remain as regression tests; the remaining finding is still pinned below. Node's answer for
//! each was observed and not transcribed.

use kiln_fuzz::engines::{self, Interpreted};
use kiln_fuzz::js::JsProgram;
use kiln_fuzz::node::{Node, NodeStatus};
use kiln_fuzz::triage::{self, Class, NativeAnswer};

/// A missing node is a skip and never a pass, for the reason `scripts/differential.sh` gives: the
/// reference implementation is the one dependency this cannot fake, and pretending to have run it
/// would be worse than saying it did not.
macro_rules! node_or_skip {
    () => {
        match Node::discover() {
            Ok(node) => node,
            Err(reason) => {
                eprintln!("SKIP: {reason}");
                return;
            }
        }
    };
}

/// The formatting `console.log` decides and a hand-written driver would get wrong.
const FORMATTING: &[&str] = &[
    "console.log([1, 2]); console.log([]); console.log([[1], [2, [3]]]);",
    "console.log({p0: 1}); console.log({}); console.log({p0: {p1: 'x'}});",
    "console.log(-0); console.log([-0]); console.log(NaN); console.log(1e308);",
    "console.log('x'); console.log(['x']); console.log(9007199254740993);",
    "console.log([undefined, , 3]); console.log([null, undefined]);",
    "console.log(1, 2); console.log(0.1 + 0.2); console.log(typeof {});",
    "console.log('\\ud800'); console.log('\\u{1f600}');",
    "console.log('a'); throw 1;",
    "console.log('before'); undefined.p0;",
];

#[test]
fn the_batched_reference_prints_exactly_what_a_real_node_process_prints() {
    let node = node_or_skip!();
    let mut sources: Vec<String> = FORMATTING.iter().map(|text| (*text).to_owned()).collect();
    // Generated programs too, and not only the hand-written formatting cases: the driver has to be
    // node for the programs it will actually be asked about, and those are much stranger.
    bolero::check!()
        .with_arbitrary::<kiln_fuzz::Roomy<JsProgram>>()
        .with_iterations(kiln_fuzz::iterations(24))
        .for_each(|kiln_fuzz::Roomy(program): &kiln_fuzz::Roomy<JsProgram>| {
            assert!(!program.render().is_empty());
        });
    let mut rng = 0x9E37_79B9_7F4A_7C15_u64;
    for _ in 0..24 {
        let mut bytes = vec![0_u8; 4096];
        for chunk in bytes.chunks_mut(8) {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            let value = rng.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
        if let Ok(program) = <JsProgram as arbitrary::Arbitrary>::arbitrary_take_rest(
            arbitrary::Unstructured::new(&bytes),
        ) {
            sources.push(program.render());
        }
    }

    let batched = node.run_batch(&sources).expect("the batch driver ran");
    assert_eq!(batched.len(), sources.len());
    for (index, (source, batch)) in sources.iter().zip(&batched).enumerate() {
        let single = node
            .run(source, &format!("pin-{index}"))
            .expect("one node process ran");
        assert_eq!(
            batch.stdout, single.stdout,
            "the batched reference and `node case.js` printed different bytes for:\n{source}"
        );
        assert_eq!(
            batch.status, single.status,
            "the batched reference and `node case.js` ended differently for:\n{source}"
        );
    }
}

#[test]
fn every_generated_program_is_javascript() {
    // The premise of `js.rs` is that every value of `JsProgram` renders to a syntactically valid
    // script. Nothing checks that premise except a reference implementation being asked, and a
    // generator that emits something node refuses is measuring its own bug -- which is why
    // `Class::GeneratorBug` is a class of its own and why this asserts on it directly.
    let node = node_or_skip!();
    let mut sources = Vec::new();
    let mut rng = 0x243F_6A88_85A3_08D3_u64;
    for _ in 0..kiln_fuzz::iterations(256) {
        let mut bytes = vec![0_u8; 4096];
        for chunk in bytes.chunks_mut(8) {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            let value = rng.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
        if let Ok(program) = <JsProgram as arbitrary::Arbitrary>::arbitrary_take_rest(
            arbitrary::Unstructured::new(&bytes),
        ) {
            sources.push(program.render());
        }
    }
    for (source, run) in sources
        .iter()
        .zip(node.run_batch(&sources).expect("node ran"))
    {
        assert_ne!(
            run.status,
            NodeStatus::Invalid,
            "the generator emitted something node refuses ({}):\n{source}",
            run.diagnostic
        );
    }
}

/// One program through node and the interpreter, classified the way a soak classifies it.
fn against_node(node: &Node, source: &str, tag: &str) -> (Class, String, String) {
    let program = engines::lower(source).expect("lowering accepted the probe");
    let interpreted = engines::interpret(&program);
    let run = node.run(source, tag).expect("node ran");
    let verdict = triage::triage(source, &run, &interpreted, &NativeAnswer::NotAsked);
    let printed = match &interpreted {
        Interpreted::Completed(stdout) | Interpreted::Stopped { stdout, .. } => stdout.clone(),
        Interpreted::OverBudget => String::new(),
    };
    (verdict.class, run.stdout, printed)
}

// --- Findings, and the fixes they are now guarding ------------------------------------------
//
// The convention is `tests/structured_ir.rs`'s: a finding is pinned as a test, so that fixing it
// is noticed. Two of the five below were wrong answers when this file was written and were fixed
// while it was being written; they are kept as the regression tests they became, because the
// interesting thing about `parseInt({toString: () => "99"})` is not that it was broken, it is that
// **both engines answered `NaN` together** and the two-engine comparison called that a pass.

/// `parseInt` runs ToPrimitive with the string hint before parsing.
///
/// THE SHAPE THE TWO-ENGINE COMPARISON CANNOT SEE. While this was broken, both engines answered
/// `NaN`, so they agreed, so `differential_native` reported a match and said nothing at all. node
/// answers `99`, and it took node to say so.
#[test]
fn parse_int_converts_an_object_with_a_user_to_string() {
    let node = node_or_skip!();
    let (class, reference, kiln) = against_node(
        &node,
        "console.log(parseInt({ toString: () => \"99\" }));\n",
        "parseint",
    );
    assert_eq!(reference, "99\n");
    assert_eq!(kiln, reference);
    assert_eq!(class, Class::Agrees);
}

/// The single-argument builtin path runs ToPrimitive.
///
/// The half of the same root cause that *was* visible to the two-engine comparison, because the
/// native backend had been fixed and the interpreter had not.
#[test]
fn the_builtin_call_path_converts_an_object_with_a_user_value_of() {
    let node = node_or_skip!();
    let (class, reference, kiln) = against_node(
        &node,
        "console.log(Math.abs({ valueOf: () => -42 }));\n",
        "mathabs",
    );
    assert_eq!(reference, "42\n");
    assert_eq!(kiln, reference);
    assert_eq!(class, Class::Agrees);
}

/// `Symbol.toPrimitive` is consulted before the ordinary conversion methods.
#[test]
fn symbol_to_primitive_is_consulted() {
    let node = node_or_skip!();
    let (class, reference, kiln) = against_node(
        &node,
        "const o = { [Symbol.toPrimitive]: (hint) => (hint === \"string\" ? \"S\" : 5) };\n\
         console.log(Number(o));\nconsole.log(String(o));\n",
        "toprimitive",
    );
    assert_eq!(reference, "5\nS\n");
    assert_eq!(kiln, reference);
    assert_eq!(class, Class::Agrees);
}

/// A conversion with no primitive available raises a TypeError before printing anything. The
/// interpreter reports that unsupported throw path as a refusal, after matching node's output.
#[test]
fn a_conversion_with_no_primitive_available_raises() {
    let node = node_or_skip!();
    let (class, reference, kiln) = against_node(
        &node,
        "console.log({ valueOf: () => ({}), toString: () => ({}) } + 1);\n",
        "noprimitive",
    );
    assert_eq!(reference, "", "node threw before printing anything");
    assert_eq!(kiln, reference);
    assert_eq!(class, Class::Refused);
}

/// A lone surrogate escape in a string literal is one code unit. **A wrong answer as this is
/// written**: kiln reads `"\ud800"` as five.
///
/// This one is in the frontend, so both engines inherit it, and it is the purest example of why
/// node is here: there is no arrangement of kiln components that could have caught it.
#[test]
fn a_lone_surrogate_escape_is_one_code_unit() {
    let node = node_or_skip!();
    let (class, reference, kiln) = against_node(
        &node,
        "console.log(\"\\ud800\".length);\nconsole.log(\"a\\ud800b\".length);\n",
        "surrogate",
    );
    assert_eq!(reference, "1\n3\n");
    assert_eq!(kiln, "5\n7\n");
    assert_eq!(class, Class::InterpreterDiffersFromNode);
}
