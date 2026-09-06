//! C99 code generation directly from the SSA Value Graph.
//!
//! As specified in Phase 1 Step 1.3 of `fastest-native-plan.md` and ADR 0056:
//! - Eliminates the memory-bound `stack[]` array and `sp` stack pointer.
//! - Emits SSA intermediate values directly as C local variables (`kiln_val v0`, `v1`, ...).
//! - Retains `locals[]` only when needed for closure cells or generator suspension frames.
//! - Materializes call arguments into local stack arrays at call sites only, avoiding whole-body address-taking.

mod body;
mod clones;
mod ops;
mod sites;

pub use body::{
    emit_entry_from_graph, emit_entry_from_graph_with_profile, emit_function_from_graph,
    emit_function_from_graph_with_profile,
};
pub use clones::{emit_fast_clone, emit_restartable};
pub(crate) use sites::{method_candidates, this_site_count};

use crate::direct::TypedNeeds;
use crate::graph::{FunctionGraph, GraphOp, ValueId};
use kiln_ir::{ExportBinding, FunctionProto};
use std::collections::HashSet;

/// The first operation or terminator in `graph` that has no lowering below, as a short name,
/// for the census. `None` when everything lowers.
pub fn first_unlowerable(graph: &FunctionGraph) -> Option<String> {
    for block in &graph.blocks {
        for (_, op) in &block.operations {
            if !lowers(op) {
                let name = format!("{op:?}");
                return Some(name.split([' ', '(', '{']).next().unwrap_or("?").to_owned());
            }
        }
    }
    None
}

fn lowers(op: &GraphOp) -> bool {
    matches!(
        op,
        GraphOp::Const { .. }
            | GraphOp::LoadLocal(_)
            | GraphOp::StoreLocal { .. }
            | GraphOp::LoadCell(_)
            | GraphOp::StoreCell { .. }
            | GraphOp::LoadUpvalue(_)
            | GraphOp::StoreUpvalue { .. }
            | GraphOp::LoadBuiltin(_)
            | GraphOp::CaughtValue
            | GraphOp::Bridge { .. }
            | GraphOp::MakeCell(_)
            | GraphOp::LoadGlobal(_)
            | GraphOp::StoreGlobal { .. }
            | GraphOp::LoadThis
            | GraphOp::LoadNewTarget
            | GraphOp::MarkUninitialized(_)
            | GraphOp::CheckTdz { .. }
            | GraphOp::Unary { .. }
            | GraphOp::Binary { .. }
            | GraphOp::Call { .. }
            | GraphOp::CallMethod { .. }
            | GraphOp::NewArray { .. }
            | GraphOp::NewObject { .. }
            | GraphOp::New { .. }
            | GraphOp::MakeFunction { .. }
            | GraphOp::GetProperty { .. }
            | GraphOp::SetProperty { .. }
            | GraphOp::SetPropertyStrict { .. }
            | GraphOp::GetComputed { .. }
            | GraphOp::SetComputed { .. }
            | GraphOp::SetComputedStrict { .. }
            | GraphOp::Phi { .. }
    )
}

/// Which clone of a function body this is. The boxed entry is what a closure carries and what
/// every generic call reaches. The typed entry (`_t`) is reached only by direct calls that
/// proved every argument a number and passed exactly the parameter count: inside it the
/// parameters are numbers by contract, and nothing checks what the caller already proved.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Boxed,
    /// The boxed signature, with every slow path replaced by a restart through the boxed
    /// entry. Only a body that can be restarted gets one: nothing in it has an effect before
    /// the point a slow path could be taken, so running it again from the top is the same
    /// as running the slow path. With no runtime call left in it, the clone needs no saved
    /// registers and the C compiler inlines it where a cached callee is called by name.
    Fast,
    /// The parameters arrive as `double`s. When the plan proved the return a number, the result
    /// leaves as one too, through `double *result`; otherwise through `kiln_val *result`.
    Typed {
        returns_number: bool,
        /// The typed signature over a restartable body: every slow path is a restart through
        /// the boxed entry, exactly as in the fast clone, so what the fast path computes is
        /// known to be a number and stays in a register. The restart boxes the parameters,
        /// runs the boxed body once, and hands its answer back through the typed result.
        restartable: bool,
    },
}

/// What a versioned body's fast clone assumes beyond its parameters, and how it leaves when
/// an assumption fails. An invariant read in `arrays` is an array, checked once at entry. A
/// method call in `calls` answers a number: checked where it answers, and a call that answers
/// anything else leaves for the generic clone at the label after that same call, with every
/// value before it already computed. The generic clone, `exits` false, only carries the labels.
#[derive(Default, Clone, Debug)]
pub struct Speculation {
    pub arrays: HashSet<ValueId>,
    pub calls: HashSet<ValueId>,
    pub exits: bool,
}

impl EntryKind {
    /// Whether every slow path of this body is a restart through the boxed entry.
    pub fn restarts(self) -> bool {
        matches!(
            self,
            EntryKind::Fast
                | EntryKind::Typed {
                    restartable: true,
                    ..
                }
        )
    }
}

/// Which body a graph is emitted as: a function, called through a closure, or the unit's entry
/// point, which the embedding calls once with the module's imports and takes the exports from.
pub enum Site<'a> {
    Function {
        /// This function's index, so a call to itself can reuse its own upvalue table.
        index: u32,
    },
    Entry {
        /// The module's exports, read from `locals[]` at the return; `None` for a scalar unit.
        exports: Option<&'a [ExportBinding]>,
        /// A CommonJS unit: its one export is `module.exports`, read from the `module` slot.
        commonjs: bool,
    },
}

/// The typed entry's declaration: the result in a register, `double` where the plan proved
/// it and `kiln_val` otherwise, with `KILN_VAL_TYPED_FAIL` for a failure; then the depth, what
/// the body needs and nothing it does not, and the parameters as the doubles they are.
pub fn typed_signature(
    proto: &FunctionProto,
    returns_number: bool,
    needs: TypedNeeds,
) -> (&'static str, String) {
    let result = if returns_number { "double" } else { "kiln_val" };
    let mut params = String::from("int depth");
    if needs.upvalues {
        params.push_str(", kiln_val *upvalues");
    }
    if needs.receiver {
        params.push_str(", kiln_val receiver");
    }
    if needs.row {
        params.push_str(", const uint32_t *recv_row_in");
    }
    if needs.runtime {
        params.push_str(", kiln_host *host, kiln_value *thrown");
    }
    for k in 0..proto.parameter_count {
        params.push_str(&format!(", double p{k}"));
    }
    (result, params)
}

pub(crate) use crate::graph::array_slots;

/// Whether every operation in `graph` has a lowering below. This is the one place that decides
/// what the emitter takes; `graph_for` asks it and fails the unit's emission otherwise.
pub fn can_lower(graph: &FunctionGraph) -> bool {
    first_unlowerable(graph).is_none()
}
