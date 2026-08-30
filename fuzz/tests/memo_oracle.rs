//! Target 3: a memoized query engine answers what a from-scratch one answers.
//!
//! ```sh
//! cd fuzz && cargo test --test memo_oracle
//! cd fuzz && cargo bolero test memo_oracle::memoized_agrees_with_a_fresh_engine_at_every_step
//! ```
//!
//! Microseconds per case, so this is the target that can actually use coverage feedback. See
//! `src/oracle.rs` for the property and where it comes from.

use kiln_fuzz::Roomy;
use kiln_fuzz::oracle::{self, History};

/// Guards the guard. If the text pool stopped producing distinguishable IR, the `irs` field of
/// the comparison would pass unconditionally and the coverage would be an illusion.
#[test]
fn the_text_pool_produces_distinguishable_ir() {
    assert!(
        oracle::text_pool_produces_distinguishable_ir(),
        "every text in the pool compiled to the same IR, so comparing it proves nothing"
    );
}

#[test]
fn memoized_agrees_with_a_fresh_engine_at_every_step() {
    bolero::check!()
        .with_arbitrary::<Roomy<History>>()
        .with_iterations(kiln_fuzz::iterations(20_000))
        .for_each(|Roomy(history): &Roomy<History>| {
            if let Err(disagreement) = oracle::memoized_agrees_at_every_step(history) {
                panic!("{}", disagreement.0);
            }
        });
}

#[test]
fn memoized_agrees_with_a_fresh_engine_after_a_batch() {
    bolero::check!()
        .with_arbitrary::<Roomy<History>>()
        .with_iterations(kiln_fuzz::iterations(20_000))
        .for_each(|Roomy(history): &Roomy<History>| {
            if let Err(disagreement) = oracle::memoized_agrees_after_a_batch(history) {
                panic!("{}", disagreement.0);
            }
        });
}
