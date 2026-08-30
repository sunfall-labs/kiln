//! Differential fuzzing for kiln.
//!
//! kiln's central architectural claim is that two independently written engines -- the Rust VM in
//! `kiln-interpreter` and the C emitted by `kiln-backend-c` -- produce byte-identical output from
//! the same bytecode. `scripts/native.sh` checks that claim over a hand-written corpus. A corpus
//! only ever contains programs somebody thought to write, and the project's own conformance
//! README says exactly what that costs: "a real program composes features in combinations nobody
//! wrote a test for". This crate is the machine that writes them.
//!
//! Three targets, in `tests/`:
//!
//! | target | what it compares | cost per case |
//! |---|---|---|
//! | `differential_native` | interpreter stdout vs native stdout | a C compile -- see `README.md` |
//! | `structured_ir` | the emitter and the VM against hand-built bytecode | microseconds |
//! | `memo_oracle` | a memoized query engine vs a from-scratch one | microseconds |
//!
//! ON `Rc` AND THREADS. `kiln-interpreter` is `!Send`: its object model is `Rc` throughout. The
//! harness never has to think about this because of where the boundary is drawn -- [`engines::interpret`]
//! takes a `&Program` and returns a `String`, and every `Rc` lives and dies inside that one call.
//! Nothing an `Rc` can reach is ever named by a value that outlives the frame, so there is
//! nothing that could cross a thread boundary even if an engine wanted to move it.

/// How many cases a target runs, from `KILN_FUZZ_ITERATIONS`.
///
/// Every target reads the same variable so one command scales all of them, and each supplies its
/// own default: a target costing microseconds and one costing a C compilation do not want the
/// same number, and hard-coding either would make `cargo test` useless for one of them.
pub fn iterations(default: usize) -> usize {
    std::env::var("KILN_FUZZ_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// A list whose length is chosen up front, instead of one coin flip at a time.
///
/// THIS EXISTS BECAUSE `Vec<T>: Arbitrary` PRODUCES ALMOST NOTHING. `arbitrary`'s `Vec` reads a
/// bool before each element and stops on the first false, so the length is geometric with
/// p = 1/2: **mean one element**. A `Vec<Statement>` body is therefore one statement, a
/// `Vec<Op>` history is one operation, and a target built on either passes while measuring
/// nothing -- the hollow green `conformance/README.md` refuses to allow, arrived at from a
/// different direction. Measured, not assumed: `tests/generator_health.rs` asserts the means and
/// caught exactly this.
///
/// `Many` reads one length byte and then that many elements, so lengths are spread over
/// `0..=MAX` instead of collapsing onto zero and one.
#[derive(Debug, Clone, Default)]
pub struct Many<T, const MAX: usize>(pub Vec<T>);

impl<T, const MAX: usize> std::ops::Deref for Many<T, MAX> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<'a, T: arbitrary::Arbitrary<'a>, const MAX: usize> arbitrary::Arbitrary<'a> for Many<T, MAX> {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let length = usize::from(u8::arbitrary(u)?) % (MAX + 1);
        let mut items = Vec::with_capacity(length);
        for _ in 0..length {
            // Stopping when the entropy runs out rather than failing: a short input should make
            // a small program, not no program. `arbitrary` signals exhaustion by returning
            // zeroes rather than an error, so the emptiness check is the reliable one.
            if u.is_empty() {
                break;
            }
            items.push(T::arbitrary(u)?);
        }
        Ok(Self(items))
    }
}

/// A generated value that asks the driver for a real byte budget.
///
/// THIS EXISTS BECAUSE `size_hint` DECIDES THE BUDGET. bolero's RNG driver picks the number of
/// bytes one value may consume uniformly from the type's own `size_hint()` -- and `arbitrary`'s
/// derive, for a type that recurses through a `Box`, reports a *finite* upper bound, because its
/// recursion guard scores the recursive arm as zero. For [`js::JsProgram`] that bound is a few
/// dozen bytes: enough for two initialisers, after which every remaining field reads zeroes and
/// renders as the same constant. Programs came out with empty bodies, empty function bodies, and
/// not one loop or `try` in two thousand cases.
///
/// Declaring no upper bound makes the driver offer up to its own `max_len` instead. Measured, as
/// everything else here is: `tests/generator_health.rs` records the before and after.
#[derive(Debug, Clone)]
pub struct Roomy<T>(pub T);

impl<'a, T: arbitrary::Arbitrary<'a>> arbitrary::Arbitrary<'a> for Roomy<T> {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        T::arbitrary(u).map(Self)
    }

    fn size_hint(_depth: usize) -> (usize, Option<usize>) {
        (1, None)
    }
}

/// A recursive field that stops recursing at a fixed depth, *while generating*.
///
/// THIS EXISTS BECAUSE TRUNCATING AT RENDER TIME THROWS THE ENTROPY AWAY. The first version of
/// this crate let `arbitrary` build an expression tree of whatever depth it liked and cut it off
/// at depth three on the way out. Measured, that spent four thousand bytes of input on a program
/// that rendered to two hundred and sixty: every subtree below the cut was generated, paid for,
/// and discarded, and the program never grew past its first two statements no matter how much
/// input it was given. Bounding the depth where the value is *made* spends the same budget on
/// breadth instead.
///
/// The depth counter is shared by every `Nested` in the process. That is correct rather than
/// convenient: only one generator runs at a time, they never nest inside one another, and the
/// counter is thread-local, so two threads generating at once cannot see each other's depth.
pub struct Nested<T, const MAX: u32>(pub Box<T>);

/// What a [`Nested`] value collapses to at the depth limit.
///
/// A leaf, not an absence: the surrounding syntax has to stay valid, and a variant that renders
/// to nothing would make the parent malformed.
pub trait Leafy: Sized {
    fn leaf(u: &mut arbitrary::Unstructured<'_>) -> arbitrary::Result<Self>;
}

impl<T, const MAX: u32> std::ops::Deref for Nested<T, MAX> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: std::fmt::Debug, const MAX: u32> std::fmt::Debug for Nested<T, MAX> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<T: Clone, const MAX: u32> Clone for Nested<T, MAX> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

thread_local! {
    static NESTING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

impl<'a, T, const MAX: u32> arbitrary::Arbitrary<'a> for Nested<T, MAX>
where
    T: arbitrary::Arbitrary<'a> + Leafy,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let depth = NESTING.get();
        if depth >= MAX {
            return T::leaf(u).map(|leaf| Self(Box::new(leaf)));
        }
        NESTING.set(depth + 1);
        let value = T::arbitrary(u);
        // Restored on the error path too: a `Result` that escaped with the counter still raised
        // would shrink every later value in the same run.
        NESTING.set(depth);
        value.map(|value| Self(Box::new(value)))
    }

    fn size_hint(_depth: usize) -> (usize, Option<usize>) {
        // No finite upper bound, for the reason `Roomy` documents: a bound here is a budget
        // there.
        (1, None)
    }
}

// Linked from the library itself, on purpose and not by accident.
//
// `cargo bolero` instruments *every* target in the package with sanitizer coverage, and the
// symbols that instrumentation calls are defined by libFuzzer's runtime, which arrives with the
// `bolero` rlib. A target that does not name `bolero` does not link that rlib and therefore does
// not link the runtime, and fails with undefined `__sanitizer_cov_*`. Naming it here puts it in
// the graph of everything that depends on this crate -- the two binaries included -- so
// `cargo bolero` can build the package as a whole.
use bolero as _;

pub mod engines;
pub mod ir;
pub mod js;
pub mod oracle;

#[cfg(test)]
mod tests {
    /// `Many` is the reason the other targets generate anything at all, so its bounds are worth
    /// one assertion of their own.
    #[test]
    fn many_respects_its_bound_and_is_not_always_empty() {
        let mut longest = 0;
        let mut total = 0;
        bolero::check!()
            .with_arbitrary::<Roomy<Many<u8, 7>>>()
            .with_iterations(500)
            .for_each(|Roomy(items): &Roomy<Many<u8, 7>>| {
                assert!(items.len() <= 7, "Many exceeded its bound");
            });
        // Re-run outside the engine so the counters are not captured by the closure, which would
        // have to be `RefUnwindSafe`.
        for seed in 0_u8..64 {
            let bytes = [seed, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
            let mut u = arbitrary::Unstructured::new(&bytes);
            let items = <Many<u8, 7> as arbitrary::Arbitrary>::arbitrary(&mut u).unwrap();
            longest = longest.max(items.len());
            total += items.len();
        }
        assert!(longest >= 6, "Many never produced a long list");
        assert!(total > 64, "Many produced a mean length below one");
    }

    use super::{Many, Roomy};
}
