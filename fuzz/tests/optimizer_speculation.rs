//! Target: Differential fuzzing of the optimizer across Node and Kiln under
//! KILN_SPECULATE=0, 1, and exit.
//!
//! As specified in Phase 2 Step 2.2 of `fastest-native-plan.md`.

use kiln_fuzz::engines::Native;
use kiln_fuzz::node::Node;
use kiln_fuzz::speculate_runner::{SpeculateVerdict, compare_all_modes};
use kiln_fuzz::typed_js::TypedProgram;

const DEFAULT_ITERATIONS: usize = 6;

#[test]
fn optimizer_speculation_matches_node_and_modes() {
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

    let iterations = kiln_fuzz::iterations(DEFAULT_ITERATIONS);
    for case in 0..iterations {
        let seed = 1000_u64 + case as u64;
        let source = TypedProgram::generate_seeded(seed);
        let tag = format!("test_opt_{case}");

        let verdict = compare_all_modes(&native, &node, &source, &tag);
        match verdict {
            SpeculateVerdict::Match => {
                eprintln!("case {case}: match across node, interp, spec0, spec1, specexit");
            }
            SpeculateVerdict::Diverged {
                source,
                divergence_reason,
                node,
                interp,
                spec0,
                spec1,
                specexit,
            } => {
                panic!(
                    "Optimizer divergence in case {case}:\nReason: {divergence_reason}\n\
                     --- Source ---\n{source}\n\
                     --- Node ---\n{node}\n\
                     --- Interp ---\n{interp}\n\
                     --- Spec0 ---\n{spec0}\n\
                     --- Spec1 ---\n{spec1}\n\
                     --- SpecExit ---\n{specexit}"
                );
            }
            SpeculateVerdict::Crashed(err) => {
                panic!("Optimizer fuzz crashed in case {case}: {err}");
            }
            SpeculateVerdict::Refused(reason) => {
                eprintln!("case {case}: refused ({reason})");
            }
            SpeculateVerdict::NotLowered(reason) => {
                eprintln!("case {case}: not-lowered ({reason})");
            }
        }
    }
}
