//! Target 1: the interpreter and the native backend must print the same bytes.
//!
//! This is the project's stated worst failure mode -- two engines that both run to completion
//! and disagree -- and it is the expensive one to look for, because every iteration compiles a C
//! translation unit and links it. `fuzz/README.md` records the measured cost. The default
//! iteration count here is therefore small enough to belong in `cargo test`; the soak binary is
//! how this target is actually run.
//!
//! `KILN_FUZZ_ITERATIONS` raises it:
//!
//! ```sh
//! cd fuzz && KILN_FUZZ_ITERATIONS=500 cargo test --test differential_native -- --nocapture
//! ```

use kiln_fuzz::Roomy;
use kiln_fuzz::engines::{self, Outcome};
use kiln_fuzz::js::JsProgram;

/// Small by default. One iteration is one C compile; a thousand of them is not a unit test.
const DEFAULT_ITERATIONS: usize = 8;

#[test]
fn the_two_engines_agree_on_generated_programs() {
    // A missing C toolchain is a skip, not a pass and not a failure. The platform compiler is
    // the one dependency this check cannot fake, and reporting success without it would be the
    // hollow green `conformance/README.md` refuses to allow.
    let available = engines::with_native(|native| match native {
        Ok(native) => {
            eprintln!("toolchain: {}", native.describe());
            true
        }
        Err(reason) => {
            eprintln!("SKIP: {reason}");
            false
        }
    });
    if !available {
        return;
    }

    // A plain counter, not a shared one: bolero drives this closure on the calling thread, and
    // the interpreter is `!Send` (its object model is `Rc` throughout, ADR 0007), so a design
    // that needed to share state across threads would already be the wrong design here.
    let mut case = 0_usize;

    bolero::check!()
        .with_arbitrary::<Roomy<JsProgram>>()
        .with_iterations(kiln_fuzz::iterations(DEFAULT_ITERATIONS))
        .for_each(move |Roomy(program): &Roomy<JsProgram>| {
            let source = program.render();
            case += 1;
            let outcome = engines::with_native(|native| match native {
                Ok(native) => engines::compare(native, &source, "check"),
                Err(reason) => Outcome::Unavailable(reason.to_owned()),
            });
            assert!(
                !outcome.is_defect(),
                "case {case}: the two engines disagree\n--- source ---\n{source}\
                 --- outcome ---\n{outcome:#?}"
            );
            // Reported rather than asserted: an interpreter that refuses every generated program
            // would make this test pass while measuring nothing, and only the count says so.
            if matches!(outcome, Outcome::Match) {
                eprintln!("case {case}: match");
            }
        });
}
