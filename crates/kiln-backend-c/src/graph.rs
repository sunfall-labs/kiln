//! SSA Value Graph representation and bytecode abstract interpretation.
//!
//! As established in ADR 0056, the Value Graph is a Turboshaft-style linear buffer of SSA operations
//! organized into a Control Flow Graph (CFG) of basic blocks. Abstract interpretation consumes
//! stack bytecode and lowers it into immutable SSA values, eliminating the runtime operand stack
//! and resolving operand dependencies into direct value edges.

use bitflags::bitflags;
use kiln_ir::{BinaryOp, Constant, FunctionProto, Instruction, Program, UnaryOp, Upvalue};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// Strongly typed identifier for an SSA value in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub u32);

/// Strongly typed identifier for a basic block in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u32);

/// Deterministic, stable identifier for a call site, property access, or allocation site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SiteId(pub u32);

bitflags! {
    /// Observable effects an operation may have (ADR 0061).
    ///
    /// The first migration preserves the old `Pure | Unknown` classification: operations are
    /// either [`Effects::PURE`] or [`Effects::UNKNOWN`]. Later changes may narrow one operation at
    /// a time, with the verifier and optimization that consumes the narrower fact in the same
    /// change.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Effects: u16 {
        const MAY_THROW       = 1 << 0;
        const CALLS_USER_CODE = 1 << 1;
        const ALLOCATES       = 1 << 2;
        const READS_HEAP      = 1 << 3;
        const WRITES_HEAP     = 1 << 4;
        const READS_HOST      = 1 << 5;
        const WRITES_HOST     = 1 << 6;
        const MAY_SUSPEND     = 1 << 7;
        const READS_CLOCK     = 1 << 8;
        const READS_ENTROPY   = 1 << 9;
        const READS_LOCALE    = 1 << 10;
    }
}

impl Effects {
    pub const PURE: Self = Self::empty();
    pub const UNKNOWN: Self = Self::all();
}

/// Node operations within the SSA Value Graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphOp {
    // Constants and Literals
    Const {
        index: u32,
        constant: Constant,
    },
    LoadLocal(u32),
    StoreLocal {
        slot: u32,
        value: ValueId,
    },
    LoadCell(u32),
    StoreCell {
        slot: u32,
        value: ValueId,
    },
    /// A captured variable of an enclosing function: the closure's upvalue box, read or written.
    LoadUpvalue(u32),
    StoreUpvalue {
        index: u32,
        value: ValueId,
    },
    /// A name the compiler resolved to something the runtime supplies.
    LoadBuiltin(kiln_ir::Builtin),
    /// The value supplied by a generator caller when a suspension resumes. It has no compile-time
    /// value or type; the C emitter restores it from `frame->sent` at the synthetic resume edge.
    ResumeValue,
    /// The value a handler block starts with: what was thrown into it. Only ever the first
    /// operation of a handler block.
    CaughtValue,
    IsReturnSignal(ValueId),
    /// An instruction lowered by its own body in `tail.rs`, on a small local stack holding
    /// `operands`. The tail of the instruction set: each of these has one correct lowering on
    /// `stack[sp]`, and this op runs it there rather than carrying a second copy. `pushes` is how many results it leaves; the op's value is the top one, and
    /// `ToPropertyKey`, the one instruction leaving two, keeps its object beneath.
    Bridge {
        instruction: Instruction,
        operands: Vec<ValueId>,
        pushes: u8,
    },
    MakeCell(u32),
    LoadGlobal(u32),
    StoreGlobal {
        slot: u32,
        value: ValueId,
    },
    LoadThis,
    LoadNewTarget,
    MarkUninitialized(u32),
    CheckTdz {
        slot: u32,
        value: ValueId,
    },

    // Unary and Binary Operators
    Unary {
        op: UnaryOp,
        value: ValueId,
    },
    Binary {
        op: BinaryOp,
        left: ValueId,
        right: ValueId,
    },

    // Property and Element Access
    GetProperty {
        object: ValueId,
        name: u32,
        site: SiteId,
    },
    SetProperty {
        object: ValueId,
        name: u32,
        value: ValueId,
        site: SiteId,
    },
    SetPropertyStrict {
        object: ValueId,
        name: u32,
        value: ValueId,
        strict: bool,
        site: SiteId,
    },
    GetComputed {
        object: ValueId,
        key: ValueId,
        site: SiteId,
    },
    SetComputed {
        object: ValueId,
        key: ValueId,
        value: ValueId,
        site: SiteId,
    },
    SetComputedStrict {
        object: ValueId,
        key: ValueId,
        value: ValueId,
        strict: bool,
        site: SiteId,
    },

    // Invocation and Construction
    MakeFunction {
        index: u32,
        site: SiteId,
    },
    Call {
        callee: ValueId,
        args: Vec<ValueId>,
        site: SiteId,
    },
    CallMethod {
        object: ValueId,
        name: u32,
        args: Vec<ValueId>,
        site: SiteId,
    },
    NewArray {
        elements: Vec<ValueId>,
        site: SiteId,
    },
    /// An object literal: `count` key/value pairs in source order.
    NewObject {
        pairs: Vec<(ValueId, ValueId)>,
        site: SiteId,
    },
    /// `new callee(args)`.
    New {
        callee: ValueId,
        args: Vec<ValueId>,
        site: SiteId,
    },

    // Super and Private Members

    // Modules

    // SSA Phi
    Phi {
        inputs: Vec<(BlockId, ValueId)>,
    },
}

impl GraphOp {
    /// Every value this operation reads.
    pub fn operands(&self) -> Vec<ValueId> {
        match self {
            Self::Const { .. }
            | Self::LoadLocal(_)
            | Self::LoadCell(_)
            | Self::LoadUpvalue(_)
            | Self::LoadBuiltin(_)
            | Self::ResumeValue
            | Self::CaughtValue
            | Self::MakeCell(_)
            | Self::LoadGlobal(_)
            | Self::LoadThis
            | Self::LoadNewTarget
            | Self::MarkUninitialized(_)
            | Self::MakeFunction { .. } => Vec::new(),
            Self::StoreLocal { value, .. }
            | Self::StoreCell { value, .. }
            | Self::StoreUpvalue { value, .. }
            | Self::StoreGlobal { value, .. }
            | Self::CheckTdz { value, .. }
            | Self::IsReturnSignal(value)
            | Self::Unary { value, .. } => vec![*value],
            Self::Bridge { operands, .. } => operands.clone(),
            Self::Binary { left, right, .. } => vec![*left, *right],
            Self::GetProperty { object, .. } => vec![*object],
            Self::SetProperty { object, value, .. }
            | Self::SetPropertyStrict { object, value, .. } => vec![*object, *value],
            Self::GetComputed { object, key, .. } => vec![*object, *key],
            Self::SetComputed {
                object, key, value, ..
            }
            | Self::SetComputedStrict {
                object, key, value, ..
            } => vec![*object, *key, *value],
            Self::Call { callee, args, .. } | Self::New { callee, args, .. } => {
                std::iter::once(*callee)
                    .chain(args.iter().copied())
                    .collect()
            }
            Self::CallMethod { object, args, .. } => std::iter::once(*object)
                .chain(args.iter().copied())
                .collect(),
            Self::NewArray { elements, .. } => elements.clone(),
            Self::NewObject { pairs, .. } => pairs.iter().flat_map(|(k, v)| [*k, *v]).collect(),
            Self::Phi { inputs } => inputs.iter().map(|(_, v)| *v).collect(),
        }
    }

    /// Rewrite SSA operands, leaving local slots, constant indices and site IDs untouched.
    fn map_operands(&mut self, map: &impl Fn(ValueId) -> ValueId) {
        let mut apply = |value: &mut ValueId| *value = map(*value);
        match self {
            Self::Const { .. }
            | Self::LoadLocal(_)
            | Self::LoadCell(_)
            | Self::LoadUpvalue(_)
            | Self::LoadBuiltin(_)
            | Self::ResumeValue
            | Self::CaughtValue
            | Self::MakeCell(_)
            | Self::LoadGlobal(_)
            | Self::LoadThis
            | Self::LoadNewTarget
            | Self::MarkUninitialized(_)
            | Self::MakeFunction { .. } => {}
            Self::StoreLocal { value, .. }
            | Self::StoreCell { value, .. }
            | Self::StoreUpvalue { value, .. }
            | Self::StoreGlobal { value, .. }
            | Self::CheckTdz { value, .. }
            | Self::IsReturnSignal(value)
            | Self::Unary { value, .. } => apply(value),
            Self::Bridge { operands, .. } => operands.iter_mut().for_each(&mut apply),
            Self::Binary { left, right, .. } => {
                apply(left);
                apply(right);
            }
            Self::GetProperty { object, .. } => apply(object),
            Self::SetProperty { object, value, .. }
            | Self::SetPropertyStrict { object, value, .. } => {
                apply(object);
                apply(value);
            }
            Self::GetComputed { object, key, .. } => {
                apply(object);
                apply(key);
            }
            Self::SetComputed {
                object, key, value, ..
            }
            | Self::SetComputedStrict {
                object, key, value, ..
            } => {
                apply(object);
                apply(key);
                apply(value);
            }
            Self::Call { callee, args, .. } | Self::New { callee, args, .. } => {
                apply(callee);
                args.iter_mut().for_each(&mut apply);
            }
            Self::CallMethod { object, args, .. } => {
                apply(object);
                args.iter_mut().for_each(&mut apply);
            }
            Self::NewArray { elements, .. } => elements.iter_mut().for_each(&mut apply),
            Self::NewObject { pairs, .. } => {
                for (key, value) in pairs {
                    apply(key);
                    apply(value);
                }
            }
            Self::Phi { inputs } => {
                for (_, value) in inputs {
                    apply(value);
                }
            }
        }
    }

    /// Effect classification for this operation (ADR 0061).
    pub fn effects(&self) -> Effects {
        match self {
            Self::Const { .. }
            | Self::LoadLocal(_)
            | Self::StoreLocal { .. }
            | Self::LoadBuiltin(_)
            | Self::ResumeValue
            | Self::CaughtValue
            | Self::IsReturnSignal(_)
            | Self::LoadThis
            | Self::LoadNewTarget
            | Self::Phi { .. } => Effects::PURE,

            Self::MarkUninitialized(_) => Effects::WRITES_HEAP,

            Self::CheckTdz { .. } => Effects::MAY_THROW,

            Self::LoadCell(_) | Self::LoadUpvalue(_) => Effects::READS_HEAP | Effects::MAY_THROW,

            Self::LoadGlobal(_) => Effects::READS_HEAP,

            Self::StoreCell { .. } | Self::StoreUpvalue { .. } | Self::StoreGlobal { .. } => {
                Effects::WRITES_HEAP
            }

            Self::MakeCell(_)
            | Self::MakeFunction { .. }
            | Self::NewArray { .. }
            | Self::NewObject { .. } => Effects::ALLOCATES,

            Self::Unary { op, .. } => match op {
                UnaryOp::Not | UnaryOp::Void | UnaryOp::TypeOf => Effects::PURE,
                UnaryOp::BitwiseNot
                | UnaryOp::Plus
                | UnaryOp::Negate
                | UnaryOp::ToNumeric
                | UnaryOp::Increment
                | UnaryOp::Decrement => Effects::UNKNOWN,
            },

            Self::Binary { op, .. } => match op {
                BinaryOp::StrictEqual | BinaryOp::StrictNotEqual => Effects::PURE,
                BinaryOp::BitwiseAnd
                | BinaryOp::BitwiseOr
                | BinaryOp::BitwiseXor
                | BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Remainder
                | BinaryOp::Exponentiate
                | BinaryOp::LessThan
                | BinaryOp::LessThanOrEqual
                | BinaryOp::GreaterThan
                | BinaryOp::GreaterThanOrEqual
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight
                | BinaryOp::UnsignedShiftRight => Effects::UNKNOWN,
                BinaryOp::In
                | BinaryOp::InstanceOf
                | BinaryOp::LooseEqual
                | BinaryOp::LooseNotEqual => Effects::UNKNOWN,
            },

            Self::GetProperty { .. } => {
                Effects::READS_HEAP
                    | Effects::MAY_THROW
                    | Effects::CALLS_USER_CODE
                    | Effects::ALLOCATES
                    | Effects::WRITES_HEAP
            }

            Self::SetProperty { .. } | Self::SetPropertyStrict { .. } => {
                Effects::WRITES_HEAP
                    | Effects::MAY_THROW
                    | Effects::CALLS_USER_CODE
                    | Effects::ALLOCATES
                    | Effects::READS_HEAP
            }

            Self::GetComputed { .. } => {
                Effects::READS_HEAP
                    | Effects::MAY_THROW
                    | Effects::CALLS_USER_CODE
                    | Effects::ALLOCATES
                    | Effects::WRITES_HEAP
            }

            Self::SetComputed { .. } | Self::SetComputedStrict { .. } => {
                Effects::WRITES_HEAP
                    | Effects::MAY_THROW
                    | Effects::CALLS_USER_CODE
                    | Effects::ALLOCATES
                    | Effects::READS_HEAP
            }

            Self::Call { .. } | Self::CallMethod { .. } | Self::New { .. } => Effects::UNKNOWN,

            Self::Bridge { instruction, .. } => match instruction {
                Instruction::PushHole => Effects::PURE,
                Instruction::ThrowReference(_) => Effects::MAY_THROW,
                Instruction::LoadPrototype => Effects::READS_HEAP | Effects::MAY_THROW,
                Instruction::SetPrototype => Effects::WRITES_HEAP | Effects::MAY_THROW,
                Instruction::GetPrivate(_) => Effects::READS_HEAP | Effects::MAY_THROW,
                Instruction::SetPrivate(_) => Effects::WRITES_HEAP | Effects::MAY_THROW,
                Instruction::NewRegExp { .. } => Effects::ALLOCATES,
                Instruction::DeleteProperty(_)
                | Instruction::DeletePropertyStrict { .. }
                | Instruction::DeleteComputed
                | Instruction::DeleteComputedStrict { .. } => {
                    Effects::WRITES_HEAP | Effects::MAY_THROW | Effects::CALLS_USER_CODE
                }
                Instruction::DirectEval(_)
                | Instruction::DynamicImport
                | Instruction::DynamicImportWithOptions => Effects::UNKNOWN,
                _ => Effects::UNKNOWN,
            },
        }
    }

    /// Refine coercion effects only when the graph's type proof establishes numeric operands.
    /// Unknown values must retain the full effect set: ToPrimitive/ToNumeric can invoke user
    /// code, allocate, mutate, or throw. Numeric specialization is safe because it bypasses that
    /// conversion path, and is shared by DCE, LICM, and parallel-map admission.
    pub fn effects_with_proven_numbers(&self, proven_numbers: &HashSet<ValueId>) -> Effects {
        let numeric = |value: &ValueId| proven_numbers.contains(value);
        let proven = match self {
            Self::Unary { op, value } => {
                matches!(
                    op,
                    UnaryOp::BitwiseNot
                        | UnaryOp::Plus
                        | UnaryOp::Negate
                        | UnaryOp::ToNumeric
                        | UnaryOp::Increment
                        | UnaryOp::Decrement
                ) && numeric(value)
            }
            Self::Binary { op, left, right } => {
                matches!(
                    op,
                    BinaryOp::BitwiseAnd
                        | BinaryOp::BitwiseOr
                        | BinaryOp::BitwiseXor
                        | BinaryOp::Add
                        | BinaryOp::Subtract
                        | BinaryOp::Multiply
                        | BinaryOp::Divide
                        | BinaryOp::Remainder
                        | BinaryOp::Exponentiate
                        | BinaryOp::LessThan
                        | BinaryOp::LessThanOrEqual
                        | BinaryOp::GreaterThan
                        | BinaryOp::GreaterThanOrEqual
                        | BinaryOp::ShiftLeft
                        | BinaryOp::ShiftRight
                        | BinaryOp::UnsignedShiftRight
                        | BinaryOp::LooseEqual
                        | BinaryOp::LooseNotEqual
                ) && numeric(left)
                    && numeric(right)
            }
            _ => false,
        };
        if proven {
            Effects::PURE
        } else {
            self.effects()
        }
    }
}

#[cfg(test)]
mod effect_tests {
    use super::*;

    #[test]
    fn unknown_contains_every_declared_effect() {
        assert_eq!(Effects::UNKNOWN, Effects::all());
        assert!(Effects::PURE.is_empty());
    }

    #[test]
    fn untrusted_bits_are_rejected() {
        let unknown_bit = 1 << 15;
        assert!(Effects::from_bits(unknown_bit).is_none());
    }

    #[test]
    fn granular_effects_distinguish_pure_and_effectful_ops() {
        assert_eq!(GraphOp::LoadLocal(0).effects(), Effects::PURE);
        assert_eq!(
            GraphOp::StoreLocal {
                slot: 0,
                value: ValueId(0)
            }
            .effects(),
            Effects::PURE
        );
        assert_eq!(
            GraphOp::CheckTdz {
                slot: 0,
                value: ValueId(0)
            }
            .effects(),
            Effects::MAY_THROW
        );
        assert_eq!(
            GraphOp::LoadCell(0).effects(),
            Effects::READS_HEAP | Effects::MAY_THROW
        );
        assert_eq!(
            GraphOp::StoreCell {
                slot: 0,
                value: ValueId(0)
            }
            .effects(),
            Effects::WRITES_HEAP
        );
        assert_eq!(GraphOp::MakeCell(0).effects(), Effects::ALLOCATES);
        assert_eq!(
            GraphOp::NewArray {
                elements: vec![],
                site: SiteId(0)
            }
            .effects(),
            Effects::ALLOCATES
        );
        assert_eq!(
            GraphOp::Unary {
                op: UnaryOp::TypeOf,
                value: ValueId(0)
            }
            .effects(),
            Effects::PURE
        );
        assert!(
            GraphOp::GetProperty {
                object: ValueId(0),
                name: 0,
                site: SiteId(0)
            }
            .effects()
            .contains(Effects::MAY_THROW | Effects::READS_HEAP)
        );
    }

    #[test]
    fn unknown_coercions_cannot_be_eliminated_as_pure() {
        for op in [
            UnaryOp::BitwiseNot,
            UnaryOp::Plus,
            UnaryOp::Negate,
            UnaryOp::ToNumeric,
            UnaryOp::Increment,
            UnaryOp::Decrement,
        ] {
            assert_eq!(
                GraphOp::Unary {
                    op,
                    value: ValueId(0),
                }
                .effects(),
                Effects::UNKNOWN
            );
        }
        for op in [
            BinaryOp::Add,
            BinaryOp::Subtract,
            BinaryOp::Multiply,
            BinaryOp::LessThan,
            BinaryOp::BitwiseAnd,
        ] {
            assert_eq!(
                GraphOp::Binary {
                    op,
                    left: ValueId(0),
                    right: ValueId(1),
                }
                .effects(),
                Effects::UNKNOWN
            );
        }
        assert_eq!(
            GraphOp::MarkUninitialized(0).effects(),
            Effects::WRITES_HEAP
        );
        let proven = [ValueId(0), ValueId(1)].into_iter().collect();
        assert_eq!(
            GraphOp::Binary {
                op: BinaryOp::Add,
                left: ValueId(0),
                right: ValueId(1),
            }
            .effects_with_proven_numbers(&proven),
            Effects::PURE
        );
        assert_eq!(
            GraphOp::Binary {
                op: BinaryOp::Add,
                left: ValueId(0),
                right: ValueId(2),
            }
            .effects_with_proven_numbers(&proven),
            Effects::UNKNOWN
        );
    }
}

/// Control-flow terminator for a basic block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminator {
    Jump(BlockId),
    Branch {
        condition: ValueId,
        true_block: BlockId,
        false_block: BlockId,
    },
    BranchNotNullish {
        condition: ValueId,
        true_block: BlockId,
        false_block: BlockId,
    },
    Return(Option<ValueId>),
    Throw(ValueId),
    /// A `yield` or an `await`: hand `value` out, keep `spilled` -- the operand stack beneath
    /// it -- in the frame, and come back at `resume` with what the caller sends. The locals are
    /// already in the frame's array; nothing else can be live across a suspend, because the
    /// bytecode has no other state.
    Suspend {
        value: ValueId,
        resume: BlockId,
        spilled: Vec<ValueId>,
        pc: u32,
        awaiting: bool,
        delegate: Option<(ValueId, BlockId, u32)>,
    },
    Unreachable,
}

/// A basic block in the Control Flow Graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub id: BlockId,
    pub operations: Vec<(ValueId, GraphOp)>,
    pub terminator: Terminator,
    pub predecessors: Vec<BlockId>,
    pub successors: Vec<BlockId>,
    pub exception_handler: Option<BlockId>,
}

impl BasicBlock {
    /// True if this block is the entry block or has at least one incoming predecessor edge.
    #[inline]
    pub fn is_reachable_from_entry(&self, entry: BlockId) -> bool {
        self.id == entry || !self.predecessors.is_empty()
    }
}

/// A statically known, non-escaping object allocation eligible for scalar replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarObject {
    /// The SSA value of the allocation (`GraphOp::NewObject`).
    pub alloc_id: ValueId,
    /// Property names in canonical sorted order, mapping name -> field index.
    pub fields: BTreeMap<String, usize>,
    /// Map from constant pool name index (`u32`) to field index.
    pub field_by_name_index: HashMap<u32, usize>,
    /// Initial values for each field in `fields` (from `NewObject.pairs`), mapping field index -> ValueId.
    pub initial_values: HashMap<usize, ValueId>,
    /// All ValueIds written to this field (initial values and SetProperty values).
    pub field_written_values: HashMap<usize, Vec<ValueId>>,
}

/// The complete SSA Value Graph for a compiled function or module top-level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionGraph {
    pub name: Option<String>,
    pub parameter_count: u32,
    pub local_count: u32,
    pub blocks: Vec<BasicBlock>,
    pub entry_block: BlockId,
    pub next_value_id: u32,
    pub next_site_id: u32,
    /// The SSA value each local slot holds on entry, in slot order. The emitter defines these
    /// from `locals[]` after the parameters are bound; without that definition a parameter read
    /// through the graph was `undefined`.
    pub initial_locals: Vec<ValueId>,
    /// Where each value came from: the bytecode index and the label a refusal note names, the
    /// same label `instruction_diagnostic` gives the instruction. Recorded once, at the walk.
    pub origins: HashMap<ValueId, Origin>,
    /// The block where parameter binding ends in a body whose binding runs at the call (a
    /// generator): the call stops there and the first `next` resumes there.
    pub seam_block: Option<BlockId>,
    /// Non-escaping object allocations identified by escape analysis.
    pub non_escaping_objects: BTreeSet<ValueId>,
    /// Metadata for each scalar-replaced object allocation.
    pub scalar_objects: HashMap<ValueId, ScalarObject>,
    /// Maps each alias ValueId (e.g. from StoreLocal, CheckTdz) to its root allocation ValueId.
    pub scalar_alias_map: HashMap<ValueId, ValueId>,
    /// Objects that do not escape *if* their spread source turns out to be a plain shaped
    /// object at run time: a literal with one `...source` and constant-key defines. The fast
    /// clone sinks them under that guard and exits to the generic clone, which allocates,
    /// when it fails; the generic clone never sees these maps.
    pub spread_scalar_objects: HashMap<ValueId, ScalarObject>,
    pub spread_alias_map: HashMap<ValueId, ValueId>,
    /// The spread bridge, its source, and the define bridges of each speculatively sunk object.
    pub spread_sinks: HashMap<ValueId, SpreadSink>,
}

/// A speculatively sunk literal with a spread: `{ ...source, k: v }` whose object never
/// escapes. Its fields hold what `source` had at the spread, overwritten by the defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpreadSink {
    pub alloc: ValueId,
    pub spread: ValueId,
    pub source: ValueId,
    pub defines: Vec<ValueId>,
}

/// The bytecode an operation was lowered from, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    pub pc: u32,
    pub label: String,
}

impl FunctionGraph {
    pub fn new(name: Option<String>, parameter_count: u32, local_count: u32) -> Self {
        Self {
            name,
            parameter_count,
            local_count,
            blocks: Vec::new(),
            entry_block: BlockId(0),
            next_value_id: 0,
            next_site_id: 0,
            initial_locals: Vec::new(),
            origins: HashMap::new(),
            seam_block: None,
            non_escaping_objects: BTreeSet::new(),
            scalar_objects: HashMap::new(),
            scalar_alias_map: HashMap::new(),
            spread_scalar_objects: HashMap::new(),
            spread_alias_map: HashMap::new(),
            spread_sinks: HashMap::new(),
        }
    }

    pub fn is_non_escaping(&self, vid: ValueId) -> bool {
        self.non_escaping_objects.contains(&vid)
    }

    pub fn mark_non_escaping_objects(&mut self, program: &Program) {
        self.mark_non_escaping_objects_with_extra_slots(program, None);
    }

    pub fn mark_non_escaping_objects_with_extra_slots(
        &mut self,
        program: &Program,
        extra_slots: Option<&HashSet<u32>>,
    ) {
        let res = escape_analysis_with(self, program, false, extra_slots).0;
        self.non_escaping_objects = res.non_escaping;
        self.scalar_objects = res.scalar_objects;
        self.scalar_alias_map = res.alias_map;
        // The same analysis with a spread allowed: what it finds beyond the unconditional
        // result is sunk speculatively, by the fast clone alone.
        let (with_spread, sinks) = escape_analysis_with(self, program, true, extra_slots);
        self.spread_scalar_objects = with_spread
            .scalar_objects
            .into_iter()
            .filter(|(alloc, _)| {
                !self.scalar_objects.contains_key(alloc) && sinks.contains_key(alloc)
            })
            .collect();
        let sunk: HashSet<ValueId> = self.spread_scalar_objects.keys().copied().collect();
        self.spread_alias_map = with_spread
            .alias_map
            .into_iter()
            .filter(|(_, root)| sunk.contains(root))
            .collect();
        self.spread_sinks = sinks
            .into_iter()
            .filter(|(alloc, _)| sunk.contains(alloc))
            .collect();
    }

    pub fn alloc_value(&mut self) -> ValueId {
        let id = self.next_value_id;
        self.next_value_id += 1;
        ValueId(id)
    }

    pub fn alloc_site(&mut self) -> SiteId {
        let id = self.next_site_id;
        self.next_site_id += 1;
        SiteId(id)
    }

    pub fn block(&self, id: BlockId) -> Option<&BasicBlock> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn block_mut(&mut self, id: BlockId) -> Option<&mut BasicBlock> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    pub fn summary(&self) -> crate::summary::FunctionSummary {
        crate::summary::summarize_graph(self)
    }
}

/// Abstract state tracking during bytecode-to-SSA graph interpretation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AbstractFrame {
    stack: Vec<ValueId>,
    locals: HashMap<u32, ValueId>,
}

/// What a value is *proven* to be, with no guess anywhere.
///
/// A literal is its own kind; arithmetic on two numbers is a number; a comparison, `!`, `in`
/// and `instanceof` are booleans whatever their operands; `typeof` is a string; an array
/// literal is an object; a function expression is callable; a phi is a kind when every input
/// is that kind. Nothing else is proven, so a parameter, a property read or a call result never
/// is, and no guard is needed for anything here: the proof is by construction.
///
/// This is the optimistic fixpoint: every value starts unconstrained, is lowered to what its
/// operation and operands allow, and settles. Starting unconstrained is what lets a loop's
/// induction variable be proven from its literal start through its own increment: the phi's
/// back-edge input is defined later in block order and would otherwise be judged before it
/// existed.
///
/// There is deliberately no per-operation type stored on the graph: the one that used to be
/// there marked `a - b` a number without looking at `a` or `b`, which is wrong for bigints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proven {
    Int,
    Number,
    Boolean,
    String,
    Object,
    Callable,
    Undefined,
    Null,
}

impl Proven {
    pub fn is_number(&self) -> bool {
        matches!(self, Proven::Int | Proven::Number)
    }

    pub fn is_int(&self) -> bool {
        matches!(self, Proven::Int)
    }
}

fn default_profile_version() -> u32 {
    1
}

/// Recorded type profile from a `--profile native-profile` execution (Step 3.2).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct TypeProfile {
    #[serde(default = "default_profile_version")]
    pub version: u32,
    #[serde(default)]
    pub functions: Vec<FunctionTypeProfile>,
    #[serde(default)]
    pub sites: Vec<SiteTypeProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct FunctionTypeProfile {
    pub index: usize,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub parameters: Vec<String>,
    #[serde(default)]
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct SiteTypeProfile {
    pub id: u32,
    pub tag: String,
}

impl TypeProfile {
    pub fn parse_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn function(&self, index: usize) -> Option<&FunctionTypeProfile> {
        self.functions.iter().find(|f| f.index == index)
    }

    pub fn site_tag(&self, id: u32) -> Option<&str> {
        self.sites
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.tag.as_str())
    }
}

/// An induction variable proven to govern a loop.
#[derive(Debug, Clone, PartialEq)]
pub struct InductionVariable {
    pub phi: ValueId,
    pub init: ValueId,
    pub step_op: ValueId,
    pub step: i64,
    pub bound: ValueId,
    pub bound_val: Option<i64>,
    pub min: i64,
    pub max: i64,
    pub non_negative: bool,
}

/// Helper to identify constant integers within safe integer range [-2^53 + 1, 2^53 - 1].
pub fn as_safe_integer(constant: &Constant) -> Option<i64> {
    if let Constant::Number(bits) = constant {
        let val = f64::from_bits(*bits);
        if val.is_finite()
            && val.floor() == val
            && (-9007199254740991.0..=9007199254740991.0).contains(&val)
            && (val != 0.0 || *bits == 0)
        {
            return Some(val as i64);
        }
    }
    None
}

/// Find loop induction variables in `graph`.
///
/// An induction variable starts at an integer `init`, is stepped in each iteration by an integer
/// `step`, and is bounded by `bound` within safe integer range `[-2^53 + 1, 2^53 - 1]`.
/// `length_bounds` are values known to be an array's `length`: a loop bounded by one runs at
/// most 2^32 - 1 times, so an induction variable stepping toward it stays a safe integer and
/// needs no overflow check, exactly as with a constant bound.
pub fn find_induction_variables(
    graph: &FunctionGraph,
    length_bounds: &HashSet<ValueId>,
) -> HashMap<ValueId, InductionVariable> {
    let mut result = HashMap::new();
    let defs: HashMap<ValueId, (BlockId, &GraphOp)> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(move |(v, op)| (*v, (b.id, op))))
        .collect();

    let resolve_alias = |mut v: ValueId| -> ValueId {
        for _ in 0..10 {
            match defs.get(&v) {
                Some((
                    _,
                    GraphOp::Unary {
                        op: UnaryOp::ToNumeric | UnaryOp::Plus,
                        value,
                    },
                )) => {
                    v = *value;
                }
                Some((_, GraphOp::StoreLocal { value, .. })) => {
                    v = *value;
                }
                Some((_, GraphOp::CheckTdz { value, .. })) => {
                    v = *value;
                }
                _ => break,
            }
        }
        v
    };

    let int_const = |vid: ValueId| -> Option<i64> {
        let resolved = resolve_alias(vid);
        if let Some((_, GraphOp::Const { constant, .. })) = defs.get(&resolved) {
            as_safe_integer(constant)
        } else {
            None
        }
    };

    for block in &graph.blocks {
        if block.predecessors.len() < 2 {
            continue;
        }
        let condition_vid = match block.terminator {
            Terminator::Branch { condition, .. } => condition,
            _ => continue,
        };
        let (cmp_op, cmp_left, cmp_right) = match defs.get(&resolve_alias(condition_vid)) {
            Some((_, GraphOp::Binary { op, left, right })) => (*op, *left, *right),
            _ => continue,
        };

        for &(phi_vid, ref op) in &block.operations {
            let inputs = match op {
                GraphOp::Phi { inputs } if inputs.len() == 2 => inputs,
                _ => continue,
            };

            // One input from entry (predecessor id < block.id), one from back-edge (predecessor id >= block.id)
            let (init_vid, step_vid) = if inputs[0].0.0 < block.id.0 && inputs[1].0.0 >= block.id.0
            {
                (inputs[0].1, inputs[1].1)
            } else if inputs[1].0.0 < block.id.0 && inputs[0].0.0 >= block.id.0 {
                (inputs[1].1, inputs[0].1)
            } else {
                continue;
            };

            let init_val = match int_const(init_vid) {
                Some(v) => v,
                None => continue,
            };

            let resolved_step = resolve_alias(step_vid);
            let mut step_sources = Vec::new();
            let step = match defs.get(&resolved_step) {
                Some((_, GraphOp::Binary { op, left, right })) => {
                    let s_left = resolve_alias(*left);
                    let s_right = resolve_alias(*right);
                    if s_left == phi_vid && matches!(op, BinaryOp::Add | BinaryOp::Subtract) {
                        let s_val = match int_const(*right) {
                            Some(v) => v,
                            None => continue,
                        };
                        step_sources.push(*left);
                        if *op == BinaryOp::Add { s_val } else { -s_val }
                    } else if s_right == phi_vid && *op == BinaryOp::Add {
                        let s_val = match int_const(*left) {
                            Some(v) => v,
                            None => continue,
                        };
                        step_sources.push(*right);
                        s_val
                    } else {
                        continue;
                    }
                }
                Some((_, GraphOp::Unary { op, value })) => {
                    let s_val = resolve_alias(*value);
                    if s_val == phi_vid {
                        step_sources.push(*value);
                        match op {
                            UnaryOp::Increment => 1,
                            UnaryOp::Decrement => -1,
                            _ => continue,
                        }
                    } else {
                        continue;
                    }
                }
                _ => continue,
            };

            if step == 0 {
                continue;
            }

            let (is_phi_left, bound_vid) = if resolve_alias(cmp_left) == phi_vid {
                (true, cmp_right)
            } else if resolve_alias(cmp_right) == phi_vid {
                (false, cmp_left)
            } else {
                continue;
            };

            let bound_val = int_const(bound_vid);
            // An array's length is below 2^32: the same arithmetic as a constant bound of that
            // size, with the bound itself left symbolic for the comparison.
            let effective_bound = bound_val.or_else(|| {
                (step > 0 && length_bounds.contains(&resolve_alias(bound_vid)))
                    .then_some(4_294_967_295i64)
            });
            let mut valid = false;
            let mut min = init_val;
            let mut max = init_val;
            let mut non_neg = init_val >= 0;

            if let Some(b_val) = effective_bound {
                if step > 0 {
                    let continues_on_less = (is_phi_left
                        && matches!(cmp_op, BinaryOp::LessThan | BinaryOp::LessThanOrEqual))
                        || (!is_phi_left
                            && matches!(
                                cmp_op,
                                BinaryOp::GreaterThan | BinaryOp::GreaterThanOrEqual
                            ));
                    if continues_on_less && init_val <= b_val {
                        min = init_val;
                        max = (b_val + step).max(init_val);
                        valid = min >= -9007199254740991 && max <= 9007199254740991;
                        non_neg = min >= 0;
                    }
                } else {
                    let continues_on_greater = (is_phi_left
                        && matches!(cmp_op, BinaryOp::GreaterThan | BinaryOp::GreaterThanOrEqual))
                        || (!is_phi_left
                            && matches!(cmp_op, BinaryOp::LessThan | BinaryOp::LessThanOrEqual));
                    if continues_on_greater && init_val >= b_val {
                        max = init_val;
                        min = (b_val + step).min(init_val);
                        valid = min >= -9007199254740991 && max <= 9007199254740991;
                        non_neg = min >= 0;
                    }
                }
            }

            if valid {
                let iv = InductionVariable {
                    phi: phi_vid,
                    init: init_vid,
                    step_op: step_vid,
                    step,
                    bound: bound_vid,
                    bound_val,
                    min,
                    max,
                    non_negative: non_neg,
                };
                result.insert(phi_vid, iv.clone());
                result.insert(step_vid, iv.clone());
                result.insert(resolved_step, iv.clone());

                let mut cur = step_vid;
                while cur != resolved_step {
                    result.insert(cur, iv.clone());
                    match defs.get(&cur) {
                        Some((
                            _,
                            GraphOp::Unary {
                                op: UnaryOp::ToNumeric | UnaryOp::Plus,
                                value: next,
                            },
                        )) => cur = *next,
                        Some((_, GraphOp::StoreLocal { value: next, .. })) => cur = *next,
                        Some((_, GraphOp::CheckTdz { value: next, .. })) => cur = *next,
                        _ => break,
                    }
                }

                for s in step_sources {
                    let mut cur = s;
                    while cur != phi_vid {
                        result.insert(cur, iv.clone());
                        match defs.get(&cur) {
                            Some((
                                _,
                                GraphOp::Unary {
                                    op: UnaryOp::ToNumeric | UnaryOp::Plus,
                                    value: next,
                                },
                            )) => cur = *next,
                            Some((_, GraphOp::StoreLocal { value: next, .. })) => cur = *next,
                            Some((_, GraphOp::CheckTdz { value: next, .. })) => cur = *next,
                            _ => break,
                        }
                    }
                }

                let mut cur = if is_phi_left { cmp_left } else { cmp_right };
                while cur != phi_vid {
                    result.insert(cur, iv.clone());
                    match defs.get(&cur) {
                        Some((
                            _,
                            GraphOp::Unary {
                                op: UnaryOp::ToNumeric | UnaryOp::Plus,
                                value: next,
                            },
                        )) => cur = *next,
                        Some((_, GraphOp::StoreLocal { value: next, .. })) => cur = *next,
                        Some((_, GraphOp::CheckTdz { value: next, .. })) => cur = *next,
                        _ => break,
                    }
                }
            }
        }
    }
    result
}

pub fn proven_types(graph: &FunctionGraph) -> HashMap<ValueId, Proven> {
    proven_types_with(graph, &HashMap::new())
}

/// `proven_types` with `assumptions` taken as given: a value the unit has proven by other
/// means -- a typed entry's parameter, a direct call whose typed entry returns a number -- has
/// its verdict here rather than one computed from its operation.
pub fn proven_types_with(
    graph: &FunctionGraph,
    assumptions: &HashMap<ValueId, Proven>,
) -> HashMap<ValueId, Proven> {
    proven_types_with_bounds(graph, assumptions, &std::collections::HashSet::new())
}

/// `proven_types_with` with explicit `length_bounds` for induction variable discovery.
pub fn proven_types_with_bounds(
    graph: &FunctionGraph,
    assumptions: &HashMap<ValueId, Proven>,
    length_bounds: &std::collections::HashSet<ValueId>,
) -> HashMap<ValueId, Proven> {
    let ivs = find_induction_variables(graph, length_bounds);
    // A resume target's stack phis also take what a suspend spilled and what the caller sent,
    // neither of which is an input here, so they prove nothing.
    let resume_targets: std::collections::HashSet<BlockId> = graph
        .blocks
        .iter()
        .flat_map(|b| match &b.terminator {
            Terminator::Suspend {
                resume, delegate, ..
            } => {
                if let Some((_, exit, _)) = delegate {
                    vec![*resume, *exit]
                } else {
                    vec![*resume]
                }
            }
            _ => vec![],
        })
        .collect();
    // `None` in `state` is "unconstrained so far"; an absent key is "proven nothing".
    let mut state: HashMap<ValueId, Option<Proven>> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(vid, _)| (*vid, None)))
        .collect();
    for (&vid, &kind) in assumptions {
        state.insert(vid, Some(kind));
    }
    let number = |state: &HashMap<ValueId, Option<Proven>>, v: &ValueId| {
        matches!(
            state.get(v),
            Some(None) | Some(Some(Proven::Number | Proven::Int))
        )
    };
    let int = |state: &HashMap<ValueId, Option<Proven>>, v: &ValueId| {
        matches!(state.get(v), Some(None) | Some(Some(Proven::Int)))
    };
    let is_non_zero = |v: &ValueId| -> bool {
        for b in &graph.blocks {
            for (vid, op) in &b.operations {
                if vid == v
                    && let GraphOp::Const { constant, .. } = op
                    && let Some(n) = as_safe_integer(constant)
                {
                    return n != 0;
                }
            }
        }
        if let Some(iv) = ivs.get(v) {
            return (iv.non_negative && iv.min > 0) || iv.max < 0;
        }
        false
    };
    loop {
        let mut changed = false;
        for block in &graph.blocks {
            for (vid, op) in &block.operations {
                if let Some(&assumed) = assumptions.get(vid) {
                    finish(&mut state, vid, Some(assumed), &mut changed);
                    continue;
                }
                if ivs.contains_key(vid) {
                    finish(&mut state, vid, Some(Proven::Int), &mut changed);
                    continue;
                }
                let verdict: Option<Proven> = match op {
                    GraphOp::Const { constant, .. } => Some(match constant {
                        Constant::Number(_) => {
                            if as_safe_integer(constant).is_some() {
                                Proven::Int
                            } else {
                                Proven::Number
                            }
                        }
                        Constant::String(_) => Proven::String,
                        Constant::Boolean(_) => Proven::Boolean,
                        Constant::Undefined => Proven::Undefined,
                        Constant::Null => Proven::Null,
                        Constant::BigInt(_) => {
                            finish(&mut state, vid, None, &mut changed);
                            continue;
                        }
                    }),
                    GraphOp::Binary { op, left, right } => match op {
                        BinaryOp::StrictEqual
                        | BinaryOp::StrictNotEqual
                        | BinaryOp::LooseEqual
                        | BinaryOp::LooseNotEqual
                        | BinaryOp::LessThan
                        | BinaryOp::LessThanOrEqual
                        | BinaryOp::GreaterThan
                        | BinaryOp::GreaterThanOrEqual
                        | BinaryOp::In
                        | BinaryOp::InstanceOf => Some(Proven::Boolean),
                        BinaryOp::BitwiseAnd
                        | BinaryOp::BitwiseOr
                        | BinaryOp::BitwiseXor
                        | BinaryOp::ShiftLeft
                        | BinaryOp::ShiftRight
                        | BinaryOp::UnsignedShiftRight => {
                            (number(&state, left) && number(&state, right)).then_some(Proven::Int)
                        }
                        BinaryOp::Remainder => {
                            if int(&state, left) && int(&state, right) && is_non_zero(right) {
                                Some(Proven::Int)
                            } else if number(&state, left) && number(&state, right) {
                                Some(Proven::Number)
                            } else {
                                None
                            }
                        }
                        BinaryOp::Add | BinaryOp::Subtract => {
                            if int(&state, left) && int(&state, right) {
                                Some(Proven::Int)
                            } else if number(&state, left) && number(&state, right) {
                                Some(Proven::Number)
                            } else {
                                None
                            }
                        }
                        _ => (number(&state, left) && number(&state, right))
                            .then_some(Proven::Number),
                    },
                    GraphOp::Unary { op, value } => match op {
                        UnaryOp::Not => Some(Proven::Boolean),
                        UnaryOp::TypeOf => Some(Proven::String),
                        UnaryOp::Void => Some(Proven::Undefined),
                        UnaryOp::BitwiseNot => number(&state, value).then_some(Proven::Int),
                        UnaryOp::Increment
                        | UnaryOp::Decrement
                        | UnaryOp::ToNumeric
                        | UnaryOp::Plus => {
                            if int(&state, value) {
                                Some(Proven::Int)
                            } else if number(&state, value) {
                                Some(Proven::Number)
                            } else {
                                None
                            }
                        }
                        _ => number(&state, value).then_some(Proven::Number),
                    },
                    GraphOp::NewArray { .. } | GraphOp::NewObject { .. } => Some(Proven::Object),
                    GraphOp::MakeFunction { .. } => Some(Proven::Callable),
                    GraphOp::IsReturnSignal(_) => Some(Proven::Boolean),
                    GraphOp::StoreLocal { value, .. }
                    | GraphOp::StoreCell { value, .. }
                    | GraphOp::StoreGlobal { value, .. }
                    | GraphOp::CheckTdz { value, .. } => state.get(value).copied().flatten(),
                    GraphOp::Phi { .. } if resume_targets.contains(&block.id) => None,
                    GraphOp::Phi { inputs } => {
                        let mut agreed: Option<Proven> = None;
                        let mut disagree = false;
                        for (_, input) in inputs {
                            match state.get(input).copied().flatten() {
                                None if state.contains_key(input) && state[input].is_none() => {}
                                None => {
                                    disagree = true;
                                    break;
                                }
                                Some(kind) => match agreed {
                                    None => agreed = Some(kind),
                                    Some(prior) if prior == kind => {}
                                    Some(prior) if prior.is_number() && kind.is_number() => {
                                        agreed = Some(Proven::Number);
                                    }
                                    Some(_) => {
                                        disagree = true;
                                        break;
                                    }
                                },
                            }
                        }
                        if disagree {
                            None
                        } else if agreed.is_none() {
                            // Every input is still unconstrained: stay unconstrained.
                            continue;
                        } else {
                            agreed
                        }
                    }
                    GraphOp::GetProperty { object, name, .. } => {
                        if let Some(&root) = graph.scalar_alias_map.get(object)
                            && let Some(scalar_obj) = graph.scalar_objects.get(&root)
                            && let Some(&field_idx) = scalar_obj.field_by_name_index.get(name)
                        {
                            if let Some(written) = scalar_obj.field_written_values.get(&field_idx) {
                                if !written.is_empty() && written.iter().all(|w| int(&state, w)) {
                                    Some(Proven::Int)
                                } else if !written.is_empty()
                                    && written.iter().all(|w| number(&state, w))
                                {
                                    Some(Proven::Number)
                                } else {
                                    None
                                }
                            } else {
                                Some(Proven::Undefined)
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                finish(&mut state, vid, verdict, &mut changed);
            }
        }
        if !changed {
            break;
        }
    }
    state
        .into_iter()
        .filter_map(|(vid, kind)| kind.map(|k| (vid, k)))
        .collect()
}

/// Record a verdict for `vid`: unconstrained becomes the verdict; a proven kind may only be
/// withdrawn, never changed, so the iteration is monotone and terminates.
fn finish(
    state: &mut HashMap<ValueId, Option<Proven>>,
    vid: &ValueId,
    verdict: Option<Proven>,
    changed: &mut bool,
) {
    match state.get(vid).copied() {
        Some(None) => {
            // Unconstrained: take the verdict, or mark "proven nothing" by removing the key.
            match verdict {
                Some(kind) => {
                    state.insert(*vid, Some(kind));
                    *changed = true;
                }
                None => {
                    state.remove(vid);
                    *changed = true;
                }
            }
        }
        Some(Some(Proven::Int)) if verdict == Some(Proven::Number) => {
            state.insert(*vid, Some(Proven::Number));
            *changed = true;
        }
        Some(Some(Proven::Number)) if verdict == Some(Proven::Int) => {
            // Already widened to Number; stay at Number.
        }
        Some(Some(prior)) if verdict != Some(prior) => {
            state.remove(vid);
            *changed = true;
        }
        None | Some(Some(_)) => {}
    }
}

/// The values proven to be numbers; what the emitter types as `double`.
pub fn proven_numbers(graph: &FunctionGraph) -> std::collections::HashSet<ValueId> {
    proven_numbers_with_assumptions(graph, &HashMap::new())
}

/// The values proven to be numbers given speculative assumptions about initial values/parameters.
pub fn proven_numbers_with_assumptions(
    graph: &FunctionGraph,
    assumptions: &HashMap<ValueId, Proven>,
) -> std::collections::HashSet<ValueId> {
    proven_types_with(graph, assumptions)
        .into_iter()
        .filter_map(|(vid, kind)| kind.is_number().then_some(vid))
        .collect()
}

/// The values proven to be integers; what the emitter can type as `int64_t`.
pub fn proven_ints(graph: &FunctionGraph) -> std::collections::HashSet<ValueId> {
    proven_ints_with_assumptions(graph, &HashMap::new())
}

/// The values proven to be integers given speculative assumptions about initial values/parameters.
pub fn proven_ints_with_assumptions(
    graph: &FunctionGraph,
    assumptions: &HashMap<ValueId, Proven>,
) -> std::collections::HashSet<ValueId> {
    proven_types_with(graph, assumptions)
        .into_iter()
        .filter_map(|(vid, kind)| (kind == Proven::Int).then_some(vid))
        .collect()
}

/// Parameter slots (0..proto.parameter_count) that can be speculatively assumed to be numbers.
///
/// In Step 2.1 of fastest-native-plan.md, versioning emits a guarded unboxed clone and generic
/// fallback clone for regions where parameter inputs arrive untyped but are consumed in
/// arithmetic, comparisons, or numeric phis.
pub fn candidate_number_parameters(graph: &FunctionGraph, proto: &FunctionProto) -> Vec<usize> {
    candidate_number_parameters_with_profile(graph, proto, None)
}

/// Parameter slots whose profiled object value is used as an array receiver.  The profile format
/// records arrays under the general `object` tag, so the emitted clone still performs the
/// authoritative Array-kind guard; this helper only narrows where paying for that clone can help.
///
/// Only simple positional parameters are eligible.  Defaults, destructuring, rest parameters,
/// mapped arguments, captured cells, and assignments to the parameter all change the relationship
/// between the entry argument and later local reads, so they deliberately stay generic.
pub fn candidate_array_parameters_with_profile(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    profile: Option<&FunctionTypeProfile>,
    program: &Program,
) -> Vec<usize> {
    if proto.parameter_count == 0
        || proto.generator
        || proto.asynchronous
        || proto.mapped_arguments
        || proto.declared_length != proto.parameter_count
        || proto.prologue_end != 0
        || profile.is_none()
    {
        return Vec::new();
    }
    let profile = profile.expect("checked above");
    let mut candidates = Vec::new();
    for (slot, tag) in profile.parameters.iter().enumerate() {
        if slot >= proto.parameter_count as usize
            || tag != "object"
            || graph.initial_locals.get(slot).is_none()
        {
            continue;
        }
        let assigned = graph.blocks.iter().any(|block| {
            block.operations.iter().any(|(_, op)| match op {
                GraphOp::StoreLocal { slot: written, .. }
                | GraphOp::StoreCell { slot: written, .. }
                | GraphOp::MakeCell(written) => *written as usize == slot,
                _ => false,
            })
        });
        if assigned {
            continue;
        }
        let mut values = HashSet::from([graph.initial_locals[slot]]);
        for block in &graph.blocks {
            for (vid, op) in &block.operations {
                if matches!(op, GraphOp::LoadLocal(load_slot) if *load_slot as usize == slot) {
                    values.insert(*vid);
                }
            }
        }
        // The parameter-array clone is only worth paying for when the function also reads the
        // Array length.  Indexed reads retain their existing generic fast tier inside this clone;
        // a pure computed read does not justify another entry clone.
        let uses_array_tier = graph.blocks.iter().any(|block| {
            block.operations.iter().any(|(_, op)| match op {
                GraphOp::GetProperty { object, name, .. } => {
                    values.contains(object)
                        && matches!(program.constants.get(*name as usize), Some(Constant::String(s)) if s == "length")
                }
                _ => false,
            })
        });
        if uses_array_tier {
            candidates.push(slot);
        }
    }
    candidates
}

/// Parameter slots (0..proto.parameter_count) that can be speculatively assumed to be numbers,
/// informed by TypeScript annotations (Step 3.1) and Type Profiles (Step 3.2).
pub fn candidate_number_parameters_with_profile(
    graph: &FunctionGraph,
    proto: &FunctionProto,
    profile: Option<&FunctionTypeProfile>,
) -> Vec<usize> {
    if proto.parameter_count == 0 || proto.generator || proto.asynchronous || proto.mapped_arguments
    {
        return Vec::new();
    }
    // Step 3.1: treat parameters annotated with `: number` as candidate numbers for guarded speculation
    let mut annotated_candidates = Vec::new();
    for slot in 0..proto.parameter_count as usize {
        if proto
            .parameter_type(slot)
            .is_some_and(|ann| ann.is_number())
            && graph.initial_locals.get(slot).is_some()
        {
            annotated_candidates.push(slot);
        }
    }
    // Step 3.2: treat parameters observed as numbers in type profile as candidate numbers for guarded speculation
    if let Some(prof) = profile {
        for (slot, tag) in prof.parameters.iter().enumerate() {
            if (tag == "number" || tag == "int")
                && (slot < proto.parameter_count as usize)
                && graph.initial_locals.get(slot).is_some()
                && !annotated_candidates.contains(&slot)
            {
                annotated_candidates.push(slot);
            }
        }
    }
    let baseline = proven_numbers(graph);
    let mut all_assumptions = HashMap::new();
    for slot in 0..proto.parameter_count as usize {
        if let Some(&vid) = graph.initial_locals.get(slot) {
            all_assumptions.insert(vid, Proven::Number);
        }
    }
    if all_assumptions.is_empty() {
        return annotated_candidates;
    }
    let speculative = proven_numbers_with_assumptions(graph, &all_assumptions);
    let mut useful = Vec::new();
    for slot in 0..proto.parameter_count as usize {
        if let Some(&param_vid) = graph.initial_locals.get(slot) {
            // Check if param_vid is consumed by any operation that benefits from being numeric
            let consumed = graph.blocks.iter().any(|b| {
                b.operations.iter().any(|(vid, op)| match op {
                    GraphOp::Binary { op, left, right }
                        if !matches!(op, BinaryOp::In | BinaryOp::InstanceOf) =>
                    {
                        if *left == param_vid || *right == param_vid {
                            let other = if *left == param_vid { right } else { left };
                            speculative.contains(other) || baseline.contains(other)
                        } else {
                            false
                        }
                    }
                    GraphOp::Unary { value, .. } => *value == param_vid,
                    GraphOp::Phi { inputs } => {
                        speculative.contains(vid) && inputs.iter().any(|(_, v)| *v == param_vid)
                    }
                    GraphOp::StoreLocal { value, .. } | GraphOp::StoreCell { value, .. } => {
                        *value == param_vid && speculative.contains(vid)
                    }
                    _ => false,
                })
            });
            if consumed {
                useful.push(slot);
            }
        }
    }
    let mut result = annotated_candidates;
    if !useful.is_empty() {
        // Verify that the useful parameters unlock numbers beyond the baseline and assumed params
        let useful_assumptions: HashMap<ValueId, Proven> = useful
            .iter()
            .filter_map(|&slot| {
                graph
                    .initial_locals
                    .get(slot)
                    .map(|&vid| (vid, Proven::Number))
            })
            .collect();
        let verified_spec = proven_numbers_with_assumptions(graph, &useful_assumptions);
        let has_body_numbers = verified_spec
            .iter()
            .any(|vid| !baseline.contains(vid) && !useful_assumptions.contains_key(vid));
        let has_numeric_comparison = graph.blocks.iter().any(|b| {
            b.operations.iter().any(|(_, op)| match op {
                GraphOp::Binary {
                    op:
                        BinaryOp::LessThan
                        | BinaryOp::LessThanOrEqual
                        | BinaryOp::GreaterThan
                        | BinaryOp::GreaterThanOrEqual
                        | BinaryOp::StrictEqual
                        | BinaryOp::StrictNotEqual
                        | BinaryOp::LooseEqual
                        | BinaryOp::LooseNotEqual,
                    left,
                    right,
                } => {
                    useful_assumptions.contains_key(left)
                        && (verified_spec.contains(right) || baseline.contains(right))
                        || useful_assumptions.contains_key(right)
                            && (verified_spec.contains(left) || baseline.contains(left))
                }
                _ => false,
            })
        });
        if has_body_numbers || has_numeric_comparison {
            for slot in useful {
                if !result.contains(&slot) {
                    result.push(slot);
                }
            }
        }
    }
    result.sort_unstable();
    result
}

/// The operand-stack effect of an instruction the graph lowers through `tail.rs`: how many
/// values it consumes and how many it leaves. The numbers are
/// the validator's (`stack_effect` in `validation.rs`), written as pops and pushes.
pub fn bridge_effect(instruction: &Instruction) -> Option<(usize, u8)> {
    Some(match instruction {
        Instruction::HostCall { argc, .. } => (usize::from(*argc), 1),
        Instruction::LoadArguments
        | Instruction::LoadTemplateObject(_)
        | Instruction::RestArguments(_)
        | Instruction::NewRegExp { .. }
        | Instruction::PushHole
        | Instruction::ThrowReference(_)
        | Instruction::LoadGlobalBindingOrUndefined(_)
        | Instruction::DeleteGlobalBinding(_) => (0, 1),
        Instruction::RequireGlobalBinding(_) | Instruction::DeclareGlobalBinding { .. } => (0, 0),
        Instruction::StoreGlobalBinding(_) | Instruction::StoreGlobalBindingStrict(_) => (1, 0),
        Instruction::Apply
        | Instruction::DefineMethod
        | Instruction::DefineAccessor { .. }
        | Instruction::DefineNamedProperty
        | Instruction::SetPrivate(_)
        | Instruction::SetSuperElementStrict { .. } => (3, 1),
        Instruction::SuperApply
        | Instruction::SpreadObject
        | Instruction::SpreadInto
        | Instruction::SetPrototype
        | Instruction::DefinePrivate { .. }
        | Instruction::GetPrivate(_)
        | Instruction::LoadSuperElement
        | Instruction::SetSuperPropertyStrict { .. }
        | Instruction::DeleteComputed
        | Instruction::DeleteComputedStrict { .. }
        | Instruction::ConstructSpread => (2, 1),
        Instruction::EnumerateKeys
        | Instruction::LoadPrototype
        | Instruction::SuperForward
        | Instruction::IterableToArray
        | Instruction::LoadSuperProperty(_)
        | Instruction::DeleteProperty(_)
        | Instruction::DeletePropertyStrict { .. }
        | Instruction::DynamicImport => (1, 1),
        Instruction::DynamicImportWithOptions => (2, 1),
        Instruction::SuperCall { argc } => (usize::from(*argc) + 1, 1),
        Instruction::CallComputed { argc } => (usize::from(*argc) + 2, 1),
        Instruction::ToPropertyKey => (2, 2),
        _ => return None,
    })
}

/// An instruction the graph builder does not lower. There is no other emitter, so the unit's
/// emission fails naming it; nothing is silently skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedInstruction {
    pub pc: usize,
    pub instruction: String,
}

impl std::fmt::Display for UnsupportedInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the value graph does not lower {} at pc {}",
            self.instruction, self.pc
        )
    }
}

pub fn build_function_graph(
    proto: &FunctionProto,
    program: &Program,
) -> Result<FunctionGraph, UnsupportedInstruction> {
    // The graph carries the effective name -- written, else inferred -- because
    // that is what the closure exposes and what the facts baseline records.
    // Carrying the written name alone dropped every inferred name from the
    // baseline the way a missing name would.
    let mut graph = FunctionGraph::new(
        proto
            .effective_name_units()
            .map(|units| units.to_lossy_string()),
        proto.parameter_count,
        proto.local_count,
    );

    let code = &proto.code;
    if code.is_empty() {
        let block_id = BlockId(0);
        graph.blocks.push(BasicBlock {
            id: block_id,
            operations: Vec::new(),
            terminator: Terminator::Return(None),
            predecessors: Vec::new(),
            successors: Vec::new(),
            exception_handler: None,
        });
        return Ok(graph);
    }

    // Phase 1: Identify Basic Block boundaries
    let mut block_starts = BTreeSet::new();
    block_starts.insert(0);

    for (pc, inst) in code.iter().enumerate() {
        match inst {
            Instruction::Jump(target) => {
                block_starts.insert(*target as usize);
                if pc + 1 < code.len() {
                    block_starts.insert(pc + 1);
                }
            }
            Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::JumpIfReturnSignal(target) => {
                block_starts.insert(*target as usize);
                if pc + 1 < code.len() {
                    block_starts.insert(pc + 1);
                }
            }
            Instruction::Return | Instruction::Throw => {
                if pc + 1 < code.len() {
                    block_starts.insert(pc + 1);
                }
            }
            Instruction::PushHandler(target) => {
                // The handler is a block of its own, and so is everything after the push: a
                // block's innermost handler is one static fact, decided by the pre-pass below
                // from the state at the block's end, so no block may straddle a push or a pop.
                block_starts.insert(*target as usize);
                block_starts.insert(pc);
                if pc + 1 < code.len() {
                    block_starts.insert(pc + 1);
                }
            }
            Instruction::PopHandler => {
                block_starts.insert(pc);
                if pc + 1 < code.len() {
                    block_starts.insert(pc + 1);
                }
            }
            Instruction::Yield | Instruction::Await if pc + 1 < code.len() => {
                block_starts.insert(pc + 1);
            }
            Instruction::YieldStar(target) => {
                block_starts.insert(*target as usize);
                if pc + 1 < code.len() {
                    block_starts.insert(pc + 1);
                }
            }
            _ => {}
        }
    }

    let suspends = proto.generator || proto.asynchronous;
    let binds_at_call =
        suspends && proto.prologue_end > 0 && (proto.prologue_end as usize) < code.len();
    if binds_at_call {
        block_starts.insert(proto.prologue_end as usize);
    }
    let jumps_to_zero = code.iter().any(|inst| {
        matches!(
            inst,
            Instruction::Jump(0)
                | Instruction::JumpIfFalse(0)
                | Instruction::JumpIfTruthy(0)
                | Instruction::JumpIfNotNullish(0)
                | Instruction::JumpIfReturnSignal(0)
                | Instruction::PushHandler(0)
                | Instruction::YieldStar(0)
        )
    });
    let block_offset = if jumps_to_zero { 1 } else { 0 };
    let block_starts_vec: Vec<usize> = block_starts.into_iter().collect();
    let pc_to_block: BTreeMap<usize, BlockId> = block_starts_vec
        .iter()
        .enumerate()
        .map(|(idx, &pc)| (pc, BlockId((idx + block_offset) as u32)))
        .collect();

    // Map each PC to its containing BlockId
    let pc_to_containing_block = |pc: usize| -> BlockId {
        let mut prev_block = BlockId(block_offset as u32);
        for (&start, &bid) in &pc_to_block {
            if start <= pc {
                prev_block = bid;
            } else {
                break;
            }
        }
        prev_block
    };

    // Phase 2: Compute predecessor/successor edges and active exception handlers
    let num_blocks = block_starts_vec.len() + block_offset;
    let mut block_ranges = Vec::with_capacity(block_starts_vec.len());
    for (i, &start) in block_starts_vec.iter().enumerate() {
        let end = if i + 1 < block_starts_vec.len() {
            block_starts_vec[i + 1]
        } else {
            code.len()
        };
        block_ranges.push((start, end));
    }

    let mut handler_stack = Vec::new();
    let mut block_handlers = vec![None; num_blocks];
    let handler_targets: HashSet<BlockId> = code
        .iter()
        .filter_map(|inst| match inst {
            Instruction::PushHandler(target) => pc_to_block.get(&(*target as usize)).copied(),
            _ => None,
        })
        .collect();
    for (i, &(start, end)) in block_ranges.iter().enumerate() {
        let b_idx = i + block_offset;
        for inst in &code[start..end] {
            match inst {
                Instruction::PushHandler(target) => {
                    handler_stack.push(*pc_to_block.get(&(*target as usize)).unwrap());
                }
                Instruction::PopHandler => {
                    handler_stack.pop();
                }
                _ => {}
            }
        }
        block_handlers[b_idx] = handler_stack.last().copied();
    }

    // Phase 3: Abstract Interpretation across basic blocks
    let mut block_entry_frames: HashMap<BlockId, AbstractFrame> = HashMap::new();
    let mut basic_blocks = Vec::with_capacity(num_blocks);

    // Initial frame for entry block (BlockId(0))
    let mut initial_locals = HashMap::new();
    for slot in 0..proto.local_count {
        let val = graph.alloc_value();
        initial_locals.insert(slot, val);
        graph.initial_locals.push(val);
    }
    block_entry_frames.insert(
        BlockId(0),
        AbstractFrame {
            stack: Vec::new(),
            locals: initial_locals.clone(),
        },
    );

    // Phase 2b: static predecessor counts, from each block's last instruction. A block with two
    // or more predecessors merges state, and a merge is a phi per local: without one, the first
    // frame to arrive was kept and every later predecessor was ignored, so a loop header compared
    // the pre-loop value forever and a local assigned in one branch of an `if` read its
    // pre-branch value after the join. Stack merges (`c ? a : b`) get their phis when the first
    // frame arrives, because only then is the depth known.
    let mut static_preds: Vec<Vec<BlockId>> = vec![Vec::new(); num_blocks];
    if jumps_to_zero {
        static_preds[1].push(BlockId(0));
    }
    for (i, &(start, end)) in block_ranges.iter().enumerate() {
        let b_idx = i + block_offset;
        let this = BlockId(b_idx as u32);
        let next = (i + 1 < block_ranges.len()).then(|| BlockId((b_idx + 1) as u32));
        let mut succs: Vec<BlockId> = Vec::new();
        let last = if end > start { code.get(end - 1) } else { None };
        match last {
            Some(Instruction::Jump(target)) => succs.push(pc_to_containing_block(*target as usize)),
            Some(
                Instruction::JumpIfFalse(target)
                | Instruction::JumpIfTruthy(target)
                | Instruction::JumpIfNotNullish(target)
                | Instruction::JumpIfReturnSignal(target),
            ) => {
                succs.push(pc_to_containing_block(*target as usize));
                succs.extend(next);
            }
            // Suspension edges are synthetic CFG edges. The body returns to its caller, but a
            // later generator call re-enters at the resume target through the dispatch switch.
            // Keeping those edges in the graph is essential: resume/delegate targets still have
            // real SSA joins (and therefore need one phi input for this activation) even though
            // the edge is implemented by the frame rather than a C goto.
            Some(Instruction::Return | Instruction::Throw) => {}
            Some(Instruction::Yield | Instruction::Await) => {
                succs.push(pc_to_containing_block(end));
            }
            Some(Instruction::YieldStar(target)) => {
                succs.push(pc_to_containing_block(end));
                succs.push(pc_to_containing_block(*target as usize));
            }
            _ => succs.extend(next),
        }
        // A protected block can leave through its handler from any throwing operation.
        succs.extend(block_handlers[b_idx]);
        for succ in succs {
            let list = &mut static_preds[succ.0 as usize];
            if !list.contains(&this) {
                list.push(this);
            }
        }
    }
    // A suspending body keeps its locals in the frame, where the runtime and every later call
    // find them, and reads them back at the start of every block: no local crosses a block in
    // SSA, so it has no local phis. Its operand stack does cross a suspend, spilled into the
    // frame, and comes back through the resume target's stack phis, which are known before the
    // walk: the depth at the yield, with the sent value on top. A pass over the verified
    // bytecode in order gives that depth, carrying a jump target's height from the first jump
    // to it and a handler's from its push.
    let mut resume_depths: HashMap<BlockId, usize> = HashMap::new();
    if suspends {
        let mut height_at: HashMap<usize, usize> = HashMap::new();
        let mut height = 0usize;
        let after = |inst: &Instruction, height: usize, pc: usize| {
            crate::validation::stack_effect(inst, height, pc)
                .map(|(h, _)| h)
                .unwrap_or(height)
        };
        for (pc, inst) in code.iter().enumerate() {
            if let Some(&known) = height_at.get(&pc) {
                height = known;
            }
            match inst {
                Instruction::Jump(target)
                | Instruction::JumpIfFalse(target)
                | Instruction::JumpIfTruthy(target)
                | Instruction::JumpIfNotNullish(target)
                | Instruction::JumpIfReturnSignal(target) => {
                    height = after(inst, height, pc);
                    height_at.entry(*target as usize).or_insert(height);
                }
                Instruction::PushHandler(target) => {
                    height_at.entry(*target as usize).or_insert(height + 1);
                }
                Instruction::Yield | Instruction::Await => {
                    // The yielded value leaves and the sent value arrives: the same height.
                    resume_depths.insert(pc_to_containing_block(pc + 1), height);
                }
                Instruction::YieldStar(target) => {
                    height = after(inst, height, pc);
                    // YieldStar consumes wrapped flag + iterator + yielded value and resumes with one sent
                    // value, so the post-instruction height is exactly the spilled depth plus
                    // the synthetic resume slot. Do not subtract again: that dropped the final
                    // phi input for delegates with a value already below the pair.
                    resume_depths.insert(pc_to_containing_block(pc + 1), height);
                    resume_depths.insert(pc_to_containing_block(*target as usize), height);
                    height_at.entry(*target as usize).or_insert(height);
                }
                other => height = after(other, height, pc),
            }
        }
    }
    let mut merges: MergeState = MergeState::default();
    for (b_idx, preds) in static_preds.iter().enumerate() {
        // A handler block merges nothing: a throw can happen mid-block, when the locals are
        // half stored, so its locals come from the array and its stack is fixed at the push.
        let this = BlockId(b_idx as u32);
        let resume_depth = resume_depths.get(&this).copied();
        if handler_targets.contains(&this) || (preds.len() < 2 && resume_depth.is_none()) {
            continue;
        }
        let block_id = BlockId(b_idx as u32);
        let locals: Vec<ValueId> = if suspends {
            Vec::new()
        } else {
            (0..proto.local_count)
                .map(|_| graph.alloc_value())
                .collect()
        };
        for &phi in &locals {
            merges.inputs.insert(phi, Vec::new());
        }
        let frame_locals: HashMap<u32, ValueId> = locals
            .iter()
            .enumerate()
            .map(|(slot, &v)| (slot as u32, v))
            .collect();
        block_entry_frames.insert(
            block_id,
            AbstractFrame {
                stack: Vec::new(),
                locals: frame_locals,
            },
        );
        merges.phis.insert(
            block_id,
            PhiSet {
                locals,
                // Stack phis are allocated from the first actual incoming frame. The bytecode
                // height walk identifies suspension targets, but cannot know the concrete stack
                // retained by a path after nested control flow. Deferring allocation prevents a
                // speculative extra phi from becoming an invalid zero-input join.
                stack: None,
            },
        );
    }

    let mut handler_caught: HashMap<BlockId, ValueId> = HashMap::new();
    if binds_at_call {
        // Reached by falling out of the binding, which stops there, and by the first `next`,
        // which starts there: both with an empty operand stack and the locals in the frame.
        graph.seam_block = Some(pc_to_containing_block(proto.prologue_end as usize));
    }
    if jumps_to_zero {
        let mut operations = Vec::new();
        for (slot, &vid) in graph.initial_locals.clone().iter().enumerate() {
            operations.push((vid, GraphOp::LoadLocal(slot as u32)));
        }
        let target = BlockId(1);
        let terminator = Terminator::Jump(target);
        let successors = vec![target];
        propagate_frame(
            &mut block_entry_frames,
            &mut merges,
            BlockId(0),
            target,
            &AbstractFrame {
                stack: Vec::new(),
                locals: initial_locals.clone(),
            },
            &mut graph,
            &mut operations,
        );
        basic_blocks.push(BasicBlock {
            id: BlockId(0),
            operations,
            terminator,
            predecessors: Vec::new(),
            successors,
            exception_handler: None,
        });
    }
    for (i, &(start, end)) in block_ranges.iter().enumerate() {
        let b_idx = i + block_offset;
        let block_id = BlockId(b_idx as u32);
        let mut frame = block_entry_frames
            .get(&block_id)
            .cloned()
            .unwrap_or_else(|| AbstractFrame {
                stack: Vec::new(),
                locals: HashMap::new(),
            });
        if suspends && b_idx != 0 {
            frame.locals.clear();
        }

        let mut operations = Vec::new();
        let mut terminator = Terminator::Unreachable;
        let mut successors = Vec::new();

        if b_idx == 0 {
            // The entry block defines what every slot holds on entry: the prologue's `locals[]`,
            // which is the bound parameters and `undefined`. A phi or a use elsewhere then has a
            // definition to point at; before this, a parameter read through the graph had none.
            for (slot, &vid) in graph.initial_locals.clone().iter().enumerate() {
                operations.push((vid, GraphOp::LoadLocal(slot as u32)));
            }
        }
        if let Some(set) = merges.phis.get(&block_id) {
            for &phi in &set.locals {
                operations.push((phi, GraphOp::Phi { inputs: Vec::new() }));
            }
        }
        if let Some(&caught) = handler_caught.get(&block_id) {
            operations.push((caught, GraphOp::CaughtValue));
            graph.origins.insert(
                caught,
                Origin {
                    pc: start as u32,
                    label: "CaughtValue".to_owned(),
                },
            );
        }

        for (offset, inst) in code[start..end].iter().enumerate() {
            let pc = start + offset;
            let ops_before = operations.len();
            match inst {
                Instruction::Const(c_idx) => {
                    let val = graph.alloc_value();
                    let constant = program
                        .constants
                        .get(*c_idx as usize)
                        .cloned()
                        .unwrap_or(Constant::Undefined);
                    operations.push((
                        val,
                        GraphOp::Const {
                            index: *c_idx,
                            constant,
                        },
                    ));
                    frame.stack.push(val);
                }
                Instruction::LoadLocal(slot) => {
                    let val = if let Some(&v) = frame.locals.get(slot) {
                        v
                    } else {
                        let v = graph.alloc_value();
                        operations.push((v, GraphOp::LoadLocal(*slot)));
                        frame.locals.insert(*slot, v);
                        v
                    };
                    frame.stack.push(val);
                }
                Instruction::StoreLocal(slot) => {
                    if let Some(val) = frame.stack.pop() {
                        frame.locals.insert(*slot, val);
                        let stored = graph.alloc_value();
                        operations.push((
                            stored,
                            GraphOp::StoreLocal {
                                slot: *slot,
                                value: val,
                            },
                        ));
                    }
                }
                Instruction::Pop => {
                    frame.stack.pop();
                }
                Instruction::Dup2 => {
                    let n = frame.stack.len();
                    if n >= 2 {
                        let a = frame.stack[n - 2];
                        let b = frame.stack[n - 1];
                        frame.stack.push(a);
                        frame.stack.push(b);
                    }
                }
                Instruction::Dup => {
                    if let Some(&top) = frame.stack.last() {
                        frame.stack.push(top);
                    }
                }
                Instruction::Unary(op) => {
                    let operand = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::Unary {
                            op: *op,
                            value: operand,
                        },
                    ));
                    frame.stack.push(res);
                }
                Instruction::Binary(op) => {
                    let right = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let left = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::Binary {
                            op: *op,
                            left,
                            right,
                        },
                    ));
                    frame.stack.push(res);
                }
                Instruction::Jump(target) => {
                    let target_block = *pc_to_block.get(&(*target as usize)).unwrap();
                    terminator = Terminator::Jump(target_block);
                    successors.push(target_block);
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        target_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                }
                Instruction::JumpIfFalse(target) => {
                    let cond = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let target_block = *pc_to_block.get(&(*target as usize)).unwrap();
                    let fallthrough_block = pc_to_containing_block(pc + 1);
                    terminator = Terminator::Branch {
                        condition: cond,
                        true_block: fallthrough_block,
                        false_block: target_block,
                    };
                    successors.push(fallthrough_block);
                    successors.push(target_block);
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        fallthrough_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        target_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                }
                Instruction::JumpIfTruthy(target) => {
                    let cond = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let target_block = *pc_to_block.get(&(*target as usize)).unwrap();
                    let fallthrough_block = pc_to_containing_block(pc + 1);
                    terminator = Terminator::Branch {
                        condition: cond,
                        true_block: target_block,
                        false_block: fallthrough_block,
                    };
                    successors.push(target_block);
                    successors.push(fallthrough_block);
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        target_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        fallthrough_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                }
                Instruction::JumpIfNotNullish(target) => {
                    let cond = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let target_block = *pc_to_block.get(&(*target as usize)).unwrap();
                    let fallthrough_block = pc_to_containing_block(pc + 1);
                    terminator = Terminator::BranchNotNullish {
                        condition: cond,
                        true_block: target_block,
                        false_block: fallthrough_block,
                    };
                    successors.push(target_block);
                    successors.push(fallthrough_block);
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        target_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        fallthrough_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                }
                Instruction::JumpIfReturnSignal(target) => {
                    let cond = frame
                        .stack
                        .last()
                        .copied()
                        .unwrap_or_else(|| graph.alloc_value());
                    let is_signal = graph.alloc_value();
                    operations.push((is_signal, GraphOp::IsReturnSignal(cond)));
                    let target_block = *pc_to_block.get(&(*target as usize)).unwrap();
                    let fallthrough_block = pc_to_containing_block(pc + 1);
                    terminator = Terminator::Branch {
                        condition: is_signal,
                        true_block: target_block,
                        false_block: fallthrough_block,
                    };
                    successors.push(target_block);
                    successors.push(fallthrough_block);
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        target_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        fallthrough_block,
                        &frame,
                        &mut graph,
                        &mut operations,
                    );
                }
                Instruction::Nop => {}
                Instruction::Return => {
                    let ret_val = frame.stack.pop();
                    terminator = Terminator::Return(ret_val);
                }
                Instruction::Throw => {
                    let ex_val = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    terminator = Terminator::Throw(ex_val);
                }
                Instruction::Call { argc } => {
                    let mut args = Vec::with_capacity(*argc as usize);
                    for _ in 0..*argc {
                        args.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    args.reverse();
                    let callee = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::Call { callee, args, site }));
                    frame.stack.push(result);
                }
                Instruction::CallMethod { name, argc } => {
                    let mut args = Vec::with_capacity(*argc as usize);
                    for _ in 0..*argc {
                        args.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    args.reverse();
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::CallMethod {
                            object,
                            name: *name,
                            args,
                            site,
                        },
                    ));
                    frame.stack.push(result);
                }
                Instruction::GetProperty(name) => {
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::GetProperty {
                            object,
                            name: *name,
                            site,
                        },
                    ));
                    frame.stack.push(result);
                }
                Instruction::SetProperty(name) => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::SetProperty {
                            object,
                            name: *name,
                            value,
                            site,
                        },
                    ));
                    frame.stack.push(result);
                }
                Instruction::SetPropertyStrict { name, strict } => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::SetPropertyStrict {
                            object,
                            name: *name,
                            value,
                            strict: *strict,
                            site,
                        },
                    ));
                    frame.stack.push(result);
                }
                Instruction::GetComputed => {
                    let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::GetComputed { object, key, site }));
                    frame.stack.push(result);
                }
                Instruction::SetComputed => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::SetComputed {
                            object,
                            key,
                            value,
                            site,
                        },
                    ));
                    frame.stack.push(result);
                }
                Instruction::SetComputedStrict { strict } => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::SetComputedStrict {
                            object,
                            key,
                            value,
                            strict: *strict,
                            site,
                        },
                    ));
                    frame.stack.push(result);
                }
                Instruction::MakeFunction(index) => {
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::MakeFunction {
                            index: *index,
                            site,
                        },
                    ));
                    frame.stack.push(result);
                }
                Instruction::NewArray { count } => {
                    let mut elements = Vec::with_capacity(*count as usize);
                    for _ in 0..*count {
                        elements.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    elements.reverse();
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::NewArray { elements, site }));
                    frame.stack.push(result);
                }
                Instruction::LoadThis => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadThis));
                    frame.stack.push(result);
                }
                Instruction::LoadNewTarget => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadNewTarget));
                    frame.stack.push(result);
                }
                Instruction::LoadUpvalue(index) => {
                    let res = graph.alloc_value();
                    operations.push((res, GraphOp::LoadUpvalue(*index)));
                    frame.stack.push(res);
                }
                Instruction::StoreUpvalue(index) => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::StoreUpvalue {
                            index: *index,
                            value,
                        },
                    ));
                }
                Instruction::LoadBuiltin(builtin) => {
                    let res = graph.alloc_value();
                    operations.push((res, GraphOp::LoadBuiltin(*builtin)));
                    frame.stack.push(res);
                }
                Instruction::New { argc } => {
                    let argc = usize::from(*argc);
                    let mut args = Vec::with_capacity(argc);
                    for _ in 0..argc {
                        args.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    args.reverse();
                    let callee = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    let site = graph.alloc_site();
                    operations.push((res, GraphOp::New { callee, args, site }));
                    frame.stack.push(res);
                }
                Instruction::NewObject { count } => {
                    let count = usize::try_from(*count).unwrap_or(0);
                    let mut pairs = Vec::with_capacity(count);
                    for _ in 0..count {
                        let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                        let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                        pairs.push((key, value));
                    }
                    pairs.reverse();
                    let res = graph.alloc_value();
                    let site = graph.alloc_site();
                    operations.push((res, GraphOp::NewObject { pairs, site }));
                    frame.stack.push(res);
                }
                Instruction::LoadCell(slot) => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadCell(*slot)));
                    frame.stack.push(result);
                }
                Instruction::StoreCell(slot) => {
                    if let Some(val) = frame.stack.pop() {
                        let res = graph.alloc_value();
                        operations.push((
                            res,
                            GraphOp::StoreCell {
                                slot: *slot,
                                value: val,
                            },
                        ));
                    }
                }
                Instruction::NewCell(slot) => {
                    let res = graph.alloc_value();
                    operations.push((res, GraphOp::MakeCell(*slot)));
                }
                Instruction::LoadGlobal(slot) => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadGlobal(*slot)));
                    frame.stack.push(result);
                }
                Instruction::StoreGlobal(slot) => {
                    if let Some(val) = frame.stack.pop() {
                        let res = graph.alloc_value();
                        operations.push((
                            res,
                            GraphOp::StoreGlobal {
                                slot: *slot,
                                value: val,
                            },
                        ));
                    }
                }
                Instruction::MarkUninitialized(slot) => {
                    let res = graph.alloc_value();
                    operations.push((res, GraphOp::MarkUninitialized(*slot)));
                    frame.locals.insert(*slot, res);
                }
                Instruction::Yield | Instruction::Await => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let spilled = frame.stack.clone();
                    let resume_block = pc_to_containing_block(pc + 1);
                    // The value sent by the next generator call is not available in this
                    // activation. Model it as a first-class unknown SSA value; the emitter
                    // restores the actual frame->sent value at the resume label.
                    let mut resumed = frame.clone();
                    let resume_value = graph.alloc_value();
                    operations.push((resume_value, GraphOp::ResumeValue));
                    resumed.stack.push(resume_value);
                    terminator = Terminator::Suspend {
                        value,
                        resume: resume_block,
                        spilled,
                        pc: pc as u32,
                        awaiting: matches!(inst, Instruction::Await),
                        delegate: None,
                    };
                    successors.push(resume_block);
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        resume_block,
                        &resumed,
                        &mut graph,
                        &mut operations,
                    );
                }
                Instruction::YieldStar(target) => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let iterator = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let _wrapped = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let spilled = frame.stack.clone();
                    let resume_block = pc_to_containing_block(pc + 1);
                    let exit_block = pc_to_containing_block(*target as usize);
                    // Both delegate suspension and delegate completion re-enter through the
                    // frame's sent slot. The value is unknown until that synthetic edge runs.
                    let mut resumed = frame.clone();
                    let resume_value = graph.alloc_value();
                    operations.push((resume_value, GraphOp::ResumeValue));
                    resumed.stack.push(resume_value);
                    terminator = Terminator::Suspend {
                        value,
                        resume: resume_block,
                        spilled,
                        pc: pc as u32,
                        awaiting: false,
                        delegate: Some((iterator, exit_block, *target)),
                    };
                    successors.push(resume_block);
                    if exit_block != resume_block {
                        successors.push(exit_block);
                    }
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        resume_block,
                        &resumed,
                        &mut graph,
                        &mut operations,
                    );
                    propagate_frame(
                        &mut block_entry_frames,
                        &mut merges,
                        block_id,
                        exit_block,
                        &resumed,
                        &mut graph,
                        &mut operations,
                    );
                }
                Instruction::PushHandler(target) => {
                    // The handler starts with the operand stack as it is here, plus the thrown
                    // value; its locals are read from the array, since a throw can land from
                    // any point of the protected region.
                    let handler = pc_to_containing_block(*target as usize);
                    let caught = graph.alloc_value();
                    let mut stack = frame.stack.clone();
                    stack.push(caught);
                    block_entry_frames.entry(handler).or_insert(AbstractFrame {
                        stack,
                        locals: HashMap::new(),
                    });
                    handler_caught.insert(handler, caught);
                }
                Instruction::PopHandler => {}
                other => {
                    let Some((pops, pushes)) = bridge_effect(other) else {
                        return Err(UnsupportedInstruction {
                            pc,
                            instruction: format!("{other:?}"),
                        });
                    };
                    let mut operands = Vec::with_capacity(pops);
                    for _ in 0..pops {
                        operands.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    operands.reverse();
                    let res = graph.alloc_value();
                    if pushes == 2 {
                        // `ToPropertyKey` leaves its object where it was and a converted key.
                        frame.stack.push(operands[0]);
                    }
                    operations.push((
                        res,
                        GraphOp::Bridge {
                            instruction: other.clone(),
                            operands,
                            pushes,
                        },
                    ));
                    if pushes > 0 {
                        frame.stack.push(res);
                    }
                }
            }
            if operations.len() > ops_before {
                let label = crate::instruction_diagnostic(inst, &program.constants);
                for (vid, _) in &operations[ops_before..] {
                    graph.origins.insert(
                        *vid,
                        Origin {
                            pc: pc as u32,
                            label: label.clone(),
                        },
                    );
                }
            }
        }

        // Fallthrough if block didn't terminate explicitly
        if terminator == Terminator::Unreachable && b_idx + 1 < num_blocks {
            let next_block = BlockId((b_idx + 1) as u32);
            terminator = Terminator::Jump(next_block);
            successors.push(next_block);
            propagate_frame(
                &mut block_entry_frames,
                &mut merges,
                block_id,
                next_block,
                &frame,
                &mut graph,
                &mut operations,
            );
        }

        if let Some(handler) = block_handlers[b_idx]
            && !successors.contains(&handler)
        {
            successors.push(handler);
        }
        basic_blocks.push(BasicBlock {
            id: block_id,
            operations,
            terminator,
            predecessors: Vec::new(),
            successors,
            exception_handler: block_handlers[b_idx],
        });
    }

    // Stack phis are allocated lazily from an actual incoming frame. Insert them now so this
    // remains true even for a back edge whose target block was emitted before that edge was
    // walked.
    for block in &mut basic_blocks {
        let Some(set) = merges.phis.get(&block.id) else {
            continue;
        };
        let existing: HashSet<ValueId> = block
            .operations
            .iter()
            .filter_map(|(value, op)| matches!(op, GraphOp::Phi { .. }).then_some(*value))
            .collect();
        let stack_phis: Vec<_> = set
            .stack
            .iter()
            .flatten()
            .copied()
            .filter(|value| !existing.contains(value))
            .map(|value| (value, GraphOp::Phi { inputs: Vec::new() }))
            .collect();
        if !stack_phis.is_empty() {
            block.operations.splice(0..0, stack_phis);
        }
    }

    // Phase 3b: fill the phi inputs collected while the predecessors were walked.
    for block in &mut basic_blocks {
        for (vid, op) in &mut block.operations {
            if let GraphOp::Phi { inputs } = op
                && let Some(collected) = merges.inputs.get(vid)
            {
                *inputs = collected.clone();
            }
        }
    }

    // Phase 4: Compute predecessor lists from successors
    let mut preds = vec![Vec::new(); num_blocks];
    for b in &basic_blocks {
        for &succ in &b.successors {
            let s_idx = succ.0 as usize;
            if s_idx < num_blocks && !preds[s_idx].contains(&b.id) {
                preds[s_idx].push(b.id);
            }
        }
    }
    for (idx, b) in basic_blocks.iter_mut().enumerate() {
        b.predecessors = std::mem::take(&mut preds[idx]);
    }

    graph.blocks = basic_blocks;
    fold_constants(&mut graph);
    fold_conditional_branches(&mut graph);
    simplify_trivial_phis(&mut graph);
    fold_undefined_returns(&mut graph);
    eliminate_dead_code(&mut graph);
    let has_suspend = graph
        .blocks
        .iter()
        .any(|b| matches!(b.terminator, Terminator::Suspend { .. }));
    if !has_suspend {
        canonicalize_loop_preheaders(&mut graph);
        hoist_loop_invariants(&mut graph);
        eliminate_common_subexpressions(&mut graph);
    }
    simplify_trivial_phis(&mut graph);
    fold_constants(&mut graph);
    fold_conditional_branches(&mut graph);
    simplify_trivial_phis(&mut graph);
    fold_undefined_returns(&mut graph);
    eliminate_dead_code(&mut graph);
    if merge_basic_blocks(&mut graph) {
        simplify_trivial_phis(&mut graph);
        eliminate_dead_code(&mut graph);
    }
    compact_value_ids(&mut graph);
    graph.mark_non_escaping_objects(program);
    Ok(graph)
}

fn to_int32(val: f64) -> i32 {
    if !val.is_finite() || val == 0.0 {
        0
    } else {
        let rem = val.trunc() % 4294967296.0;
        rem as i64 as i32
    }
}

fn to_uint32(val: f64) -> u32 {
    to_int32(val) as u32
}

fn constant_truthy(c: &Constant) -> bool {
    match c {
        Constant::Undefined | Constant::Null => false,
        Constant::Boolean(b) => *b,
        Constant::Number(bits) => {
            let f = f64::from_bits(*bits);
            f != 0.0 && !f.is_nan()
        }
        Constant::String(s) => !s.is_empty(),
        Constant::BigInt(s) => s != "0" && !s.is_empty(),
    }
}

fn fold_unary(op: UnaryOp, c: &Constant) -> Option<Constant> {
    match op {
        UnaryOp::Not => Some(Constant::Boolean(!constant_truthy(c))),
        UnaryOp::Void => Some(Constant::Undefined),
        UnaryOp::Negate => match c {
            Constant::Number(bits) => {
                let f = -f64::from_bits(*bits);
                Some(Constant::Number(f.to_bits()))
            }
            _ => None,
        },
        UnaryOp::Plus => match c {
            Constant::Number(bits) => Some(Constant::Number(*bits)),
            Constant::Boolean(b) => {
                Some(Constant::Number(if *b { 1.0f64 } else { 0.0f64 }.to_bits()))
            }
            Constant::Null => Some(Constant::Number(0.0f64.to_bits())),
            Constant::Undefined => Some(Constant::Number(f64::NAN.to_bits())),
            _ => None,
        },
        UnaryOp::BitwiseNot => match c {
            Constant::Number(bits) => {
                let n = to_int32(f64::from_bits(*bits));
                Some(Constant::Number(((!n) as f64).to_bits()))
            }
            Constant::Boolean(b) => {
                let n = if *b { 1i32 } else { 0i32 };
                Some(Constant::Number(((!n) as f64).to_bits()))
            }
            Constant::Null | Constant::Undefined => Some(Constant::Number((-1.0f64).to_bits())),
            _ => None,
        },
        UnaryOp::ToNumeric => match c {
            Constant::Number(bits) => Some(Constant::Number(*bits)),
            _ => None,
        },
        _ => None,
    }
}

fn fold_binary(op: BinaryOp, left: &Constant, right: &Constant) -> Option<Constant> {
    match op {
        BinaryOp::Add => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) + f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Subtract => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) - f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Multiply => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) * f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Divide => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) / f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Remainder => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a) % f64::from_bits(*b);
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::Exponentiate => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = f64::from_bits(*a).powf(f64::from_bits(*b));
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::BitwiseAnd => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = (to_int32(f64::from_bits(*a)) & to_int32(f64::from_bits(*b))) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::BitwiseOr => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = (to_int32(f64::from_bits(*a)) | to_int32(f64::from_bits(*b))) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::BitwiseXor => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let val = (to_int32(f64::from_bits(*a)) ^ to_int32(f64::from_bits(*b))) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::ShiftLeft => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let shift = to_uint32(f64::from_bits(*b)) & 0x1f;
                let val = (to_int32(f64::from_bits(*a)) << shift) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::ShiftRight => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let shift = to_uint32(f64::from_bits(*b)) & 0x1f;
                let val = (to_int32(f64::from_bits(*a)) >> shift) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::UnsignedShiftRight => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let shift = to_uint32(f64::from_bits(*b)) & 0x1f;
                let val = (to_uint32(f64::from_bits(*a)) >> shift) as f64;
                Some(Constant::Number(val.to_bits()))
            }
            _ => None,
        },
        BinaryOp::StrictEqual => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa == fb))
            }
            (Constant::Boolean(a), Constant::Boolean(b)) => Some(Constant::Boolean(a == b)),
            (Constant::Null, Constant::Null) => Some(Constant::Boolean(true)),
            (Constant::Undefined, Constant::Undefined) => Some(Constant::Boolean(true)),
            (Constant::Null, Constant::Undefined) | (Constant::Undefined, Constant::Null) => {
                Some(Constant::Boolean(false))
            }
            _ => None,
        },
        BinaryOp::StrictNotEqual => match fold_binary(BinaryOp::StrictEqual, left, right) {
            Some(Constant::Boolean(b)) => Some(Constant::Boolean(!b)),
            _ => None,
        },
        BinaryOp::LessThan => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa < fb))
            }
            _ => None,
        },
        BinaryOp::LessThanOrEqual => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa <= fb))
            }
            _ => None,
        },
        BinaryOp::GreaterThan => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa > fb))
            }
            _ => None,
        },
        BinaryOp::GreaterThanOrEqual => match (left, right) {
            (Constant::Number(a), Constant::Number(b)) => {
                let fa = f64::from_bits(*a);
                let fb = f64::from_bits(*b);
                Some(Constant::Boolean(fa >= fb))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Constant folding pass over the SSA Value Graph.
/// Evaluates pure operations with known constant operands and replaces them with `GraphOp::Const`.
pub fn fold_constants(graph: &mut FunctionGraph) {
    let mut constants: HashMap<ValueId, Constant> = HashMap::new();
    loop {
        let mut changed = false;
        for block in &mut graph.blocks {
            for (vid, op) in &mut block.operations {
                match op {
                    GraphOp::Const { constant, .. } => {
                        if !constants.contains_key(vid) {
                            constants.insert(*vid, constant.clone());
                        }
                    }
                    GraphOp::Unary { op: u_op, value } => {
                        if let Some(c) = constants.get(value) {
                            if let Some(folded) = fold_unary(*u_op, c) {
                                *op = GraphOp::Const {
                                    index: 0,
                                    constant: folded.clone(),
                                };
                                constants.insert(*vid, folded);
                                changed = true;
                            }
                        }
                    }
                    GraphOp::Binary {
                        op: b_op,
                        left,
                        right,
                    } => {
                        if let (Some(cl), Some(cr)) = (constants.get(left), constants.get(right)) {
                            if let Some(folded) = fold_binary(*b_op, cl, cr) {
                                *op = GraphOp::Const {
                                    index: 0,
                                    constant: folded.clone(),
                                };
                                constants.insert(*vid, folded);
                                changed = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Normalize returns of constant `undefined` to `Terminator::Return(None)`, allowing dead
/// constant definitions to be eliminated and direct undefined return pathways to be emitted.
pub fn fold_undefined_returns(graph: &mut FunctionGraph) {
    let mut undefined_values = HashSet::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            if let GraphOp::Const {
                constant: Constant::Undefined,
                ..
            } = op
            {
                undefined_values.insert(*vid);
            }
        }
    }
    if undefined_values.is_empty() {
        return;
    }
    for block in &mut graph.blocks {
        if let Terminator::Return(Some(vid)) = &block.terminator
            && undefined_values.contains(vid)
        {
            block.terminator = Terminator::Return(None);
        }
    }
}

/// Prune unreachable basic blocks from the CFG, updating predecessors and Phi inputs in surviving blocks.
pub fn prune_unreachable_blocks(graph: &mut FunctionGraph) {
    if graph.blocks.is_empty() {
        return;
    }

    let mut reachable = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    reachable.insert(graph.entry_block);
    queue.push_back(graph.entry_block);

    if let Some(seam) = graph.seam_block {
        if reachable.insert(seam) {
            queue.push_back(seam);
        }
    }

    let block_map: HashMap<BlockId, &BasicBlock> = graph.blocks.iter().map(|b| (b.id, b)).collect();

    while let Some(bid) = queue.pop_front() {
        if let Some(b) = block_map.get(&bid) {
            for &succ in &b.successors {
                if reachable.insert(succ) {
                    queue.push_back(succ);
                }
            }
            if let Some(handler) = b.exception_handler {
                if reachable.insert(handler) {
                    queue.push_back(handler);
                }
            }
            if let Terminator::Suspend {
                resume, delegate, ..
            } = &b.terminator
            {
                if reachable.insert(*resume) {
                    queue.push_back(*resume);
                }
                if let Some((_, exit, _)) = delegate
                    && reachable.insert(*exit)
                {
                    queue.push_back(*exit);
                }
            }
        }
    }

    if reachable.len() == graph.blocks.len() {
        return;
    }

    // Filter predecessors and Phi inputs in surviving blocks
    for block in &mut graph.blocks {
        if reachable.contains(&block.id) {
            block.predecessors.retain(|p| reachable.contains(p));
            for (_, op) in &mut block.operations {
                if let GraphOp::Phi { inputs } = op {
                    inputs.retain(|(p, _)| reachable.contains(p));
                }
            }
        }
    }

    // Prune unreachable blocks
    graph.blocks.retain(|b| reachable.contains(&b.id));
}

/// Fold conditional branches whose condition is statically known to a constant truthy/falsy value
/// or whose true and false targets are identical into unconditional jumps, and prune newly unreachable blocks.
pub fn fold_conditional_branches(graph: &mut FunctionGraph) -> bool {
    let mut constants: HashMap<ValueId, Constant> = HashMap::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            if let GraphOp::Const { constant, .. } = op {
                constants.insert(*vid, constant.clone());
            }
        }
    }

    let mut changed = false;
    let mut eliminated_edges: Vec<(BlockId, BlockId)> = Vec::new();

    for block in &mut graph.blocks {
        let replacement = match &block.terminator {
            Terminator::Branch {
                condition,
                true_block,
                false_block,
            } => {
                if true_block == false_block {
                    Some((*true_block, None))
                } else if let Some(c) = constants.get(condition) {
                    if constant_truthy(c) {
                        Some((*true_block, Some(*false_block)))
                    } else {
                        Some((*false_block, Some(*true_block)))
                    }
                } else {
                    None
                }
            }
            Terminator::BranchNotNullish {
                condition,
                true_block,
                false_block,
            } => {
                if true_block == false_block {
                    Some((*true_block, None))
                } else if let Some(c) = constants.get(condition) {
                    let nullish = matches!(c, Constant::Null | Constant::Undefined);
                    if !nullish {
                        Some((*true_block, Some(*false_block)))
                    } else {
                        Some((*false_block, Some(*true_block)))
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some((target, eliminated_target)) = replacement {
            block.terminator = Terminator::Jump(target);
            let mut successors = vec![target];
            if let Some(handler) = block.exception_handler
                && !successors.contains(&handler)
            {
                successors.push(handler);
            }
            block.successors = successors;
            if let Some(elim) = eliminated_target {
                eliminated_edges.push((block.id, elim));
            }
            changed = true;
        }
    }

    if !changed {
        return false;
    }

    // Update predecessors and Phis in eliminated target blocks
    for (from_block, to_block) in eliminated_edges {
        if let Some(from_b) = graph.blocks.iter().find(|b| b.id == from_block)
            && from_b.successors.contains(&to_block)
        {
            continue;
        }
        if let Some(b) = graph.blocks.iter_mut().find(|b| b.id == to_block) {
            b.predecessors.retain(|&p| p != from_block);
            for (_, op) in &mut b.operations {
                if let GraphOp::Phi { inputs } = op {
                    inputs.retain(|(p, _)| *p != from_block);
                }
            }
        }
    }

    prune_unreachable_blocks(graph);
    true
}

/// Merge sequential basic blocks where block A unconditionally jumps to block B,
/// B has A as its only predecessor, exception handling scopes match, and B is not
/// a protected entry, seam, exception handler, or coroutine resume target.
pub fn merge_basic_blocks(graph: &mut FunctionGraph) -> bool {
    if graph.blocks.len() < 2 {
        return false;
    }

    let mut protected_targets = HashSet::new();
    protected_targets.insert(graph.entry_block);
    if let Some(seam) = graph.seam_block {
        protected_targets.insert(seam);
    }
    for block in &graph.blocks {
        if let Some(h) = block.exception_handler {
            protected_targets.insert(h);
        }
        if let Terminator::Suspend {
            resume, delegate, ..
        } = &block.terminator
        {
            protected_targets.insert(*resume);
            if let Some((_, exit, _)) = delegate {
                protected_targets.insert(*exit);
            }
        }
    }

    let mut changed = false;
    loop {
        let mut merge_candidate: Option<(BlockId, BlockId)> = None;

        for block in &graph.blocks {
            if let Terminator::Jump(target) = block.terminator
                && target != block.id
                && !protected_targets.contains(&target)
                && let Some(target_block) = graph.blocks.iter().find(|b| b.id == target)
                && target_block.predecessors.len() == 1
                && target_block.predecessors[0] == block.id
                && block.exception_handler == target_block.exception_handler
                && !matches!(target_block.terminator, Terminator::Suspend { .. })
                && !target_block
                    .operations
                    .iter()
                    .any(|(_, op)| matches!(op, GraphOp::Phi { .. }))
            {
                merge_candidate = Some((block.id, target));
                break;
            }
        }

        let Some((a_id, b_id)) = merge_candidate else {
            break;
        };

        let b_idx = graph.blocks.iter().position(|b| b.id == b_id).unwrap();
        let b_block = graph.blocks.remove(b_idx);

        let a_block = graph.blocks.iter_mut().find(|b| b.id == a_id).unwrap();
        a_block.operations.extend(b_block.operations);
        a_block.terminator = b_block.terminator;
        a_block.successors = b_block.successors.clone();

        for succ_id in &b_block.successors {
            if let Some(succ_block) = graph.blocks.iter_mut().find(|b| b.id == *succ_id) {
                if succ_block.predecessors.contains(&a_id) {
                    succ_block.predecessors.retain(|&p| p != b_id);
                } else {
                    for p in &mut succ_block.predecessors {
                        if *p == b_id {
                            *p = a_id;
                        }
                    }
                }

                for (_, op) in &mut succ_block.operations {
                    if let GraphOp::Phi { inputs } = op {
                        for (p, _) in inputs {
                            if *p == b_id {
                                *p = a_id;
                            }
                        }
                    }
                }
            }
        }

        changed = true;
    }

    changed
}

/// Alias for dead code elimination preserving existing test entry points.
pub fn remove_dead_phis(graph: &mut FunctionGraph) {
    eliminate_dead_code(graph);
}

/// Remove dead operations and dead phi chains with no consumers (ADR 0061).
///
/// An operation is inherently live if it has observable side-effects (throwing, calling user code,
/// allocating, or writing heap/host), or if it is a local variable store (`StoreLocal`) whose slot
/// may be accessed in a frame/array, or a phi in a generator resume block. Pure operations with no
/// consumers are soundly eliminated.
pub fn eliminate_dead_code(graph: &mut FunctionGraph) {
    let mut live = HashSet::new();
    let mut resumes = HashSet::new();
    for block in &graph.blocks {
        match &block.terminator {
            Terminator::Return(Some(value))
            | Terminator::Throw(value)
            | Terminator::Branch {
                condition: value, ..
            }
            | Terminator::BranchNotNullish {
                condition: value, ..
            } => {
                live.insert(*value);
            }
            Terminator::Suspend {
                value,
                spilled,
                resume,
                delegate,
                ..
            } => {
                live.insert(*value);
                if let Some((iter, exit, _)) = delegate {
                    live.insert(*iter);
                    resumes.insert(*exit);
                }
                live.extend(spilled.iter().copied());
                resumes.insert(*resume);
            }
            _ => {}
        }
    }

    let entry_block = graph.entry_block;
    let proven_numbers = proven_numbers(graph);
    let is_inherently_live = |op: &GraphOp, block_id: BlockId| -> bool {
        if matches!(op, GraphOp::Phi { .. }) {
            return resumes.contains(&block_id);
        }
        if matches!(op, GraphOp::StoreLocal { .. }) {
            return true;
        }
        if matches!(op, GraphOp::LoadLocal(_)) && block_id == entry_block {
            return true;
        }
        let effects = op.effects_with_proven_numbers(&proven_numbers);
        effects.intersects(
            Effects::MAY_THROW
                | Effects::CALLS_USER_CODE
                | Effects::ALLOCATES
                | Effects::WRITES_HEAP
                | Effects::WRITES_HOST
                | Effects::MAY_SUSPEND,
        )
    };

    let mut def_operands: HashMap<ValueId, Vec<ValueId>> = HashMap::new();
    for block in &graph.blocks {
        for (value, op) in &block.operations {
            if is_inherently_live(op, block.id) {
                live.insert(*value);
                live.extend(op.operands());
            } else {
                def_operands.insert(*value, op.operands());
            }
        }
    }

    let mut pending: Vec<ValueId> = live.iter().copied().collect();
    while let Some(value) = pending.pop() {
        if let Some(operands) = def_operands.get(&value) {
            for operand in operands {
                if live.insert(*operand) {
                    pending.push(*operand);
                }
            }
        }
    }

    for block in &mut graph.blocks {
        block
            .operations
            .retain(|(value, op)| is_inherently_live(op, block.id) || live.contains(value));
    }
}

/// Compute the dominator set for each basic block in `graph`.
///
/// A block `D` dominates `B` if every control-flow path from `graph.entry_block` to `B`
/// must pass through `D`.
pub fn compute_dominators(graph: &FunctionGraph) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut dominators: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    if graph.blocks.is_empty() {
        return dominators;
    }
    let mut reachable = HashSet::new();
    let mut worklist = vec![graph.entry_block];
    reachable.insert(graph.entry_block);
    while let Some(bid) = worklist.pop() {
        if let Some(block) = graph.block(bid) {
            for &succ in &block.successors {
                if reachable.insert(succ) {
                    worklist.push(succ);
                }
            }
        }
    }

    let all_reachable: HashSet<BlockId> = reachable.clone();

    let mut entry_dom = HashSet::new();
    entry_dom.insert(graph.entry_block);
    dominators.insert(graph.entry_block, entry_dom);

    for block in &graph.blocks {
        if block.id != graph.entry_block {
            if reachable.contains(&block.id) {
                dominators.insert(block.id, all_reachable.clone());
            } else {
                let mut self_dom = HashSet::new();
                self_dom.insert(block.id);
                dominators.insert(block.id, self_dom);
            }
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for block in &graph.blocks {
            if !reachable.contains(&block.id) || block.id == graph.entry_block {
                continue;
            }
            let mut new_dom: Option<HashSet<BlockId>> = None;
            for &pred in &block.predecessors {
                if !reachable.contains(&pred) {
                    continue;
                }
                if let Some(pred_dom) = dominators.get(&pred) {
                    match &mut new_dom {
                        Some(current) => {
                            *current = current.intersection(pred_dom).copied().collect();
                        }
                        None => {
                            new_dom = Some(pred_dom.clone());
                        }
                    }
                }
            }
            let mut new_dom = new_dom.unwrap_or_default();
            new_dom.insert(block.id);

            if dominators.get(&block.id) != Some(&new_dom) {
                dominators.insert(block.id, new_dom);
                changed = true;
            }
        }
    }
    dominators
}

/// Find all natural loops in `graph` given precomputed `dominators`.
///
/// Returns a map from loop header `BlockId` to the set of `BlockId`s belonging to the loop.
pub fn find_natural_loops(
    graph: &FunctionGraph,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut loops: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    for block in &graph.blocks {
        let b_id = block.id;
        for &succ in &block.successors {
            if dominators
                .get(&b_id)
                .is_some_and(|doms| doms.contains(&succ))
            {
                // succ is loop header H
                let h_id = succ;
                let entry = loops.entry(h_id).or_default();
                entry.insert(h_id);
                entry.insert(b_id);

                let mut worklist = vec![b_id];
                while let Some(node) = worklist.pop() {
                    if node == h_id {
                        continue;
                    }
                    if let Some(b) = graph.blocks.iter().find(|b| b.id == node) {
                        for &pred in &b.predecessors {
                            if entry.insert(pred) {
                                worklist.push(pred);
                            }
                        }
                    }
                }
            }
        }
    }
    loops
}

/// Ensure every natural loop has a dedicated, single preheader block that jumps directly to the header.
///
/// If a loop header has multiple outside predecessors (e.g. entered from an if-else) or if its outside
/// predecessor does not end in a simple Jump to the header, a synthetic preheader trampoline is created.
/// Outside edges are retargeted to the preheader, and any phis in the header are appropriately split.
pub fn canonicalize_loop_preheaders(graph: &mut FunctionGraph) {
    if graph.blocks.len() < 2 {
        return;
    }
    for _ in 0..=graph.blocks.len() {
        let dominators = compute_dominators(graph);
        let loops = find_natural_loops(graph, &dominators);
        let mut synthesized = false;

        let mut header_ids: Vec<BlockId> = loops.keys().copied().collect();
        header_ids.sort_by_key(|b| b.0);

        for header_id in header_ids {
            let loop_blocks = &loops[&header_id];
            let header_idx = match graph.blocks.iter().position(|b| b.id == header_id) {
                Some(idx) => idx,
                None => continue,
            };

            // If any block in the loop can suspend, skip
            if loop_blocks.iter().any(|&b_id| {
                graph
                    .blocks
                    .iter()
                    .find(|b| b.id == b_id)
                    .is_some_and(|b| matches!(b.terminator, Terminator::Suspend { .. }))
            }) {
                continue;
            }

            let outside_preds: Vec<BlockId> = graph.blocks[header_idx]
                .predecessors
                .iter()
                .filter(|p| !loop_blocks.contains(p))
                .copied()
                .collect();

            if outside_preds.is_empty() {
                continue;
            }

            // Check if already canonical: single outside predecessor that jumps directly to header
            if outside_preds.len() == 1 {
                let pred_id = outside_preds[0];
                if let Some(pred_block) = graph.blocks.iter().find(|b| b.id == pred_id)
                    && matches!(pred_block.terminator, Terminator::Jump(target) if target == header_id)
                    && pred_block.successors.len() == 1
                {
                    continue;
                }
            }

            // We need a synthetic preheader.
            synthesized = true;
            let new_bid = BlockId(graph.blocks.iter().map(|b| b.id.0).max().unwrap_or(0) + 1);

            // Rewrite outside predecessors to jump to new_bid instead of header_id
            for &pred_id in &outside_preds {
                if let Some(pred_block) = graph.blocks.iter_mut().find(|b| b.id == pred_id) {
                    match &mut pred_block.terminator {
                        Terminator::Jump(target) if *target == header_id => {
                            *target = new_bid;
                        }
                        Terminator::Branch {
                            true_block,
                            false_block,
                            ..
                        }
                        | Terminator::BranchNotNullish {
                            true_block,
                            false_block,
                            ..
                        } => {
                            if *true_block == header_id {
                                *true_block = new_bid;
                            }
                            if *false_block == header_id {
                                *false_block = new_bid;
                            }
                        }
                        _ => {}
                    }
                    for succ in &mut pred_block.successors {
                        if *succ == header_id {
                            *succ = new_bid;
                        }
                    }
                }
            }

            // Build phis for the new preheader and rewrite header phis
            let mut preheader_ops: Vec<(ValueId, GraphOp)> = Vec::new();
            let mut next_vid = graph.next_value_id;
            let header_block = &mut graph.blocks[header_idx];

            header_block
                .predecessors
                .retain(|p| loop_blocks.contains(p));
            header_block.predecessors.push(new_bid);

            for (_, op) in &mut header_block.operations {
                if let GraphOp::Phi { inputs } = op {
                    let mut outside_inputs: Vec<(BlockId, ValueId)> = Vec::new();
                    let mut loop_inputs: Vec<(BlockId, ValueId)> = Vec::new();

                    for (pred_b, in_vid) in inputs.drain(..) {
                        if outside_preds.contains(&pred_b) {
                            outside_inputs.push((pred_b, in_vid));
                        } else {
                            loop_inputs.push((pred_b, in_vid));
                        }
                    }

                    if outside_inputs.len() == 1 {
                        loop_inputs.push((new_bid, outside_inputs[0].1));
                        *inputs = loop_inputs;
                    } else if outside_inputs.len() > 1 {
                        let preheader_phi_vid = ValueId(next_vid);
                        next_vid += 1;
                        preheader_ops.push((
                            preheader_phi_vid,
                            GraphOp::Phi {
                                inputs: outside_inputs,
                            },
                        ));
                        loop_inputs.push((new_bid, preheader_phi_vid));
                        *inputs = loop_inputs;
                    }
                }
            }
            graph.next_value_id = next_vid;

            let is_entry = header_id == graph.entry_block;
            let preheader_block = BasicBlock {
                id: new_bid,
                operations: preheader_ops,
                terminator: Terminator::Jump(header_id),
                predecessors: outside_preds,
                successors: vec![header_id],
                exception_handler: None,
            };

            graph.blocks.push(preheader_block);
            if is_entry {
                graph.entry_block = new_bid;
            }
            break;
        }

        if !synthesized {
            break;
        }
    }
}

/// Hoist loop-invariant pure operations out of loops into preheaders (LICM, ADR 0061).
///
/// An operation can be hoisted if:
/// 1. Its effects are `Effects::PURE` (cannot throw, allocate, call user code, or touch heap/host).
/// 2. It is not a Phi, StoreLocal, or generator/handler special op.
/// 3. All of its operands are loop-invariant (defined outside the loop or already hoisted).
/// 4. The loop has a well-defined single preheader block that dominates the loop header.
pub fn hoist_loop_invariants(graph: &mut FunctionGraph) {
    if graph.blocks.len() < 2 {
        return;
    }
    let proven_numbers = proven_numbers(graph);
    let dominators = compute_dominators(graph);
    let loops = find_natural_loops(graph, &dominators);

    if loops.is_empty() {
        return;
    }

    // Step 3: For each natural loop, check if a unique preheader exists.
    for (header_id, loop_blocks) in loops {
        let header_block = match graph.blocks.iter().find(|b| b.id == header_id) {
            Some(b) => b,
            None => continue,
        };

        // If any block in the loop can suspend, hoisting to a preheader outside the loop
        // is unsound because resumption re-enters inside the loop with a fresh C stack frame.
        if loop_blocks.iter().any(|&b_id| {
            graph
                .blocks
                .iter()
                .find(|b| b.id == b_id)
                .is_some_and(|b| matches!(b.terminator, Terminator::Suspend { .. }))
        }) {
            continue;
        }

        // Predecessors not in loop
        let outside_preds: Vec<BlockId> = header_block
            .predecessors
            .iter()
            .filter(|p| !loop_blocks.contains(p))
            .copied()
            .collect();

        if outside_preds.len() != 1 {
            continue;
        }

        let preheader_id = outside_preds[0];
        let preheader_idx = match graph.blocks.iter().position(|b| b.id == preheader_id) {
            Some(idx) => idx,
            None => continue,
        };

        // The preheader must jump to the loop header
        if !matches!(graph.blocks[preheader_idx].terminator, Terminator::Jump(target) if target == header_id)
        {
            continue;
        }

        // Check that preheader dominates loop header (standard CFG property)
        if !dominators
            .get(&header_id)
            .is_some_and(|d| d.contains(&preheader_id))
        {
            continue;
        }

        // Map definitions to their containing blocks
        let mut def_block: HashMap<ValueId, BlockId> = HashMap::new();
        for block in &graph.blocks {
            for (vid, _) in &block.operations {
                def_block.insert(*vid, block.id);
            }
        }

        // Hoist loop-invariant operations
        let mut hoisted_vids: HashSet<ValueId> = HashSet::new();
        let mut progress = true;

        while progress {
            progress = false;
            let mut ops_to_hoist: Vec<(BlockId, usize, ValueId, GraphOp)> = Vec::new();

            for block in &graph.blocks {
                if !loop_blocks.contains(&block.id) || block.id == preheader_id {
                    continue;
                }

                // If block is inside a try-catch, do not hoist out of it
                if block.exception_handler.is_some() {
                    continue;
                }

                for (op_idx, (vid, op)) in block.operations.iter().enumerate() {
                    if hoisted_vids.contains(vid) {
                        continue;
                    }
                    if op.effects_with_proven_numbers(&proven_numbers) != Effects::PURE {
                        continue;
                    }
                    if matches!(
                        op,
                        GraphOp::Phi { .. }
                            | GraphOp::LoadLocal(_)
                            | GraphOp::StoreLocal { .. }
                            | GraphOp::ResumeValue
                            | GraphOp::CaughtValue
                    ) {
                        continue;
                    }

                    let operands = op.operands();
                    let all_operands_invariant = operands.iter().all(|operand| {
                        match def_block.get(operand) {
                            Some(&def_b) => {
                                !loop_blocks.contains(&def_b) || hoisted_vids.contains(operand)
                            }
                            None => true, // Function parameter or external
                        }
                    });

                    if all_operands_invariant {
                        ops_to_hoist.push((block.id, op_idx, *vid, op.clone()));
                        hoisted_vids.insert(*vid);
                        def_block.insert(*vid, preheader_id);
                        progress = true;
                        break; // hoist one per iteration to maintain ordering
                    }
                }

                if progress {
                    break;
                }
            }

            for (from_block, op_idx, vid, op) in ops_to_hoist {
                if let Some(b) = graph.blocks.iter_mut().find(|b| b.id == from_block) {
                    if op_idx < b.operations.len() && b.operations[op_idx].0 == vid {
                        b.operations.remove(op_idx);
                    } else if let Some(pos) = b.operations.iter().position(|(v, _)| *v == vid) {
                        b.operations.remove(pos);
                    }
                }
                graph.blocks[preheader_idx].operations.push((vid, op));
            }
        }
    }
}

/// Expression key for Dominator-Based Global Common Subexpression Elimination (CSE).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CseKey {
    Const(Constant),
    Unary(UnaryOp, ValueId),
    Binary(BinaryOp, ValueId, ValueId),
    Phi(BlockId, Vec<(BlockId, ValueId)>),
    LoadBuiltin(kiln_ir::Builtin),
    LoadThis,
    LoadNewTarget,
    IsReturnSignal(ValueId),
}

/// Compute the immediate dominator tree: maps each block to the children it immediately dominates.
pub fn compute_dominator_tree(
    graph: &FunctionGraph,
    dominators: &HashMap<BlockId, HashSet<BlockId>>,
) -> HashMap<BlockId, Vec<BlockId>> {
    let mut dom_children: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for block in &graph.blocks {
        if block.id == graph.entry_block {
            continue;
        }
        if let Some(doms) = dominators.get(&block.id) {
            let mut candidates: Vec<BlockId> =
                doms.iter().copied().filter(|&d| d != block.id).collect();
            candidates.sort_by_key(|b| b.0);
            let mut best_idom: Option<(BlockId, usize)> = None;
            for d in candidates {
                let d_size = dominators.get(&d).map_or(0, |s| s.len());
                if best_idom.is_none() || d_size > best_idom.unwrap().1 {
                    best_idom = Some((d, d_size));
                }
            }
            if let Some((idom, _)) = best_idom {
                dom_children.entry(idom).or_default().push(block.id);
            }
        }
    }
    for children in dom_children.values_mut() {
        children.sort_by_key(|b| b.0);
    }
    dom_children
}

fn remap_graph_operands(graph: &mut FunctionGraph, map: impl Fn(ValueId) -> ValueId) {
    for block in &mut graph.blocks {
        for (_, op) in &mut block.operations {
            op.map_operands(&map);
        }
        match &mut block.terminator {
            Terminator::Return(Some(value))
            | Terminator::Throw(value)
            | Terminator::Branch {
                condition: value, ..
            }
            | Terminator::BranchNotNullish {
                condition: value, ..
            } => *value = map(*value),
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                *value = map(*value);
                if let Some((iter, _, _)) = delegate {
                    *iter = map(*iter);
                }
                for value in spilled {
                    *value = map(*value);
                }
            }
            _ => {}
        }
    }
}

/// Deduplicate identical pure expressions across dominating basic blocks (Global CSE).
pub fn eliminate_common_subexpressions(graph: &mut FunctionGraph) {
    if graph.blocks.is_empty() {
        return;
    }
    let dominators = compute_dominators(graph);
    let dom_tree = compute_dominator_tree(graph, &dominators);
    let proven_numbers = proven_numbers(graph);

    let mut available: HashMap<CseKey, ValueId> = HashMap::new();
    let mut history: Vec<(CseKey, Option<ValueId>)> = Vec::new();
    let mut replacements: HashMap<ValueId, ValueId> = HashMap::new();
    let mut ops_to_remove: HashSet<ValueId> = HashSet::new();

    fn resolve_alias(replacements: &HashMap<ValueId, ValueId>, mut v: ValueId) -> ValueId {
        while let Some(&next) = replacements.get(&v) {
            v = next;
        }
        v
    }

    fn to_cse_key(
        block_id: BlockId,
        op: &GraphOp,
        proven_numbers: &HashSet<ValueId>,
        replacements: &HashMap<ValueId, ValueId>,
    ) -> Option<CseKey> {
        if op.effects_with_proven_numbers(proven_numbers) != Effects::PURE {
            return None;
        }
        match op {
            GraphOp::Const { constant, .. } => Some(CseKey::Const(constant.clone())),
            GraphOp::Unary { op, value } => {
                let v = resolve_alias(replacements, *value);
                Some(CseKey::Unary(*op, v))
            }
            GraphOp::Binary { op, left, right } => {
                let mut l = resolve_alias(replacements, *left);
                let mut r = resolve_alias(replacements, *right);
                if matches!(
                    op,
                    BinaryOp::Add
                        | BinaryOp::Multiply
                        | BinaryOp::BitwiseAnd
                        | BinaryOp::BitwiseOr
                        | BinaryOp::BitwiseXor
                        | BinaryOp::StrictEqual
                        | BinaryOp::StrictNotEqual
                ) && l.0 > r.0
                {
                    std::mem::swap(&mut l, &mut r);
                }
                Some(CseKey::Binary(*op, l, r))
            }
            GraphOp::Phi { inputs } => {
                let mut resolved_inputs: Vec<(BlockId, ValueId)> = inputs
                    .iter()
                    .map(|(b, v)| (*b, resolve_alias(replacements, *v)))
                    .collect();
                resolved_inputs.sort_by_key(|(b, _)| b.0);
                Some(CseKey::Phi(block_id, resolved_inputs))
            }
            GraphOp::LoadBuiltin(b) => Some(CseKey::LoadBuiltin(*b)),
            GraphOp::LoadThis => Some(CseKey::LoadThis),
            GraphOp::LoadNewTarget => Some(CseKey::LoadNewTarget),
            GraphOp::IsReturnSignal(v) => {
                Some(CseKey::IsReturnSignal(resolve_alias(replacements, *v)))
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn traverse(
        block_id: BlockId,
        graph: &FunctionGraph,
        dom_tree: &HashMap<BlockId, Vec<BlockId>>,
        proven_numbers: &HashSet<ValueId>,
        available: &mut HashMap<CseKey, ValueId>,
        history: &mut Vec<(CseKey, Option<ValueId>)>,
        replacements: &mut HashMap<ValueId, ValueId>,
        ops_to_remove: &mut HashSet<ValueId>,
    ) {
        let checkpoint = history.len();
        if let Some(block) = graph.block(block_id) {
            for (vid, op) in &block.operations {
                if let Some(key) = to_cse_key(block_id, op, proven_numbers, replacements) {
                    if let Some(&existing_vid) = available.get(&key) {
                        replacements.insert(*vid, existing_vid);
                        ops_to_remove.insert(*vid);
                    } else {
                        let old = available.insert(key.clone(), *vid);
                        history.push((key, old));
                    }
                }
            }
        }

        if let Some(children) = dom_tree.get(&block_id) {
            for &child in children {
                traverse(
                    child,
                    graph,
                    dom_tree,
                    proven_numbers,
                    available,
                    history,
                    replacements,
                    ops_to_remove,
                );
            }
        }

        while history.len() > checkpoint {
            let (key, old) = history.pop().unwrap();
            match old {
                Some(val) => {
                    available.insert(key, val);
                }
                None => {
                    available.remove(&key);
                }
            }
        }
    }

    traverse(
        graph.entry_block,
        graph,
        &dom_tree,
        &proven_numbers,
        &mut available,
        &mut history,
        &mut replacements,
        &mut ops_to_remove,
    );

    if !replacements.is_empty() {
        for block in &mut graph.blocks {
            block
                .operations
                .retain(|(vid, _)| !ops_to_remove.contains(vid));
        }
        let resolve = |v: ValueId| resolve_alias(&replacements, v);
        remap_graph_operands(graph, resolve);
        simplify_trivial_phis(graph);
        eliminate_dead_code(graph);
    }
}

/// Apply a value renaming before escape-analysis metadata is constructed.
fn map_graph_values(graph: &mut FunctionGraph, map: impl Fn(ValueId) -> ValueId) {
    debug_assert!(graph.scalar_objects.is_empty() && graph.spread_scalar_objects.is_empty());
    for block in &mut graph.blocks {
        for (value, op) in &mut block.operations {
            *value = map(*value);
            op.map_operands(&map);
        }
        match &mut block.terminator {
            Terminator::Return(Some(value))
            | Terminator::Throw(value)
            | Terminator::Branch {
                condition: value, ..
            }
            | Terminator::BranchNotNullish {
                condition: value, ..
            } => *value = map(*value),
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                *value = map(*value);
                if let Some((iter, _, _)) = delegate {
                    *iter = map(*iter);
                }
                for value in spilled {
                    *value = map(*value);
                }
            }
            _ => {}
        }
    }
    for value in &mut graph.initial_locals {
        *value = map(*value);
    }
    graph.origins = std::mem::take(&mut graph.origins)
        .into_iter()
        .map(|(value, origin)| (map(value), origin))
        .collect();
}

/// A phi whose non-self inputs all name one value contributes no choice. Resolve its users
/// directly to that value, including loop-invariant phis with a self input. Resume phis have
/// implicit frame inputs and must never be simplified from their explicit edges alone.
fn simplify_trivial_phis(graph: &mut FunctionGraph) {
    let resumes: HashSet<_> = graph
        .blocks
        .iter()
        .flat_map(|block| match &block.terminator {
            Terminator::Suspend {
                resume, delegate, ..
            } => {
                if let Some((_, exit, _)) = delegate {
                    vec![*resume, *exit]
                } else {
                    vec![*resume]
                }
            }
            _ => vec![],
        })
        .collect();
    let mut aliases: Vec<_> = (0..graph.next_value_id).map(ValueId).collect();
    let mut users = vec![Vec::new(); aliases.len()];
    let mut phis = HashMap::new();
    let mut pending = Vec::new();
    for block in &graph.blocks {
        for (value, op) in &block.operations {
            if let GraphOp::Phi { inputs } = op {
                if resumes.contains(&block.id) {
                    continue;
                }
                for (_, input) in inputs {
                    users[input.0 as usize].push(*value);
                }
                phis.insert(*value, inputs);
                pending.push(*value);
            }
        }
    }
    fn root(aliases: &[ValueId], mut value: ValueId) -> ValueId {
        while aliases[value.0 as usize] != value {
            value = aliases[value.0 as usize];
        }
        value
    }
    while let Some(phi) = pending.pop() {
        if aliases[phi.0 as usize] != phi {
            continue;
        }
        let mut replacement = None;
        let mut agrees = true;
        for (_, input) in phis[&phi] {
            let input = root(&aliases, *input);
            if input == phi {
                continue;
            }
            if replacement.is_some_and(|value| value != input) {
                agrees = false;
                break;
            }
            replacement = Some(input);
        }
        if agrees && let Some(replacement) = replacement {
            aliases[phi.0 as usize] = replacement;
            // Users of an alias also need revisiting if its new root later simplifies.
            let moved = std::mem::take(&mut users[phi.0 as usize]);
            pending.extend(moved.iter().copied());
            users[replacement.0 as usize].extend(moved);
        }
    }
    for index in 0..aliases.len() {
        aliases[index] = root(&aliases, ValueId(index as u32));
    }
    for block in &mut graph.blocks {
        block
            .operations
            .retain(|(value, _)| aliases[value.0 as usize] == *value);
    }
    graph
        .origins
        .retain(|value, _| aliases[value.0 as usize] == *value);
    map_graph_values(graph, |value| aliases[value.0 as usize]);
}

/// Pruning must shrink the backing SSA array too. Renumber surviving definitions and all
/// references in old-ID order, retaining even an undefined reference for the verifier to reject.
/// Site IDs, local slots, suspension order and bytecode diagnostic origins are preserved.
fn compact_value_ids(graph: &mut FunctionGraph) {
    let mut present = vec![false; graph.next_value_id as usize];
    for value in &graph.initial_locals {
        present[value.0 as usize] = true;
    }
    for block in &graph.blocks {
        for (value, op) in &block.operations {
            present[value.0 as usize] = true;
            for operand in op.operands() {
                present[operand.0 as usize] = true;
            }
        }
        match &block.terminator {
            Terminator::Return(Some(value))
            | Terminator::Throw(value)
            | Terminator::Branch {
                condition: value, ..
            }
            | Terminator::BranchNotNullish {
                condition: value, ..
            } => present[value.0 as usize] = true,
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                present[value.0 as usize] = true;
                if let Some((iter, _, _)) = delegate {
                    present[iter.0 as usize] = true;
                }
                for value in spilled {
                    present[value.0 as usize] = true;
                }
            }
            _ => {}
        }
    }
    let mut next = 0;
    let mapping: Vec<_> = present
        .iter()
        .map(|used| {
            if *used {
                let value = ValueId(next);
                next += 1;
                value
            } else {
                ValueId(u32::MAX)
            }
        })
        .collect();
    graph.origins.retain(|value, _| present[value.0 as usize]);
    map_graph_values(graph, |value| mapping[value.0 as usize]);
    graph.next_value_id = next;
}

/// The slots that must exist in `locals[]`: cells, and slots some block loads other than the
/// entry block's definitions of the initial values.
pub fn array_slots(graph: &FunctionGraph, program: &Program) -> HashSet<u32> {
    let mut slots = HashSet::new();
    if graph.blocks.iter().any(|b| b.exception_handler.is_some()) {
        // A handler reads the locals as they were at the throw, which can be any point of the
        // protected region: every slot lives in the array.
        return (0..graph.local_count).collect();
    }
    for block in &graph.blocks {
        for (_, op) in &block.operations {
            match op {
                GraphOp::LoadCell(slot)
                | GraphOp::StoreCell { slot, .. }
                | GraphOp::MakeCell(slot) => {
                    slots.insert(*slot);
                }
                GraphOp::LoadLocal(slot) if block.id != graph.entry_block => {
                    slots.insert(*slot);
                }
                GraphOp::MakeFunction { index, .. } => {
                    // A closure copies its captured boxes out of the array.
                    for upvalue in &program.functions[*index as usize].upvalues {
                        if let Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } =
                            upvalue
                        {
                            slots.insert(*slot);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    slots
}

pub fn resolve_root_alias(mut vid: ValueId, defs: &HashMap<ValueId, &GraphOp>) -> ValueId {
    let mut steps = 0;
    while let Some(op) = defs.get(&vid) {
        match op {
            GraphOp::StoreLocal { value, .. } | GraphOp::CheckTdz { value, .. } => {
                vid = *value;
            }
            // A merge of one value with itself is that value: a constant hoisted before a
            // conditional and carried across it on the operand stack, or a loop-invariant local.
            GraphOp::Phi { inputs } => {
                let mut single = None;
                for (_, input) in inputs {
                    if *input == vid {
                        continue;
                    }
                    match single {
                        None => single = Some(*input),
                        Some(seen) if seen == *input => {}
                        Some(_) => return vid,
                    }
                }
                match single {
                    Some(input) => vid = input,
                    None => break,
                }
            }
            _ => break,
        }
        steps += 1;
        if steps > 64 {
            break;
        }
    }
    vid
}

pub fn resolve_const_string(
    vid: ValueId,
    defs: &HashMap<ValueId, &GraphOp>,
    _program: &Program,
) -> Option<String> {
    let root = resolve_root_alias(vid, defs);
    if let Some(GraphOp::Const {
        constant: Constant::String(s),
        ..
    }) = defs.get(&root)
    {
        return s.to_scalar_string().ok();
    }
    None
}

/// Result of escape analysis on a function graph.
#[derive(Debug, Clone, Default)]
pub struct EscapeAnalysisResult {
    pub non_escaping: BTreeSet<ValueId>,
    pub scalar_objects: HashMap<ValueId, ScalarObject>,
    pub alias_map: HashMap<ValueId, ValueId>,
}

const OBJECT_PROTOTYPE_PROPERTIES: &[&str] = &[
    "constructor",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__proto__",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

pub fn escape_analysis(graph: &FunctionGraph, program: &Program) -> EscapeAnalysisResult {
    escape_analysis_with(graph, program, false, None).0
}

/// A bridge that a spread-aware analysis treats as an aliasing write onto its target rather
/// than an escape: the spread of a literal, and a constant-key define after one. Answers the
/// target, the define's key and value when it is a define, and the source when it is a spread.
type SpreadBridge<'a> = (
    &'a ValueId,
    Option<(String, u32, &'a ValueId)>,
    Option<&'a ValueId>,
);

fn spread_bridge<'a>(
    op: &'a GraphOp,
    defs: &HashMap<ValueId, &'a GraphOp>,
    program: &'a Program,
) -> Option<SpreadBridge<'a>> {
    match op {
        GraphOp::Bridge {
            instruction: Instruction::SpreadObject,
            operands,
            pushes: 1,
        } if operands.len() == 2 => Some((&operands[0], None, Some(&operands[1]))),
        GraphOp::Bridge {
            instruction: Instruction::DefineNamedProperty,
            operands,
            pushes: 1,
        } if operands.len() == 3 => {
            let key = resolve_const_string(operands[1], defs, program)?;
            let index = match defs.get(&resolve_root_alias(operands[1], defs)) {
                Some(GraphOp::Const { index, .. }) => *index,
                _ => return None,
            };
            Some((&operands[0], Some((key, index, &operands[2])), None))
        }
        _ => None,
    }
}

/// `with_spread`: a literal's spread and its defines alias their target instead of escaping
/// it, and an object with exactly one spread is reported as a sink. Everything else is the
/// unconditional analysis.
fn escape_analysis_with(
    graph: &FunctionGraph,
    program: &Program,
    with_spread: bool,
    extra_slots: Option<&HashSet<u32>>,
) -> (EscapeAnalysisResult, HashMap<ValueId, SpreadSink>) {
    let mut defs = HashMap::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            defs.insert(*vid, op);
        }
    }

    // Step 1: Collect candidates (NewObject where all keys are known constant strings)
    let mut candidates = HashSet::new();
    for (vid, op) in &defs {
        if let GraphOp::NewObject { pairs, .. } = op {
            let all_keys_const = pairs
                .iter()
                .all(|(k, _)| resolve_const_string(*k, &defs, program).is_some());
            if all_keys_const {
                candidates.insert(*vid);
            }
        }
    }

    if candidates.is_empty() {
        return (EscapeAnalysisResult::default(), HashMap::new());
    }

    // Step 2: Build alias map (ValueId -> root candidate alloc_id)
    let mut alias_to_root: HashMap<ValueId, ValueId> = HashMap::new();
    for &alloc_id in &candidates {
        alias_to_root.insert(alloc_id, alloc_id);
    }

    let mut changed = true;
    while changed {
        changed = false;
        for (vid, op) in &defs {
            if alias_to_root.contains_key(vid) {
                continue;
            }
            match op {
                GraphOp::StoreLocal { value, .. } | GraphOp::CheckTdz { value, .. } => {
                    if let Some(&root) = alias_to_root.get(value) {
                        alias_to_root.insert(*vid, root);
                        changed = true;
                    }
                }
                GraphOp::Bridge { .. } if with_spread => {
                    if let Some((target, _, _)) = spread_bridge(op, &defs, program)
                        && let Some(&root) = alias_to_root.get(target)
                    {
                        alias_to_root.insert(*vid, root);
                        changed = true;
                    }
                }
                GraphOp::Phi { inputs }
                    if !inputs.is_empty() && !alias_to_root.contains_key(vid) =>
                {
                    // Optimistic over the phis among its inputs that have no root yet -- two
                    // phis of a loop header and its body merge each wait on the other -- and
                    // exact over everything else. Step 4 verifies every aliased phi against
                    // the fixpoint, so a guess that does not hold escapes the object instead.
                    let non_self: Vec<ValueId> = inputs
                        .iter()
                        .map(|(_, v)| *v)
                        .filter(|v| v != vid)
                        .collect();
                    let mut root = None;
                    let mut agree = true;
                    for v in &non_self {
                        match alias_to_root.get(v) {
                            Some(&r) => {
                                if root.is_some_and(|seen| seen != r) {
                                    agree = false;
                                }
                                root = Some(r);
                            }
                            None if matches!(defs.get(v), Some(GraphOp::Phi { .. })) => {}
                            None => agree = false,
                        }
                    }
                    if agree && let Some(root) = root {
                        alias_to_root.insert(*vid, root);
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }

    // Step 3: Compute liveness of Phis
    let mut live_values: HashSet<ValueId> = HashSet::new();
    for block in &graph.blocks {
        match &block.terminator {
            Terminator::Return(Some(v))
            | Terminator::Throw(v)
            | Terminator::Branch { condition: v, .. }
            | Terminator::BranchNotNullish { condition: v, .. } => {
                live_values.insert(*v);
            }
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                live_values.insert(*value);
                if let Some((iter, _, _)) = delegate {
                    live_values.insert(*iter);
                }
                live_values.extend(spilled);
            }
            _ => {}
        }
        for (_, op) in &block.operations {
            if !matches!(op, GraphOp::Phi { .. }) {
                match op {
                    GraphOp::StoreLocal { value, .. }
                    | GraphOp::StoreCell { value, .. }
                    | GraphOp::StoreGlobal { value, .. }
                    | GraphOp::StoreUpvalue { value, .. }
                    | GraphOp::CheckTdz { value, .. }
                    | GraphOp::Unary { value, .. }
                    | GraphOp::IsReturnSignal(value) => {
                        live_values.insert(*value);
                    }
                    GraphOp::Binary { left, right, .. } => {
                        live_values.insert(*left);
                        live_values.insert(*right);
                    }
                    GraphOp::GetProperty { object, .. } => {
                        live_values.insert(*object);
                    }
                    GraphOp::SetProperty { object, value, .. }
                    | GraphOp::SetPropertyStrict { object, value, .. } => {
                        live_values.insert(*object);
                        live_values.insert(*value);
                    }
                    GraphOp::GetComputed { object, key, .. } => {
                        live_values.insert(*object);
                        live_values.insert(*key);
                    }
                    GraphOp::SetComputed {
                        object, key, value, ..
                    }
                    | GraphOp::SetComputedStrict {
                        object, key, value, ..
                    } => {
                        live_values.insert(*object);
                        live_values.insert(*key);
                        live_values.insert(*value);
                    }
                    GraphOp::Call { callee, args, .. } => {
                        live_values.insert(*callee);
                        live_values.extend(args);
                    }
                    GraphOp::CallMethod { object, args, .. } => {
                        live_values.insert(*object);
                        live_values.extend(args);
                    }
                    GraphOp::New { callee, args, .. } => {
                        live_values.insert(*callee);
                        live_values.extend(args);
                    }
                    GraphOp::NewArray { elements, .. } => {
                        live_values.extend(elements);
                    }
                    GraphOp::NewObject { pairs, .. } => {
                        for (k, v) in pairs {
                            live_values.insert(*k);
                            live_values.insert(*v);
                        }
                    }
                    GraphOp::Bridge { operands, .. } => {
                        live_values.extend(operands);
                    }
                    _ => {}
                }
            }
        }
    }

    let mut worklist: Vec<ValueId> = live_values.iter().copied().collect();
    while let Some(v) = worklist.pop() {
        if let Some(GraphOp::Phi { inputs }) = defs.get(&v) {
            for (_, in_v) in inputs {
                if live_values.insert(*in_v) {
                    worklist.push(*in_v);
                }
            }
        }
    }

    // Step 4: Check escapes
    let mut escaped: HashSet<ValueId> = HashSet::new();
    let mut spread_seen: HashSet<ValueId> = HashSet::new();
    let defined_by_bridge = |root: ValueId, name: &str| -> bool {
        with_spread
            && defs.values().any(|op| {
                matches!(spread_bridge(op, &defs, program),
                    Some((target, Some((key, _, _)), _)) if alias_to_root.get(target) == Some(&root) && key == name)
            })
    };
    let mut slots_in_array = array_slots(graph, program);
    if let Some(extra) = extra_slots {
        slots_in_array.extend(extra.iter().copied());
    }

    for block in &graph.blocks {
        match &block.terminator {
            Terminator::Return(Some(v))
            | Terminator::Throw(v)
            | Terminator::Branch { condition: v, .. }
            | Terminator::BranchNotNullish { condition: v, .. } => {
                if let Some(&root) = alias_to_root.get(v) {
                    escaped.insert(root);
                }
            }
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                ..
            } => {
                if let Some(&root) = alias_to_root.get(value) {
                    escaped.insert(root);
                }
                if let Some((iter, _, _)) = delegate
                    && let Some(&root) = alias_to_root.get(iter)
                {
                    escaped.insert(root);
                }
                for s in spilled {
                    if let Some(&root) = alias_to_root.get(s) {
                        escaped.insert(root);
                    }
                }
            }
            _ => {}
        }

        for (vid, op) in &block.operations {
            match op {
                GraphOp::StoreGlobal { value, .. }
                | GraphOp::StoreCell { value, .. }
                | GraphOp::StoreUpvalue { value, .. } => {
                    if let Some(&root) = alias_to_root.get(value) {
                        escaped.insert(root);
                    }
                }
                GraphOp::StoreLocal { slot, value } => {
                    if slots_in_array.contains(slot)
                        && let Some(&root) = alias_to_root.get(value)
                    {
                        escaped.insert(root);
                    }
                }
                GraphOp::Call { callee, args, .. } => {
                    if let Some(&root) = alias_to_root.get(callee) {
                        escaped.insert(root);
                    }
                    for a in args {
                        if let Some(&root) = alias_to_root.get(a) {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::CallMethod { object, args, .. } => {
                    if let Some(&root) = alias_to_root.get(object) {
                        escaped.insert(root);
                    }
                    for a in args {
                        if let Some(&root) = alias_to_root.get(a) {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::New { callee, args, .. } => {
                    if let Some(&root) = alias_to_root.get(callee) {
                        escaped.insert(root);
                    }
                    for a in args {
                        if let Some(&root) = alias_to_root.get(a) {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::NewArray { elements, .. } => {
                    for e in elements {
                        if let Some(&root) = alias_to_root.get(e) {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::NewObject { pairs, .. } => {
                    for (k, v) in pairs {
                        if let Some(&root) = alias_to_root.get(k) {
                            escaped.insert(root);
                        }
                        if let Some(&root) = alias_to_root.get(v) {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::Bridge { operands, .. } => {
                    let handled = with_spread
                        && match spread_bridge(op, &defs, program) {
                            Some((target, define, source))
                                if alias_to_root.contains_key(target) =>
                            {
                                let root = alias_to_root[target];
                                if let Some(source) = source {
                                    // A candidate spread into another escapes; a second spread
                                    // of the same object is not modelled.
                                    if let Some(&sroot) = alias_to_root.get(source) {
                                        escaped.insert(sroot);
                                    }
                                    if !spread_seen.insert(root) {
                                        escaped.insert(root);
                                    }
                                }
                                if let Some((_, _, value)) = define
                                    && let Some(&vroot) = alias_to_root.get(value)
                                {
                                    escaped.insert(vroot);
                                }
                                true
                            }
                            _ => false,
                        };
                    if !handled {
                        for o in operands {
                            if let Some(&root) = alias_to_root.get(o) {
                                escaped.insert(root);
                            }
                        }
                    }
                }
                GraphOp::Unary { value, .. } | GraphOp::IsReturnSignal(value) => {
                    if let Some(&root) = alias_to_root.get(value) {
                        escaped.insert(root);
                    }
                }
                GraphOp::Binary { left, right, .. } => {
                    if let Some(&root) = alias_to_root.get(left) {
                        escaped.insert(root);
                    }
                    if let Some(&root) = alias_to_root.get(right) {
                        escaped.insert(root);
                    }
                }
                GraphOp::Phi { inputs } => {
                    if live_values.contains(vid) || alias_to_root.contains_key(vid) {
                        let non_self: Vec<ValueId> = inputs
                            .iter()
                            .map(|(_, in_v)| *in_v)
                            .filter(|in_v| in_v != vid)
                            .collect();
                        let roots: HashSet<Option<ValueId>> = non_self
                            .iter()
                            .map(|in_v| alias_to_root.get(in_v).copied())
                            .collect();
                        if roots.len() > 1 || roots.contains(&None) {
                            for in_v in non_self {
                                if let Some(&root) = alias_to_root.get(&in_v) {
                                    escaped.insert(root);
                                }
                            }
                        }
                    }
                }
                GraphOp::GetProperty { object, name, .. } => {
                    if let Some(&root) = alias_to_root.get(object) {
                        let prop_name = match program.constants.get(*name as usize) {
                            Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
                            _ => String::new(),
                        };
                        if OBJECT_PROTOTYPE_PROPERTIES.contains(&prop_name.as_str()) {
                            let defined =
                                if let Some(GraphOp::NewObject { pairs, .. }) = defs.get(&root) {
                                    pairs.iter().any(|(k, _)| {
                                        resolve_const_string(*k, &defs, program)
                                            == Some(prop_name.clone())
                                    })
                                } else {
                                    false
                                } || defined_by_bridge(root, &prop_name);
                            if !defined {
                                escaped.insert(root);
                            }
                        }
                    }
                }
                GraphOp::SetProperty {
                    object,
                    name,
                    value,
                    ..
                }
                | GraphOp::SetPropertyStrict {
                    object,
                    name,
                    value,
                    ..
                } => {
                    if let Some(&root) = alias_to_root.get(value) {
                        escaped.insert(root);
                    }
                    if let Some(&root) = alias_to_root.get(object) {
                        let prop_name = match program.constants.get(*name as usize) {
                            Some(Constant::String(s)) => s.to_scalar_string().unwrap_or_default(),
                            _ => String::new(),
                        };
                        if prop_name == "__proto__" {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::GetComputed { object, key, .. } => {
                    if let Some(&root) = alias_to_root.get(key) {
                        escaped.insert(root);
                    }
                    if let Some(&root) = alias_to_root.get(object) {
                        if let Some(prop_name) = resolve_const_string(*key, &defs, program) {
                            if OBJECT_PROTOTYPE_PROPERTIES.contains(&prop_name.as_str()) {
                                let defined = if let Some(GraphOp::NewObject { pairs, .. }) =
                                    defs.get(&root)
                                {
                                    pairs.iter().any(|(k, _)| {
                                        resolve_const_string(*k, &defs, program)
                                            == Some(prop_name.clone())
                                    })
                                } else {
                                    false
                                };
                                if !defined {
                                    escaped.insert(root);
                                }
                            }
                        } else {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::SetComputed {
                    object, key, value, ..
                }
                | GraphOp::SetComputedStrict {
                    object, key, value, ..
                } => {
                    if let Some(&root) = alias_to_root.get(value) {
                        escaped.insert(root);
                    }
                    if let Some(&root) = alias_to_root.get(key) {
                        escaped.insert(root);
                    }
                    if let Some(&root) = alias_to_root.get(object) {
                        if let Some(prop_name) = resolve_const_string(*key, &defs, program) {
                            if prop_name == "__proto__" {
                                escaped.insert(root);
                            }
                        } else {
                            escaped.insert(root);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Step 5: Construct ScalarObjects for non-escaping allocations
    let mut sinks: HashMap<ValueId, SpreadSink> = HashMap::new();
    let mut non_escaping = BTreeSet::new();
    let mut scalar_objects = HashMap::new();

    for alloc_id in candidates {
        if escaped.contains(&alloc_id) {
            continue;
        }

        let mut field_names = BTreeSet::new();

        // 1. From NewObject pairs
        if let Some(GraphOp::NewObject { pairs, .. }) = defs.get(&alloc_id) {
            for (k, _) in pairs {
                if let Some(prop) = resolve_const_string(*k, &defs, program) {
                    field_names.insert(prop.to_owned());
                }
            }
        }

        // 2. From SetProperty / GetProperty / SetComputed / GetComputed, and the define bridges
        for block in &graph.blocks {
            for (_, op) in &block.operations {
                if with_spread
                    && let Some((target, Some((key, _, _)), _)) = spread_bridge(op, &defs, program)
                    && alias_to_root.get(target) == Some(&alloc_id)
                {
                    field_names.insert(key.to_owned());
                }
                match op {
                    GraphOp::GetProperty { object, name, .. }
                    | GraphOp::SetProperty { object, name, .. }
                    | GraphOp::SetPropertyStrict { object, name, .. }
                        if alias_to_root.get(object) == Some(&alloc_id) =>
                    {
                        if let Some(Constant::String(s)) = program.constants.get(*name as usize) {
                            if let Some(name) = s.to_scalar_string().ok() {
                                field_names.insert(name);
                            }
                        }
                    }
                    GraphOp::GetComputed { object, key, .. }
                    | GraphOp::SetComputed { object, key, .. }
                    | GraphOp::SetComputedStrict { object, key, .. }
                        if alias_to_root.get(object) == Some(&alloc_id) =>
                    {
                        if let Some(prop) = resolve_const_string(*key, &defs, program) {
                            field_names.insert(prop.to_owned());
                        }
                    }
                    _ => {}
                }
            }
        }

        let fields: BTreeMap<String, usize> = field_names
            .into_iter()
            .enumerate()
            .map(|(idx, name)| (name, idx))
            .collect();

        let mut field_by_name_index = HashMap::new();
        for (c_idx, c) in program.constants.iter().enumerate() {
            if let Constant::String(s) = c
                && let Some(name) = s.to_scalar_string().ok()
                && let Some(&field_idx) = fields.get(&name)
            {
                field_by_name_index.insert(c_idx as u32, field_idx);
            }
        }

        let mut initial_values = HashMap::new();
        let mut field_written_values: HashMap<usize, Vec<ValueId>> = HashMap::new();

        if let Some(GraphOp::NewObject { pairs, .. }) = defs.get(&alloc_id) {
            for (k, v) in pairs {
                if let Some(prop) = resolve_const_string(*k, &defs, program)
                    && let Some(&field_idx) = fields.get(&prop)
                {
                    initial_values.insert(field_idx, *v);
                    field_written_values.entry(field_idx).or_default().push(*v);
                }
            }
        }

        for block in &graph.blocks {
            for (op_vid, op) in &block.operations {
                match op {
                    GraphOp::SetProperty {
                        object,
                        name,
                        value,
                        ..
                    }
                    | GraphOp::SetPropertyStrict {
                        object,
                        name,
                        value,
                        ..
                    } if alias_to_root.get(object) == Some(&alloc_id) => {
                        if let Some(&field_idx) = field_by_name_index.get(name) {
                            field_written_values
                                .entry(field_idx)
                                .or_default()
                                .push(*value);
                        }
                    }
                    GraphOp::SetComputed {
                        object, key, value, ..
                    }
                    | GraphOp::SetComputedStrict {
                        object, key, value, ..
                    } if alias_to_root.get(object) == Some(&alloc_id) => {
                        if let Some(prop) = resolve_const_string(*key, &defs, program)
                            && let Some(&field_idx) = fields.get(&prop)
                        {
                            field_written_values
                                .entry(field_idx)
                                .or_default()
                                .push(*value);
                        }
                    }
                    GraphOp::Bridge { .. } if with_spread => {
                        if let Some((target, define, source)) = spread_bridge(op, &defs, program)
                            && alias_to_root.get(target) == Some(&alloc_id)
                        {
                            if let Some((key, _, value)) = define
                                && let Some(&field_idx) = fields.get(&key)
                            {
                                field_written_values
                                    .entry(field_idx)
                                    .or_default()
                                    .push(*value);
                            }
                            // The spread may write any named field, with a value nothing
                            // proves: the bridge itself stands in as the writer.
                            if source.is_some() {
                                for &field_idx in fields.values() {
                                    field_written_values
                                        .entry(field_idx)
                                        .or_default()
                                        .push(*op_vid);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if with_spread {
            let mut sink = SpreadSink {
                alloc: alloc_id,
                spread: alloc_id,
                source: alloc_id,
                defines: Vec::new(),
            };
            let mut has_spread = false;
            for block in &graph.blocks {
                for (vid, op) in &block.operations {
                    if let Some((target, define, source)) = spread_bridge(op, &defs, program)
                        && alias_to_root.get(target) == Some(&alloc_id)
                    {
                        if let Some(source) = source {
                            sink.spread = *vid;
                            sink.source = *source;
                            has_spread = true;
                        } else if define.is_some() {
                            sink.defines.push(*vid);
                        }
                    }
                }
            }
            if has_spread {
                sinks.insert(alloc_id, sink);
            }
        }

        non_escaping.insert(alloc_id);
        scalar_objects.insert(
            alloc_id,
            ScalarObject {
                alloc_id,
                fields,
                field_by_name_index,
                initial_values,
                field_written_values,
            },
        );
    }

    let mut alias_map = HashMap::new();
    for (vid, root) in alias_to_root {
        if non_escaping.contains(&root) {
            alias_map.insert(vid, root);
        }
    }

    (
        EscapeAnalysisResult {
            non_escaping,
            scalar_objects,
            alias_map,
        },
        sinks,
    )
}

/// The phis a merge block owns: one per local slot, allocated before the walk, and one per stack
/// slot, allocated when the first frame arrives and the depth is known.
struct PhiSet {
    locals: Vec<ValueId>,
    stack: Option<Vec<ValueId>>,
}

#[derive(Default)]
struct MergeState {
    phis: HashMap<BlockId, PhiSet>,
    inputs: HashMap<ValueId, Vec<(BlockId, ValueId)>>,
}

/// Hand `outgoing` to `target` along the edge from `from`.
///
/// A block with one predecessor takes the frame as it is. A merge block already holds phis for
/// its locals; this records `outgoing`'s value for each as that phi's input from `from`, and does
/// the same for the stack once its depth is known. A local the outgoing frame never touched is
/// read from `locals[]` in the predecessor, so the phi has a defined input on every edge; the
/// verifier then checks that each phi has exactly one input per predecessor.
fn propagate_frame(
    frames: &mut HashMap<BlockId, AbstractFrame>,
    merges: &mut MergeState,
    from: BlockId,
    target: BlockId,
    outgoing: &AbstractFrame,
    graph: &mut FunctionGraph,
    operations: &mut Vec<(ValueId, GraphOp)>,
) {
    let Some(set) = merges.phis.get_mut(&target) else {
        frames.entry(target).or_insert_with(|| outgoing.clone());
        return;
    };
    for (slot, &phi) in set.locals.iter().enumerate() {
        let value = match outgoing.locals.get(&(slot as u32)) {
            Some(&v) => v,
            None => {
                let v = graph.alloc_value();
                operations.push((v, GraphOp::LoadLocal(slot as u32)));
                v
            }
        };
        if let Some(list) = merges.inputs.get_mut(&phi)
            && !list.iter().any(|(b, _)| *b == from)
        {
            list.push((from, value));
        }
    }
    if set.stack.is_none() {
        let stack: Vec<ValueId> = outgoing.stack.iter().map(|_| graph.alloc_value()).collect();
        for &phi in &stack {
            merges.inputs.insert(phi, Vec::new());
        }
        if let Some(frame) = frames.get_mut(&target) {
            frame.stack = stack.clone();
        }
        set.stack = Some(stack);
    }
    if let Some(stack) = &set.stack {
        // A depth mismatch leaves a phi short of an input, which the verifier rejects.
        for (phi, value) in stack.iter().zip(outgoing.stack.iter()) {
            if let Some(list) = merges.inputs.get_mut(phi)
                && !list.iter().any(|(b, _)| *b == from)
            {
                list.push((from, *value));
            }
        }
    }
}

#[cfg(test)]
mod dead_phi_tests {
    use super::*;
    use crate::verify_function_graph;
    use kiln_ir::Span;

    #[test]
    fn delegate_resume_and_completion_edges_supply_phi_inputs() {
        // The post-YieldStar loop is intentionally tiny: block 3 is the ordinary resume path,
        // while block 8 is both the ordinary done path and the delegate-completion path. Both
        // targets must have complete phis before C emission is allowed to run.
        let proto = FunctionProto {
            inferred_name: None,
            span: Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: Some("delegate_edges".into()),
            arrow: false,
            generator: true,
            async_generator: false,
            is_method: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 1,
            code: vec![
                Instruction::Const(0),      // wrapped flag placeholder
                Instruction::Const(0),      // iterator placeholder
                Instruction::Const(0),      // yielded value
                Instruction::YieldStar(8),  // synthetic edges to pc 4 and pc 8
                Instruction::StoreLocal(0), // resume sent value
                Instruction::Jump(0),       // loop back to the delegate
                Instruction::Const(0),      // ordinary done value
                Instruction::Jump(8),       // ordinary predecessor of the exit
                Instruction::Return,
            ],
            upvalues: Vec::new(),
            type_annotations: None,
        };
        let mut program = Program::default();
        program.constants.push(Constant::Undefined);
        let graph = build_function_graph(&proto, &program).expect("delegate graph builds");
        verify_function_graph(&graph).expect("synthetic resume edges make phis complete");
        let resume_values: Vec<_> = graph
            .blocks
            .iter()
            .flat_map(|block| block.operations.iter())
            .filter_map(|(value, op)| matches!(op, GraphOp::ResumeValue).then_some(*value))
            .collect();
        assert!(!resume_values.is_empty());
        let proven_numbers = proven_numbers(&graph);
        assert!(
            resume_values
                .iter()
                .all(|value| !proven_numbers.contains(value))
        );

        let exit = graph
            .blocks
            .iter()
            .find(|block| matches!(block.terminator, Terminator::Return(_)))
            .expect("exit block");
        assert!(
            exit.predecessors.len() >= 2,
            "ordinary and delegate completion edges"
        );
        assert!(exit.operations.iter().any(|(_, op)| matches!(op, GraphOp::Phi { inputs } if inputs.len() == exit.predecessors.len())));
    }

    #[test]
    fn removes_dead_cycles_but_keeps_live_parallel_copies() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 6;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(1),
                    },
                )],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: [(1, 2), (2, 1), (3, 4), (4, 3), (5, 4)]
                    .into_iter()
                    .map(|(to, from)| {
                        (
                            ValueId(to),
                            GraphOp::Phi {
                                inputs: vec![(BlockId(0), ValueId(0)), (BlockId(1), ValueId(from))],
                            },
                        )
                    })
                    .collect(),
                terminator: Terminator::Branch {
                    condition: ValueId(1),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![BlockId(0), BlockId(1)],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![],
                terminator: Terminator::Return(Some(ValueId(2))),
                predecessors: vec![BlockId(1)],
                successors: vec![],
                exception_handler: None,
            },
        ];
        crate::verifier::verify_function_graph(&graph).unwrap();
        remove_dead_phis(&mut graph);
        crate::verifier::verify_function_graph(&graph).unwrap();
        assert_eq!(
            graph.blocks[1]
                .operations
                .iter()
                .map(|(v, _)| v.0)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        let once = graph.clone();
        remove_dead_phis(&mut graph);
        assert_eq!(graph, once);
    }

    #[test]
    fn trivial_phis_and_dense_ids_preserve_live_choices_and_origins() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 6;
        graph.initial_locals = vec![ValueId(0)];
        graph.origins.insert(
            ValueId(4),
            Origin {
                pc: 17,
                label: "choice".into(),
            },
        );
        graph.origins.insert(
            ValueId(2),
            Origin {
                pc: 3,
                label: "deleted phi".into(),
            },
        );
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![
                    (
                        ValueId(0),
                        GraphOp::Const {
                            index: 0,
                            constant: Constant::Number(1),
                        },
                    ),
                    (
                        ValueId(1),
                        GraphOp::Const {
                            index: 1,
                            constant: Constant::Number(2),
                        },
                    ),
                ],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![
                    (
                        ValueId(2),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(0), ValueId(0)), (BlockId(1), ValueId(2))],
                        },
                    ),
                    (
                        ValueId(3),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(0), ValueId(0)), (BlockId(1), ValueId(2))],
                        },
                    ),
                    (
                        ValueId(4),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(0), ValueId(0)), (BlockId(1), ValueId(1))],
                        },
                    ),
                    (
                        ValueId(5),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(3),
                            right: ValueId(4),
                        },
                    ),
                ],
                terminator: Terminator::Branch {
                    condition: ValueId(5),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![BlockId(0), BlockId(1)],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![],
                terminator: Terminator::Return(Some(ValueId(5))),
                predecessors: vec![BlockId(1)],
                successors: vec![],
                exception_handler: None,
            },
        ];
        crate::verifier::verify_function_graph(&graph).unwrap();
        simplify_trivial_phis(&mut graph);
        compact_value_ids(&mut graph);
        crate::verifier::verify_function_graph(&graph).unwrap();
        assert_eq!(graph.next_value_id, 4);
        assert_eq!(graph.initial_locals, vec![ValueId(0)]);
        assert_eq!(graph.origins.len(), 1);
        assert_eq!(graph.origins[&ValueId(2)].pc, 17);
        assert!(matches!(
            graph.blocks[1].operations[1],
            (
                ValueId(3),
                GraphOp::Binary {
                    left: ValueId(0),
                    right: ValueId(2),
                    ..
                }
            )
        ));
        let once = graph.clone();
        simplify_trivial_phis(&mut graph);
        compact_value_ids(&mut graph);
        assert_eq!(graph, once);
    }

    #[test]
    fn preserves_positional_resume_phis_even_when_unused() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 3;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(1),
                    },
                )],
                terminator: Terminator::Suspend {
                    value: ValueId(0),
                    spilled: vec![ValueId(0)],
                    resume: BlockId(1),
                    pc: 1,
                    awaiting: false,
                    delegate: None,
                },
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![
                    (
                        ValueId(1),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(0), ValueId(0))],
                        },
                    ),
                    (ValueId(2), GraphOp::Phi { inputs: vec![] }),
                ],
                terminator: Terminator::Return(Some(ValueId(2))),
                predecessors: vec![BlockId(0)],
                successors: vec![],
                exception_handler: None,
            },
        ];
        let before = graph.clone();
        remove_dead_phis(&mut graph);
        simplify_trivial_phis(&mut graph);
        compact_value_ids(&mut graph);
        assert_eq!(graph, before);
    }

    #[test]
    fn eliminate_dead_code_removes_unused_pure_ops_and_keeps_effects() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![BasicBlock {
            id: BlockId(0),
            operations: vec![
                // Dead pure constant
                (
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(10),
                    },
                ),
                // Dead pure binary op using dead constant
                (
                    ValueId(1),
                    GraphOp::Binary {
                        op: BinaryOp::Add,
                        left: ValueId(0),
                        right: ValueId(0),
                    },
                ),
                // Live pure constant (used in return)
                (
                    ValueId(2),
                    GraphOp::Const {
                        index: 1,
                        constant: Constant::Number(42),
                    },
                ),
                // Side-effecting op (CheckTdz) with no consumer - must NOT be removed
                (
                    ValueId(3),
                    GraphOp::CheckTdz {
                        slot: 0,
                        value: ValueId(2),
                    },
                ),
            ],
            terminator: Terminator::Return(Some(ValueId(2))),
            predecessors: vec![],
            successors: vec![],
            exception_handler: None,
        }];

        eliminate_dead_code(&mut graph);
        let remaining_ops: Vec<ValueId> =
            graph.blocks[0].operations.iter().map(|(v, _)| *v).collect();
        // Values 0 and 1 are dead pure operations and must be eliminated
        assert!(!remaining_ops.contains(&ValueId(0)));
        assert!(!remaining_ops.contains(&ValueId(1)));
        // Values 2 (live return) and 3 (side-effecting CheckTdz) must be preserved
        assert!(remaining_ops.contains(&ValueId(2)));
        assert!(remaining_ops.contains(&ValueId(3)));
        crate::verifier::verify_function_graph(&graph).unwrap();
    }

    #[test]
    fn hoist_loop_invariants_moves_pure_ops_to_preheader() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![
            // Preheader: Block 0
            BasicBlock {
                id: BlockId(0),
                operations: vec![
                    (
                        ValueId(0),
                        GraphOp::Const {
                            index: 0,
                            constant: Constant::Number(5),
                        },
                    ),
                    (
                        ValueId(1),
                        GraphOp::Const {
                            index: 1,
                            constant: Constant::Number(10),
                        },
                    ),
                ],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            // Loop Header: Block 1
            BasicBlock {
                id: BlockId(1),
                operations: vec![
                    (
                        ValueId(2),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(0), ValueId(0)), (BlockId(2), ValueId(6))],
                        },
                    ),
                    (
                        ValueId(3),
                        GraphOp::Binary {
                            op: BinaryOp::LessThan,
                            left: ValueId(2),
                            right: ValueId(1),
                        },
                    ),
                ],
                terminator: Terminator::Branch {
                    condition: ValueId(3),
                    true_block: BlockId(2),
                    false_block: BlockId(3),
                },
                predecessors: vec![BlockId(0), BlockId(2)],
                successors: vec![BlockId(2), BlockId(3)],
                exception_handler: None,
            },
            // Loop Body: Block 2
            BasicBlock {
                id: BlockId(2),
                operations: vec![
                    // Invariant pure operation inside loop body: 5 * 10
                    (
                        ValueId(4),
                        GraphOp::Binary {
                            op: BinaryOp::Multiply,
                            left: ValueId(0),
                            right: ValueId(1),
                        },
                    ),
                    // Variant operation depending on phi
                    (
                        ValueId(5),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(2),
                            right: ValueId(4),
                        },
                    ),
                    // Step i++
                    (
                        ValueId(6),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(2),
                            right: ValueId(0),
                        },
                    ),
                ],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![BlockId(1)],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            // Exit: Block 3
            BasicBlock {
                id: BlockId(3),
                operations: vec![],
                terminator: Terminator::Return(Some(ValueId(2))),
                predecessors: vec![BlockId(1)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        crate::verifier::verify_function_graph(&graph).unwrap();
        hoist_loop_invariants(&mut graph);

        // Value 4 (invariant multiply) should now be in Block 0 (preheader)
        let preheader_ops: Vec<ValueId> =
            graph.blocks[0].operations.iter().map(|(v, _)| *v).collect();
        assert!(
            preheader_ops.contains(&ValueId(4)),
            "invariant op must be hoisted to preheader"
        );

        // Value 4 should no longer be in Block 2 (loop body)
        let body_ops: Vec<ValueId> = graph.blocks[2].operations.iter().map(|(v, _)| *v).collect();
        assert!(
            !body_ops.contains(&ValueId(4)),
            "hoisted op must be removed from loop body"
        );

        // Dominance and verifier invariants must hold
        crate::verifier::verify_function_graph(&graph).unwrap();
    }

    #[test]
    fn hoist_loop_invariants_skips_loops_with_suspension() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![
            // Preheader: Block 0
            BasicBlock {
                id: BlockId(0),
                operations: vec![
                    (
                        ValueId(0),
                        GraphOp::Const {
                            index: 0,
                            constant: Constant::Number(5),
                        },
                    ),
                    (
                        ValueId(1),
                        GraphOp::Const {
                            index: 1,
                            constant: Constant::Number(10),
                        },
                    ),
                ],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            // Loop Header: Block 1
            BasicBlock {
                id: BlockId(1),
                operations: vec![
                    (
                        ValueId(2),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(0), ValueId(0)), (BlockId(2), ValueId(6))],
                        },
                    ),
                    (
                        ValueId(3),
                        GraphOp::Binary {
                            op: BinaryOp::LessThan,
                            left: ValueId(2),
                            right: ValueId(1),
                        },
                    ),
                ],
                terminator: Terminator::Branch {
                    condition: ValueId(3),
                    true_block: BlockId(2),
                    false_block: BlockId(3),
                },
                predecessors: vec![BlockId(0), BlockId(2)],
                successors: vec![BlockId(2), BlockId(3)],
                exception_handler: None,
            },
            // Loop Body: Block 2 (contains a suspension terminator)
            BasicBlock {
                id: BlockId(2),
                operations: vec![
                    (
                        ValueId(4),
                        GraphOp::Binary {
                            op: BinaryOp::Multiply,
                            left: ValueId(0),
                            right: ValueId(1),
                        },
                    ),
                    (
                        ValueId(6),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(2),
                            right: ValueId(0),
                        },
                    ),
                ],
                terminator: Terminator::Suspend {
                    value: ValueId(4),
                    resume: BlockId(1),
                    spilled: vec![],
                    pc: 42,
                    awaiting: false,
                    delegate: None,
                },
                predecessors: vec![BlockId(1)],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            // Exit: Block 3
            BasicBlock {
                id: BlockId(3),
                operations: vec![],
                terminator: Terminator::Return(Some(ValueId(2))),
                predecessors: vec![BlockId(1)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        hoist_loop_invariants(&mut graph);

        // Value 4 should NOT be hoisted to preheader because the loop suspends
        let preheader_ops: Vec<ValueId> =
            graph.blocks[0].operations.iter().map(|(v, _)| *v).collect();
        assert!(
            !preheader_ops.contains(&ValueId(4)),
            "suspending loop must not hoist invariants to preheader"
        );
        let body_ops: Vec<ValueId> = graph.blocks[2].operations.iter().map(|(v, _)| *v).collect();
        assert!(
            body_ops.contains(&ValueId(4)),
            "invariant op must remain in loop body"
        );
    }

    #[test]
    fn canonicalize_loop_preheaders_synthesizes_trampoline_for_multi_pred_loop() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![
            // Block 0: outside pred 1
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(1),
                    },
                )],
                terminator: Terminator::Branch {
                    condition: ValueId(0),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            // Block 1: outside pred 2
            BasicBlock {
                id: BlockId(1),
                operations: vec![(
                    ValueId(1),
                    GraphOp::Const {
                        index: 1,
                        constant: Constant::Number(2),
                    },
                )],
                terminator: Terminator::Jump(BlockId(2)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(2)],
                exception_handler: None,
            },
            // Block 2: Loop Header (has 2 outside preds: 0 and 1, plus back edge from 3)
            BasicBlock {
                id: BlockId(2),
                operations: vec![(
                    ValueId(2),
                    GraphOp::Phi {
                        inputs: vec![
                            (BlockId(0), ValueId(0)),
                            (BlockId(1), ValueId(1)),
                            (BlockId(3), ValueId(5)),
                        ],
                    },
                )],
                terminator: Terminator::Branch {
                    condition: ValueId(2),
                    true_block: BlockId(3),
                    false_block: BlockId(4),
                },
                predecessors: vec![BlockId(0), BlockId(1), BlockId(3)],
                successors: vec![BlockId(3), BlockId(4)],
                exception_handler: None,
            },
            // Block 3: Loop Body (back edge to 2)
            BasicBlock {
                id: BlockId(3),
                operations: vec![(
                    ValueId(5),
                    GraphOp::Binary {
                        op: BinaryOp::Add,
                        left: ValueId(2),
                        right: ValueId(0),
                    },
                )],
                terminator: Terminator::Jump(BlockId(2)),
                predecessors: vec![BlockId(2)],
                successors: vec![BlockId(2)],
                exception_handler: None,
            },
            // Block 4: Exit
            BasicBlock {
                id: BlockId(4),
                operations: vec![],
                terminator: Terminator::Return(Some(ValueId(2))),
                predecessors: vec![BlockId(2)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        canonicalize_loop_preheaders(&mut graph);

        // A new preheader should have been synthesized
        assert_eq!(graph.blocks.len(), 6);
        let header = graph.block(BlockId(2)).unwrap();
        // The header must now have exactly 2 predecessors: the synthesized preheader and back-edge Block 3
        assert_eq!(header.predecessors.len(), 2);
        assert!(header.predecessors.contains(&BlockId(3)));
        let preheader_id = header
            .predecessors
            .iter()
            .copied()
            .find(|&p| p != BlockId(3))
            .unwrap();

        let preheader = graph.block(preheader_id).unwrap();
        assert_eq!(preheader.terminator, Terminator::Jump(BlockId(2)));
        assert_eq!(preheader.successors, vec![BlockId(2)]);
        assert!(preheader.predecessors.contains(&BlockId(0)));
        assert!(preheader.predecessors.contains(&BlockId(1)));

        // Verify that outside predecessors now jump/branch to preheader_id instead of Block 2
        let block0 = graph.block(BlockId(0)).unwrap();
        assert_eq!(
            block0.terminator,
            Terminator::Branch {
                condition: ValueId(0),
                true_block: BlockId(1),
                false_block: preheader_id,
            }
        );
        let block1 = graph.block(BlockId(1)).unwrap();
        assert_eq!(block1.terminator, Terminator::Jump(preheader_id));

        // Header phi inputs should now come from preheader_id and Block 3
        let (_, header_phi) = &header.operations[0];
        if let GraphOp::Phi { inputs } = header_phi {
            assert_eq!(inputs.len(), 2);
            assert!(inputs.iter().any(|(b, _)| *b == preheader_id));
            assert!(
                inputs
                    .iter()
                    .any(|(b, v)| *b == BlockId(3) && *v == ValueId(5))
            );
        } else {
            panic!("expected Phi operation in header");
        }
    }

    #[test]
    fn eliminate_common_subexpressions_deduplicates_pure_ops_and_canonicalizes_commutative() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 20;
        graph.blocks = vec![
            // Block 0: defines constants and binary expression v2 = v0 + v1
            BasicBlock {
                id: BlockId(0),
                operations: vec![
                    (
                        ValueId(0),
                        GraphOp::Const {
                            index: 0,
                            constant: Constant::Number(42),
                        },
                    ),
                    (
                        ValueId(1),
                        GraphOp::Const {
                            index: 1,
                            constant: Constant::Number(100),
                        },
                    ),
                    (
                        ValueId(2),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(0),
                            right: ValueId(1),
                        },
                    ),
                ],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            // Block 1 (dominated by Block 0): defines v3 = v1 + v0 (commutative duplicate of v2!)
            // and v4 = v3 + v2 (which should become v2 + v2)
            BasicBlock {
                id: BlockId(1),
                operations: vec![
                    (
                        ValueId(3),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(1),
                            right: ValueId(0),
                        },
                    ),
                    (
                        ValueId(4),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(3),
                            right: ValueId(2),
                        },
                    ),
                ],
                terminator: Terminator::Return(Some(ValueId(4))),
                predecessors: vec![BlockId(0)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        eliminate_common_subexpressions(&mut graph);

        let block1 = graph.block(BlockId(1)).unwrap();
        // v3 must have been eliminated
        assert!(
            !block1.operations.iter().any(|(v, _)| *v == ValueId(3)),
            "v3 should have been eliminated by CSE"
        );
        // v4 must now reference v2 instead of v3
        let v4_op = block1
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(4))
            .map(|(_, op)| op)
            .unwrap();
        assert_eq!(
            v4_op,
            &GraphOp::Binary {
                op: BinaryOp::Add,
                left: ValueId(2),
                right: ValueId(2),
            }
        );
    }

    #[test]
    fn eliminate_common_subexpressions_does_not_dedup_across_disjoint_branches() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 20;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![
                    (
                        ValueId(0),
                        GraphOp::Const {
                            index: 0,
                            constant: Constant::Number(1),
                        },
                    ),
                    (
                        ValueId(1),
                        GraphOp::Const {
                            index: 1,
                            constant: Constant::Number(2),
                        },
                    ),
                ],
                terminator: Terminator::Branch {
                    condition: ValueId(0),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            // Block 1 (left branch)
            BasicBlock {
                id: BlockId(1),
                operations: vec![(
                    ValueId(2),
                    GraphOp::Binary {
                        op: BinaryOp::Add,
                        left: ValueId(0),
                        right: ValueId(1),
                    },
                )],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            // Block 2 (right branch - does NOT dominate and is NOT dominated by Block 1)
            BasicBlock {
                id: BlockId(2),
                operations: vec![(
                    ValueId(3),
                    GraphOp::Binary {
                        op: BinaryOp::Add,
                        left: ValueId(0),
                        right: ValueId(1),
                    },
                )],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            // Block 3 (merge)
            BasicBlock {
                id: BlockId(3),
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![BlockId(1), BlockId(2)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        eliminate_common_subexpressions(&mut graph);

        // Neither v2 nor v3 should be eliminated because neither block dominates the other
        let block1 = graph.block(BlockId(1)).unwrap();
        assert!(block1.operations.iter().any(|(v, _)| *v == ValueId(2)));
        let block2 = graph.block(BlockId(2)).unwrap();
        assert!(block2.operations.iter().any(|(v, _)| *v == ValueId(3)));
    }

    #[test]
    fn fold_undefined_returns_simplifies_terminator_and_dce_prunes_const() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 2;
        graph.blocks = vec![BasicBlock {
            id: BlockId(0),
            operations: vec![(
                ValueId(0),
                GraphOp::Const {
                    index: 0,
                    constant: Constant::Undefined,
                },
            )],
            terminator: Terminator::Return(Some(ValueId(0))),
            predecessors: vec![],
            successors: vec![],
            exception_handler: None,
        }];

        fold_undefined_returns(&mut graph);
        assert_eq!(graph.blocks[0].terminator, Terminator::Return(None));

        eliminate_dead_code(&mut graph);
        assert!(graph.blocks[0].operations.is_empty());
    }

    #[test]
    fn eliminate_common_subexpressions_works_in_single_block() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![BasicBlock {
            id: BlockId(0),
            operations: vec![
                (
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(10),
                    },
                ),
                (
                    ValueId(1),
                    GraphOp::Const {
                        index: 1,
                        constant: Constant::Number(20),
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::Binary {
                        op: BinaryOp::Add,
                        left: ValueId(0),
                        right: ValueId(1),
                    },
                ),
                (
                    ValueId(3),
                    GraphOp::Binary {
                        op: BinaryOp::Add,
                        left: ValueId(1),
                        right: ValueId(0),
                    },
                ),
                (
                    ValueId(4),
                    GraphOp::Binary {
                        op: BinaryOp::Multiply,
                        left: ValueId(3),
                        right: ValueId(2),
                    },
                ),
            ],
            terminator: Terminator::Return(Some(ValueId(4))),
            predecessors: vec![],
            successors: vec![],
            exception_handler: None,
        }];

        eliminate_common_subexpressions(&mut graph);

        let block0 = graph.block(BlockId(0)).unwrap();
        // v3 should have been eliminated
        assert!(
            !block0.operations.iter().any(|(v, _)| *v == ValueId(3)),
            "v3 should be eliminated in a single-block function"
        );
        // v4 should now be v2 * v2
        let v4_op = block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(4))
            .map(|(_, op)| op)
            .unwrap();
        assert_eq!(
            v4_op,
            &GraphOp::Binary {
                op: BinaryOp::Multiply,
                left: ValueId(2),
                right: ValueId(2),
            }
        );
    }

    #[test]
    fn eliminate_common_subexpressions_deduplicates_identical_phis() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![
                    (
                        ValueId(0),
                        GraphOp::Const {
                            index: 0,
                            constant: Constant::Number(1),
                        },
                    ),
                    (
                        ValueId(1),
                        GraphOp::Const {
                            index: 1,
                            constant: Constant::Number(2),
                        },
                    ),
                ],
                terminator: Terminator::Branch {
                    condition: ValueId(0),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            // Merge block 3 with two identical Phis:
            // v2 = Phi([(BlockId(1), ValueId(0)), (BlockId(2), ValueId(1))])
            // v3 = Phi([(BlockId(1), ValueId(0)), (BlockId(2), ValueId(1))])
            // v4 = v2 + v3
            BasicBlock {
                id: BlockId(3),
                operations: vec![
                    (
                        ValueId(2),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(1), ValueId(0)), (BlockId(2), ValueId(1))],
                        },
                    ),
                    (
                        ValueId(3),
                        GraphOp::Phi {
                            inputs: vec![(BlockId(1), ValueId(0)), (BlockId(2), ValueId(1))],
                        },
                    ),
                    (
                        ValueId(4),
                        GraphOp::Binary {
                            op: BinaryOp::Add,
                            left: ValueId(2),
                            right: ValueId(3),
                        },
                    ),
                ],
                terminator: Terminator::Return(Some(ValueId(4))),
                predecessors: vec![BlockId(1), BlockId(2)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        eliminate_common_subexpressions(&mut graph);

        let block3 = graph.block(BlockId(3)).unwrap();
        // v3 must be eliminated
        assert!(
            !block3.operations.iter().any(|(v, _)| *v == ValueId(3)),
            "identical Phi v3 must be eliminated"
        );
        // v4 should now be v2 + v2
        let v4_op = block3
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(4))
            .map(|(_, op)| op)
            .unwrap();
        assert_eq!(
            v4_op,
            &GraphOp::Binary {
                op: BinaryOp::Add,
                left: ValueId(2),
                right: ValueId(2),
            }
        );
    }

    #[test]
    fn fold_constants_evaluates_chained_arithmetic_and_comparisons() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 20;
        graph.blocks = vec![BasicBlock {
            id: BlockId(0),
            operations: vec![
                (
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(10.0f64.to_bits()),
                    },
                ),
                (
                    ValueId(1),
                    GraphOp::Const {
                        index: 1,
                        constant: Constant::Number(20.0f64.to_bits()),
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::Const {
                        index: 2,
                        constant: Constant::Number(2.0f64.to_bits()),
                    },
                ),
                // v3 = 10 + 20 (= 30)
                (
                    ValueId(3),
                    GraphOp::Binary {
                        op: BinaryOp::Add,
                        left: ValueId(0),
                        right: ValueId(1),
                    },
                ),
                // v4 = v3 * 2 (= 60)
                (
                    ValueId(4),
                    GraphOp::Binary {
                        op: BinaryOp::Multiply,
                        left: ValueId(3),
                        right: ValueId(2),
                    },
                ),
                // v5 = v4 - 10 (= 50)
                (
                    ValueId(5),
                    GraphOp::Binary {
                        op: BinaryOp::Subtract,
                        left: ValueId(4),
                        right: ValueId(0),
                    },
                ),
                // v6 = -v5 (= -50)
                (
                    ValueId(6),
                    GraphOp::Unary {
                        op: UnaryOp::Negate,
                        value: ValueId(5),
                    },
                ),
                // v7 = v6 < 0 (= true)
                (
                    ValueId(7),
                    GraphOp::Binary {
                        op: BinaryOp::LessThan,
                        left: ValueId(6),
                        right: ValueId(0),
                    },
                ),
                // v8 = !v7 (= false)
                (
                    ValueId(8),
                    GraphOp::Unary {
                        op: UnaryOp::Not,
                        value: ValueId(7),
                    },
                ),
            ],
            terminator: Terminator::Return(Some(ValueId(8))),
            predecessors: vec![],
            successors: vec![],
            exception_handler: None,
        }];

        fold_constants(&mut graph);

        let block0 = graph.block(BlockId(0)).unwrap();
        // Check v3 folded to 30
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(3))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), 30.0);
            }
            _other => panic!("expected Const 30"),
        }
        // Check v4 folded to 60
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(4))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), 60.0);
            }
            _other => panic!("expected Const 60"),
        }
        // Check v5 folded to 50
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(5))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), 50.0);
            }
            _other => panic!("expected Const 50"),
        }
        // Check v6 folded to -50
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(6))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), -50.0);
            }
            _other => panic!("expected Const -50"),
        }
        // Check v7 folded to Boolean(true)
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(7))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Boolean(b),
                ..
            } => {
                assert_eq!(*b, true);
            }
            _other => panic!("expected Const true"),
        }
        // Check v8 folded to Boolean(false)
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(8))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Boolean(b),
                ..
            } => {
                assert_eq!(*b, false);
            }
            _other => panic!("expected Const false"),
        }
    }

    #[test]
    fn fold_constants_bitwise_and_division() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 20;
        graph.blocks = vec![BasicBlock {
            id: BlockId(0),
            operations: vec![
                (
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(12.0f64.to_bits()),
                    },
                ),
                (
                    ValueId(1),
                    GraphOp::Const {
                        index: 1,
                        constant: Constant::Number(5.0f64.to_bits()),
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::Const {
                        index: 2,
                        constant: Constant::Number(0.0f64.to_bits()),
                    },
                ),
                // v3 = 12 & 5 (= 4)
                (
                    ValueId(3),
                    GraphOp::Binary {
                        op: BinaryOp::BitwiseAnd,
                        left: ValueId(0),
                        right: ValueId(1),
                    },
                ),
                // v4 = 12 | 5 (= 13)
                (
                    ValueId(4),
                    GraphOp::Binary {
                        op: BinaryOp::BitwiseOr,
                        left: ValueId(0),
                        right: ValueId(1),
                    },
                ),
                // v5 = 12 ^ 5 (= 9)
                (
                    ValueId(5),
                    GraphOp::Binary {
                        op: BinaryOp::BitwiseXor,
                        left: ValueId(0),
                        right: ValueId(1),
                    },
                ),
                // v6 = ~12 (= -13)
                (
                    ValueId(6),
                    GraphOp::Unary {
                        op: UnaryOp::BitwiseNot,
                        value: ValueId(0),
                    },
                ),
                // v7 = 12 / 0 (= Infinity)
                (
                    ValueId(7),
                    GraphOp::Binary {
                        op: BinaryOp::Divide,
                        left: ValueId(0),
                        right: ValueId(2),
                    },
                ),
            ],
            terminator: Terminator::Return(None),
            predecessors: vec![],
            successors: vec![],
            exception_handler: None,
        }];

        fold_constants(&mut graph);

        let block0 = graph.block(BlockId(0)).unwrap();
        // Check v3 folded to 4
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(3))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), 4.0);
            }
            _other => panic!("expected Const 4"),
        }
        // Check v4 folded to 13
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(4))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), 13.0);
            }
            _other => panic!("expected Const 13"),
        }
        // Check v5 folded to 9
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(5))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), 9.0);
            }
            _other => panic!("expected Const 9"),
        }
        // Check v6 folded to -13
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(6))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert_eq!(f64::from_bits(*bits), -13.0);
            }
            _other => panic!("expected Const -13"),
        }
        // Check v7 folded to Infinity
        match &block0
            .operations
            .iter()
            .find(|(v, _)| *v == ValueId(7))
            .unwrap()
            .1
        {
            GraphOp::Const {
                constant: Constant::Number(bits),
                ..
            } => {
                assert!(f64::from_bits(*bits).is_infinite());
            }
            _other => panic!("expected Const Infinity"),
        }
    }

    #[test]
    fn fold_conditional_branches_truthy_prunes_false_block() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Boolean(true),
                    },
                )],
                terminator: Terminator::Branch {
                    condition: ValueId(0),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![(
                    ValueId(1),
                    GraphOp::Const {
                        index: 1,
                        constant: Constant::Number(42.0f64.to_bits()),
                    },
                )],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![(
                    ValueId(2),
                    GraphOp::Const {
                        index: 2,
                        constant: Constant::Number(99.0f64.to_bits()),
                    },
                )],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(3),
                operations: vec![(
                    ValueId(3),
                    GraphOp::Phi {
                        inputs: vec![(BlockId(1), ValueId(1)), (BlockId(2), ValueId(2))],
                    },
                )],
                terminator: Terminator::Return(Some(ValueId(3))),
                predecessors: vec![BlockId(1), BlockId(2)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        let changed = fold_conditional_branches(&mut graph);
        assert!(changed);

        // Block 0 should now Jump(BlockId(1))
        assert_eq!(graph.blocks[0].terminator, Terminator::Jump(BlockId(1)));
        assert_eq!(graph.blocks[0].successors, vec![BlockId(1)]);

        // Block 2 should be completely pruned!
        assert!(
            !graph.blocks.iter().any(|b| b.id == BlockId(2)),
            "Block 2 must be pruned as unreachable"
        );

        // Block 3 should only have Block 1 as predecessor
        let block3 = graph.block(BlockId(3)).unwrap();
        assert_eq!(block3.predecessors, vec![BlockId(1)]);

        // Block 3's phi should only have input from Block 1
        if let GraphOp::Phi { inputs } = &block3.operations[0].1 {
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].0, BlockId(1));
        } else {
            panic!("expected Phi");
        }

        // Must pass graph verification!
        crate::verifier::verify_function_graph(&graph).expect("graph verifies");

        // Simplifying trivial phis should resolve the phi to v1
        simplify_trivial_phis(&mut graph);
        eliminate_dead_code(&mut graph);

        let block3 = graph.block(BlockId(3)).unwrap();
        // Phi should be eliminated and terminator should return v1
        assert!(block3.operations.is_empty());
        assert_eq!(block3.terminator, Terminator::Return(Some(ValueId(1))));

        crate::verifier::verify_function_graph(&graph).expect("verified after phi simplification");
    }

    #[test]
    fn fold_conditional_branches_falsy_prunes_true_block() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Boolean(false),
                    },
                )],
                terminator: Terminator::Branch {
                    condition: ValueId(0),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![(
                    ValueId(2),
                    GraphOp::Const {
                        index: 2,
                        constant: Constant::Number(99.0f64.to_bits()),
                    },
                )],
                terminator: Terminator::Jump(BlockId(3)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(3)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(3),
                operations: vec![(
                    ValueId(3),
                    GraphOp::Phi {
                        inputs: vec![(BlockId(1), ValueId(0)), (BlockId(2), ValueId(2))],
                    },
                )],
                terminator: Terminator::Return(Some(ValueId(3))),
                predecessors: vec![BlockId(1), BlockId(2)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        let changed = fold_conditional_branches(&mut graph);
        assert!(changed);

        // Block 0 should now Jump(BlockId(2))
        assert_eq!(graph.blocks[0].terminator, Terminator::Jump(BlockId(2)));

        // Block 1 should be completely pruned
        assert!(
            !graph.blocks.iter().any(|b| b.id == BlockId(1)),
            "Block 1 must be pruned"
        );

        // Block 3 should only have Block 2 as predecessor
        let block3 = graph.block(BlockId(3)).unwrap();
        assert_eq!(block3.predecessors, vec![BlockId(2)]);

        crate::verifier::verify_function_graph(&graph).expect("graph verifies");
    }

    #[test]
    fn fold_conditional_branches_nullish_semantics() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        // Test BranchNotNullish on 0: in JS, 0 is NOT nullish, so it takes true_block!
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(0.0f64.to_bits()),
                    },
                )],
                terminator: Terminator::BranchNotNullish {
                    condition: ValueId(0),
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![BlockId(0)],
                successors: vec![],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![BlockId(0)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        let changed = fold_conditional_branches(&mut graph);
        assert!(changed);

        // 0 is not nullish -> Jump(BlockId(1))
        assert_eq!(graph.blocks[0].terminator, Terminator::Jump(BlockId(1)));
        assert!(!graph.blocks.iter().any(|b| b.id == BlockId(2)));
        crate::verifier::verify_function_graph(&graph).expect("graph verifies");
    }

    #[test]
    fn prune_unreachable_blocks_preserves_exception_handlers() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = 10;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![],
                successors: vec![],
                exception_handler: Some(BlockId(1)),
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![],
                successors: vec![],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2), // Completely unreachable
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![],
                successors: vec![],
                exception_handler: None,
            },
        ];

        prune_unreachable_blocks(&mut graph);

        // Block 1 should be preserved because it is an exception handler
        assert!(graph.blocks.iter().any(|b| b.id == BlockId(1)));
        // Block 2 should be pruned
        assert!(!graph.blocks.iter().any(|b| b.id == BlockId(2)));
    }

    #[test]
    fn merge_basic_blocks_simple_chain() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        let v0 = ValueId(0);
        graph.next_value_id = 1;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    v0,
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(42.0f64.to_bits()),
                    },
                )],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![],
                terminator: Terminator::Return(Some(v0)),
                predecessors: vec![BlockId(0)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        let changed = merge_basic_blocks(&mut graph);
        assert!(changed);
        assert_eq!(graph.blocks.len(), 1);
        assert_eq!(graph.blocks[0].id, BlockId(0));
        assert_eq!(graph.blocks[0].terminator, Terminator::Return(Some(v0)));
        assert_eq!(graph.blocks[0].operations.len(), 1);
        crate::verifier::verify_function_graph(&graph).expect("merged graph verifies");
    }

    #[test]
    fn merge_basic_blocks_multi_block_chain() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        let v0 = ValueId(0);
        let v1 = ValueId(1);
        graph.next_value_id = 2;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    v0,
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Number(10.0f64.to_bits()),
                    },
                )],
                terminator: Terminator::Jump(BlockId(1)),
                predecessors: vec![],
                successors: vec![BlockId(1)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![(
                    v1,
                    GraphOp::Const {
                        index: 1,
                        constant: Constant::Number(20.0f64.to_bits()),
                    },
                )],
                terminator: Terminator::Jump(BlockId(2)),
                predecessors: vec![BlockId(0)],
                successors: vec![BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![],
                terminator: Terminator::Return(Some(v1)),
                predecessors: vec![BlockId(1)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        let changed = merge_basic_blocks(&mut graph);
        assert!(changed);
        assert_eq!(graph.blocks.len(), 1);
        assert_eq!(graph.blocks[0].id, BlockId(0));
        assert_eq!(graph.blocks[0].terminator, Terminator::Return(Some(v1)));
        assert_eq!(graph.blocks[0].operations.len(), 2);
        crate::verifier::verify_function_graph(&graph).expect("merged multi-chain verifies");
    }

    #[test]
    fn merge_basic_blocks_preserves_branch_and_protected_targets() {
        let mut graph = FunctionGraph::new(None, 0, 0);
        let v_cond = ValueId(0);
        graph.next_value_id = 1;
        graph.blocks = vec![
            BasicBlock {
                id: BlockId(0),
                operations: vec![(
                    v_cond,
                    GraphOp::Const {
                        index: 0,
                        constant: Constant::Boolean(true),
                    },
                )],
                terminator: Terminator::Branch {
                    condition: v_cond,
                    true_block: BlockId(1),
                    false_block: BlockId(2),
                },
                predecessors: vec![],
                successors: vec![BlockId(1), BlockId(2)],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(1),
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![BlockId(0)],
                successors: vec![],
                exception_handler: None,
            },
            BasicBlock {
                id: BlockId(2),
                operations: vec![],
                terminator: Terminator::Return(None),
                predecessors: vec![BlockId(0)],
                successors: vec![],
                exception_handler: None,
            },
        ];

        // Block 0 ends in Branch, so neither Block 1 nor Block 2 should merge into Block 0.
        let changed = merge_basic_blocks(&mut graph);
        assert!(!changed);
        assert_eq!(graph.blocks.len(), 3);
        crate::verifier::verify_function_graph(&graph).expect("graph verifies");
    }
}
