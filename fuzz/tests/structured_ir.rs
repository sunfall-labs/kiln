//! Target 2: two independently written validators for one bytecode contract must agree.
//!
//! ```sh
//! cd fuzz && cargo test --test structured_ir
//! cd fuzz && cargo bolero test structured_ir::the_validators_agree_about_well_formedness
//! ```
//!
//! Microseconds per case: nothing here compiles or executes C, so this is a target coverage
//! feedback can actually steer. It is also the reason `src/ir.rs` bothers to be a small compiler
//! rather than a byte source -- see its module documentation.
//!
//! WHAT IS BEING COMPARED. `kiln_backend_c::emit` runs a static validator over a `Program` and
//! then generates C that trusts it: the emitted code has no bounds check on a constant index and
//! no underflow check on the operand stack, because `validate` promised there was no need. The
//! interpreter checks the same things dynamically and reports them as `ExecutionError`. So the
//! two crates hold separate, hand-written answers to one question, which is exactly the shape of
//! thing that drifts apart. If the emitter accepts a program the interpreter then calls
//! structurally malformed, the C for that program has undefined behaviour where the interpreter
//! has a diagnostic -- and undefined behaviour in a compiler's output is the failure that has no
//! upper bound on its consequences.

use std::rc::Rc;

use kiln_fuzz::Roomy;
use kiln_fuzz::ir::IrProgram;
use kiln_interpreter::{ExecutionError, execute_reclaiming};
use kiln_ir::Program;
use kiln_runtime::CaptureHost;

/// Well below the interpreter's default. A perturbation can turn a straight line into a loop
/// only by moving a jump backwards, which `src/ir.rs` never does -- so anything that reaches
/// this limit is already suspicious, and a low ceiling keeps a million iterations affordable.
const LIMITS: kiln_interpreter::ExecutionLimits = kiln_interpreter::ExecutionLimits {
    max_steps: 20_000,
    max_call_depth: 32,
    max_native_depth: 16,
};

/// The errors that mean "this bytecode is malformed" rather than "this program threw".
///
/// Only these are the emitter's business. A `TypeError` from `null.x` is a program doing
/// something JavaScript defines an answer for, and the emitter is right to accept it.
const fn is_structural(error: &ExecutionError) -> bool {
    matches!(
        error,
        ExecutionError::InvalidInstructionPointer { .. }
            | ExecutionError::ConstantOutOfBounds { .. }
            | ExecutionError::LocalOutOfBounds { .. }
            | ExecutionError::StackUnderflow { .. }
            | ExecutionError::InvalidJump { .. }
            | ExecutionError::UnknownFunction { .. }
            | ExecutionError::ImportCountMismatch { .. }
    )
}

/// Run one program. Returns the structural complaint, if there was one.
///
/// Every `Rc` the VM allocates is created and dropped inside this function; only a plain
/// `Option<String>` comes back. `kiln-interpreter` is `!Send`, and this is where that is dealt
/// with -- not by a lock or a marker, but by never letting an `Rc` be named outside the frame
/// that made it.
fn structural_complaint(program: &Program) -> Option<String> {
    let mut host = CaptureHost::new();
    match execute_reclaiming(
        Rc::new(program.clone()),
        &mut host,
        LIMITS,
        kiln_interpreter::ExecutionCapabilities::default(),
    ) {
        Ok(_) => None,
        Err(error) if is_structural(&error) => Some(error.to_string()),
        Err(_) => None,
    }
}

#[test]
fn the_validators_agree_about_well_formedness() {
    bolero::check!()
        .with_arbitrary::<Roomy<IrProgram>>()
        .with_iterations(kiln_fuzz::iterations(20_000))
        .for_each(|Roomy(generated): &Roomy<IrProgram>| {
            let (well_formed, perturbed) = generated.build();

            // The generator's own claim, checked first: if the *unperturbed* program is not
            // well-formed, `src/ir.rs` has a bug and every finding below is about the harness
            // rather than about kiln.
            assert!(
                structural_complaint(&well_formed).is_none(),
                "the generator emitted malformed bytecode before any perturbation was applied: \
                 {:?}\n{well_formed:#?}",
                structural_complaint(&well_formed)
            );

            for program in [&well_formed, &perturbed] {
                let Ok(module) = kiln_backend_c::emit(program) else {
                    // A refusal is the native surface still growing, and an `InvalidProgram` is
                    // the validator doing its job. Neither is a finding.
                    continue;
                };

                // Emission is documented to depend on nothing but the bytecode: "equal programs
                // produce byte-for-byte equal objects". The artifact cache is keyed on that
                // (ADR 0009), so an emitter that was not a pure function of its input would make
                // the cache serve one program's binary for another's.
                let again = kiln_backend_c::emit(program).expect("emission is deterministic");
                assert_eq!(module.source, again.source, "emission is not deterministic");
                assert_eq!(
                    module.symbol, again.symbol,
                    "the entry symbol is not stable"
                );

                if let Some(complaint) = structural_complaint(program) {
                    panic!(
                        "the emitter certified this program well-formed and the interpreter says \
                         it is not: {complaint}\n\
                         The generated C carries no check for this, so the same bytecode is a \
                         diagnostic on one engine and undefined behaviour on the other.\n\
                         {program:#?}"
                    );
                }
            }

            // The entry symbol is a content hash of the bytecode and the artifact cache is
            // addressed by it. Two different programs sharing one is not a slow build, it is the
            // wrong binary. Astronomically unlikely to be found by chance, and free to state.
            if let (Ok(left), Ok(right)) = (
                kiln_backend_c::emit(&well_formed),
                kiln_backend_c::emit(&perturbed),
            ) {
                assert!(
                    left.symbol != right.symbol || well_formed == perturbed,
                    "two different programs hashed to the same entry symbol {}",
                    left.symbol
                );
            }
        });
}

// ---------------------------------------------------------------------------------------------
// WHAT THIS TARGET HAS ALREADY FOUND, pinned so it cannot be lost or silently fixed.
//
// These live here rather than in a file of their own for a mechanical reason worth writing down:
// `cargo bolero` instruments every test target in the crate with sanitizer coverage, but only the
// ones that call `bolero::check!` pull in the libFuzzer runtime that defines the coverage symbols.
// A test file with no bolero target in it therefore fails to link under `cargo bolero` -- so every
// test target in this crate has one.
//
// Both findings are the same shape, and it is worth stating the shape before the cases.
// `kiln_backend_c::emit` runs a static validator and then generates C that *trusts* it: the
// emitted code has no bounds check on a constant index and no tag check before it uses a value,
// because `validate` promised there was no need. The interpreter carries its own idea of what a
// well-formed program is, and where the two differ the difference is not symmetrical -- the
// interpreter aborts the process, and the generated C has no opinion at all.
//
// Neither program below can be produced by today's lowering, which is why they are recorded
// here rather than filed as wrong answers. What they show is that `validate` is not currently a
// sufficient precondition for the C it licenses, and both are two instructions long.
//
// These tests assert that the findings are *still present*. When kiln fixes one, its test fails
// and says so -- which is the only way a suppression in `src/ir.rs` gets removed rather than
// outliving the bug it was written for.

use std::panic::{AssertUnwindSafe, catch_unwind};

use kiln_ir::{Constant, Instruction, UnaryOp};

/// Does the interpreter abort on this program, and does the emitter accept it?
fn probe(program: Program) -> (bool, bool) {
    let accepted = kiln_backend_c::emit(&program).is_ok();
    // The default hook would print a backtrace per case and drown the report.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let aborted = catch_unwind(AssertUnwindSafe(|| {
        let mut host = CaptureHost::new();
        let _ = kiln_interpreter::execute_reclaiming(
            Rc::new(program),
            &mut host,
            kiln_interpreter::ExecutionLimits::default(),
            kiln_interpreter::ExecutionCapabilities::default(),
        );
    }))
    .is_err();
    std::panic::set_hook(previous);
    (accepted, aborted)
}

/// A cell read as a local.
///
/// `NewCell(0)` puts a cell in slot 0; `LoadLocal(0)` reads the slot itself rather than through
/// the cell, so the cell marker reaches an expression. The interpreter's `is_truthy` answers
/// `unreachable!("a cell is never used as a value")` and the process dies.
///
/// Lowering decides per binding whether a slot is a cell or a plain local and never mixes the
/// two, so no source program reaches this today. `validate` does not enforce it, though, and the
/// C emitted for `LoadLocal` on a cell slot is not a diagnostic -- it is whatever the generated
/// code does with a tag it was told could not appear.
///
/// Found by `structured_ir` on its first run, before any perturbation was involved.
#[test]
fn a_cell_read_as_a_local_still_aborts_the_interpreter() {
    let (accepted, aborted) = probe(Program {
        source: std::sync::Arc::from(""),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::Undefined],
        code: vec![
            Instruction::NewCell(0),
            Instruction::LoadLocal(0),
            Instruction::Unary(UnaryOp::Not),
            Instruction::Pop,
            Instruction::Const(0),
            Instruction::Return,
        ],
        local_count: 4,
        functions: Vec::new(),
    });

    assert!(
        accepted && aborted,
        "accepted by the emitter: {accepted}, aborted the interpreter: {aborted}.\n\
         If this now fails, the finding is fixed -- delete the suppression in `src/ir.rs` that \
         keeps the generator from putting a `LoadLocal` on a cell slot, and delete this test."
    );
}

/// An array hole used as a value.
///
/// `PushHole` is the marker for a *missing* element of an array literal. Lowering only ever
/// emits it between the elements of a `NewArray`, where every read peels it off again. Left on
/// the stack and then *called*, it reaches the native-builtin dispatch, which answers
/// `unreachable!("an array hole is never a value")`.
///
/// Found by `structured_ir` at around a hundred thousand cases, from a hole that had reached an
/// ordinary expression position. Two instructions reproduce it.
#[test]
fn an_array_hole_used_as_a_value_still_aborts_the_interpreter() {
    let (accepted, aborted) = probe(Program {
        source: std::sync::Arc::from(""),
        version: kiln_ir::BYTECODE_VERSION,
        constants: vec![Constant::Undefined],
        code: vec![
            Instruction::PushHole,
            Instruction::Call { argc: 0 },
            Instruction::Pop,
            Instruction::Const(0),
            Instruction::Return,
        ],
        local_count: 1,
        functions: Vec::new(),
    });

    assert!(
        accepted && aborted,
        "accepted by the emitter: {accepted}, aborted the interpreter: {aborted}.\n\
         If this now fails, the finding is fixed -- delete the suppression in `src/ir.rs` that \
         keeps `Expr::Hole` inside array literals, and delete this test."
    );
}
