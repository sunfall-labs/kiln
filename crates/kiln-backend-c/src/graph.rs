//! SSA Value Graph representation and bytecode abstract interpretation.
//!
//! As established in ADR 0056, the Value Graph is a Turboshaft-style linear buffer of SSA operations
//! organized into a Control Flow Graph (CFG) of basic blocks. Abstract interpretation consumes
//! stack bytecode and lowers it into immutable SSA values, eliminating the runtime operand stack
//! and resolving operand dependencies into direct value edges.

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

/// What an operation may do besides produce its value: nothing, or anything. Two-valued on
/// purpose (plan invariant); a finer lattice waits for a benchmark that needs one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    Pure,
    Unknown,
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
    /// The value a handler block starts with: what was thrown into it. Only ever the first
    /// operation of a handler block.
    CaughtValue,
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

    /// Effect classification for this operation.
    pub fn effect(&self) -> Effect {
        match self {
            Self::Const { .. }
            | Self::LoadLocal(_)
            | Self::LoadBuiltin(_)
            | Self::LoadThis
            | Self::LoadNewTarget
            | Self::Phi { .. } => Effect::Pure,
            Self::Unary {
                op: UnaryOp::Not | UnaryOp::BitwiseNot | UnaryOp::Void,
                ..
            } => Effect::Pure,
            Self::Binary {
                op:
                    BinaryOp::StrictEqual
                    | BinaryOp::StrictNotEqual
                    | BinaryOp::BitwiseAnd
                    | BinaryOp::BitwiseOr
                    | BinaryOp::BitwiseXor,
                ..
            } => Effect::Pure,
            _ => Effect::Unknown,
        }
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
        }
    }

    pub fn is_non_escaping(&self, vid: ValueId) -> bool {
        self.non_escaping_objects.contains(&vid)
    }

    pub fn mark_non_escaping_objects(&mut self, program: &Program) {
        let res = escape_analysis(self, program);
        self.non_escaping_objects = res.non_escaping;
        self.scalar_objects = res.scalar_objects;
        self.scalar_alias_map = res.alias_map;
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
pub fn find_induction_variables(graph: &FunctionGraph) -> HashMap<ValueId, InductionVariable> {
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
            let mut valid = false;
            let mut min = init_val;
            let mut max = init_val;
            let mut non_neg = init_val >= 0;

            if let Some(b_val) = bound_val {
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
    let ivs = find_induction_variables(graph);
    // A resume target's stack phis also take what a suspend spilled and what the caller sent,
    // neither of which is an input here, so they prove nothing.
    let resume_targets: std::collections::HashSet<BlockId> = graph
        .blocks
        .iter()
        .filter_map(|b| match b.terminator {
            Terminator::Suspend { resume, .. } => Some(resume),
            _ => None,
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
                    GraphOp::NewArray { .. } => Some(Proven::Object),
                    GraphOp::MakeFunction { .. } => Some(Proven::Callable),
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
        | Instruction::SetPrivate(_) => (3, 1),
        Instruction::SuperApply
        | Instruction::SpreadObject
        | Instruction::SpreadInto
        | Instruction::SetPrototype
        | Instruction::DefinePrivate { .. }
        | Instruction::GetPrivate(_)
        | Instruction::LoadSuperElement
        | Instruction::DeleteComputed
        | Instruction::DeleteComputedStrict { .. }
        | Instruction::ConstructSpread => (2, 1),
        Instruction::EnumerateKeys
        | Instruction::LoadPrototype
        | Instruction::SuperForward
        | Instruction::IterableToArray
        | Instruction::LoadSuperProperty(_)
        | Instruction::DeleteProperty(_)
        | Instruction::DeletePropertyStrict { .. } => (1, 1),
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
    let mut graph =
        FunctionGraph::new(proto.name.clone(), proto.parameter_count, proto.local_count);

    let code = &proto.code;
    if code.is_empty() {
        let block_id = BlockId(0);
        let ret_val = graph.alloc_value();
        let ops = vec![(
            ret_val,
            GraphOp::Const {
                index: 0,
                constant: Constant::Undefined,
            },
        )];
        graph.blocks.push(BasicBlock {
            id: block_id,
            operations: ops,
            terminator: Terminator::Return(Some(ret_val)),
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
            | Instruction::JumpIfNotNullish(target) => {
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
            _ => {}
        }
    }

    let suspends = proto.generator || proto.asynchronous;
    let binds_at_call =
        suspends && proto.prologue_end > 0 && (proto.prologue_end as usize) < code.len();
    if binds_at_call {
        block_starts.insert(proto.prologue_end as usize);
    }
    let block_starts_vec: Vec<usize> = block_starts.into_iter().collect();
    let pc_to_block: BTreeMap<usize, BlockId> = block_starts_vec
        .iter()
        .enumerate()
        .map(|(idx, &pc)| (pc, BlockId(idx as u32)))
        .collect();

    // Map each PC to its containing BlockId
    let pc_to_containing_block = |pc: usize| -> BlockId {
        let mut prev_block = BlockId(0);
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
    let num_blocks = block_starts_vec.len();
    let mut block_ranges = Vec::with_capacity(num_blocks);
    for (i, &start) in block_starts_vec.iter().enumerate() {
        let end = if i + 1 < num_blocks {
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
    for (b_idx, &(start, end)) in block_ranges.iter().enumerate() {
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
            locals: initial_locals,
        },
    );

    // Phase 2b: static predecessor counts, from each block's last instruction. A block with two
    // or more predecessors merges state, and a merge is a phi per local: without one, the first
    // frame to arrive was kept and every later predecessor was ignored, so a loop header compared
    // the pre-loop value forever and a local assigned in one branch of an `if` read its
    // pre-branch value after the join. Stack merges (`c ? a : b`) get their phis when the first
    // frame arrives, because only then is the depth known.
    let mut static_preds: Vec<Vec<BlockId>> = vec![Vec::new(); num_blocks];
    for (b_idx, &(start, end)) in block_ranges.iter().enumerate() {
        let this = BlockId(b_idx as u32);
        let next = (b_idx + 1 < num_blocks).then(|| BlockId((b_idx + 1) as u32));
        let mut succs: Vec<BlockId> = Vec::new();
        let last = if end > start { code.get(end - 1) } else { None };
        match last {
            Some(Instruction::Jump(target)) => succs.push(pc_to_containing_block(*target as usize)),
            Some(
                Instruction::JumpIfFalse(target)
                | Instruction::JumpIfTruthy(target)
                | Instruction::JumpIfNotNullish(target),
            ) => {
                succs.push(pc_to_containing_block(*target as usize));
                succs.extend(next);
            }
            // A suspend is not an edge: the body returns, and a later call re-enters at the
            // resume target through the dispatch switch.
            Some(
                Instruction::Return | Instruction::Throw | Instruction::Yield | Instruction::Await,
            ) => {}
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
                | Instruction::JumpIfNotNullish(target) => {
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
        let stack: Option<Vec<ValueId>> = resume_depth.map(|depth| {
            let phis: Vec<ValueId> = (0..depth).map(|_| graph.alloc_value()).collect();
            for &phi in &phis {
                merges.inputs.insert(phi, Vec::new());
            }
            phis
        });
        block_entry_frames.insert(
            block_id,
            AbstractFrame {
                stack: stack.clone().unwrap_or_default(),
                locals: frame_locals,
            },
        );
        merges.phis.insert(block_id, PhiSet { locals, stack });
    }

    let mut handler_caught: HashMap<BlockId, ValueId> = HashMap::new();
    if binds_at_call {
        // Reached by falling out of the binding, which stops there, and by the first `next`,
        // which starts there: both with an empty operand stack and the locals in the frame.
        graph.seam_block = Some(pc_to_containing_block(proto.prologue_end as usize));
    }
    for (b_idx, &(start, end)) in block_ranges.iter().enumerate() {
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
            for &phi in set.locals.iter().chain(set.stack.iter().flatten()) {
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
                    terminator = Terminator::Suspend {
                        value,
                        resume: resume_block,
                        spilled,
                        pc: pc as u32,
                        awaiting: matches!(inst, Instruction::Await),
                    };
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
    graph.mark_non_escaping_objects(program);
    Ok(graph)
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
                        if let Upvalue::ParentLocal(slot) = upvalue {
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
    while let Some(op) = defs.get(&vid) {
        match op {
            GraphOp::StoreLocal { value, .. } | GraphOp::CheckTdz { value, .. } => {
                vid = *value;
            }
            _ => break,
        }
    }
    vid
}

pub fn resolve_const_string<'a>(
    vid: ValueId,
    defs: &HashMap<ValueId, &'a GraphOp>,
    _program: &'a Program,
) -> Option<&'a str> {
    let root = resolve_root_alias(vid, defs);
    if let Some(GraphOp::Const {
        constant: Constant::String(s),
        ..
    }) = defs.get(&root)
    {
        return Some(s.as_str());
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
        return EscapeAnalysisResult::default();
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
                GraphOp::Phi { inputs } if !inputs.is_empty() => {
                    let non_self: Vec<ValueId> = inputs
                        .iter()
                        .map(|(_, v)| *v)
                        .filter(|v| v != vid)
                        .collect();
                    if let Some(&first_root) = non_self.first().and_then(|v| alias_to_root.get(v))
                        && non_self
                            .iter()
                            .all(|v| alias_to_root.get(v) == Some(&first_root))
                    {
                        alias_to_root.insert(*vid, first_root);
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
            | Terminator::Branch { condition: v, .. } => {
                live_values.insert(*v);
            }
            Terminator::Suspend { value, spilled, .. } => {
                live_values.insert(*value);
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
                    | GraphOp::Unary { value, .. } => {
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
    let slots_in_array = array_slots(graph, program);

    for block in &graph.blocks {
        match &block.terminator {
            Terminator::Return(Some(v))
            | Terminator::Throw(v)
            | Terminator::Branch { condition: v, .. } => {
                if let Some(&root) = alias_to_root.get(v) {
                    escaped.insert(root);
                }
            }
            Terminator::Suspend { value, spilled, .. } => {
                if let Some(&root) = alias_to_root.get(value) {
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
                    for o in operands {
                        if let Some(&root) = alias_to_root.get(o) {
                            escaped.insert(root);
                        }
                    }
                }
                GraphOp::Unary { value, .. } => {
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
                    if live_values.contains(vid) {
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
                            Some(Constant::String(s)) => s.as_str(),
                            _ => "",
                        };
                        if OBJECT_PROTOTYPE_PROPERTIES.contains(&prop_name) {
                            let defined =
                                if let Some(GraphOp::NewObject { pairs, .. }) = defs.get(&root) {
                                    pairs.iter().any(|(k, _)| {
                                        resolve_const_string(*k, &defs, program) == Some(prop_name)
                                    })
                                } else {
                                    false
                                };
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
                            Some(Constant::String(s)) => s.as_str(),
                            _ => "",
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
                            if OBJECT_PROTOTYPE_PROPERTIES.contains(&prop_name) {
                                let defined = if let Some(GraphOp::NewObject { pairs, .. }) =
                                    defs.get(&root)
                                {
                                    pairs.iter().any(|(k, _)| {
                                        resolve_const_string(*k, &defs, program) == Some(prop_name)
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

        // 2. From SetProperty / GetProperty / SetComputed / GetComputed
        for block in &graph.blocks {
            for (_, op) in &block.operations {
                match op {
                    GraphOp::GetProperty { object, name, .. }
                    | GraphOp::SetProperty { object, name, .. }
                    | GraphOp::SetPropertyStrict { object, name, .. }
                        if alias_to_root.get(object) == Some(&alloc_id) =>
                    {
                        if let Some(Constant::String(s)) = program.constants.get(*name as usize) {
                            field_names.insert(s.clone());
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
                && let Some(&field_idx) = fields.get(s)
            {
                field_by_name_index.insert(c_idx as u32, field_idx);
            }
        }

        let mut initial_values = HashMap::new();
        let mut field_written_values: HashMap<usize, Vec<ValueId>> = HashMap::new();

        if let Some(GraphOp::NewObject { pairs, .. }) = defs.get(&alloc_id) {
            for (k, v) in pairs {
                if let Some(prop) = resolve_const_string(*k, &defs, program)
                    && let Some(&field_idx) = fields.get(prop)
                {
                    initial_values.insert(field_idx, *v);
                    field_written_values.entry(field_idx).or_default().push(*v);
                }
            }
        }

        for block in &graph.blocks {
            for (_, op) in &block.operations {
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
                            && let Some(&field_idx) = fields.get(prop)
                        {
                            field_written_values
                                .entry(field_idx)
                                .or_default()
                                .push(*value);
                        }
                    }
                    _ => {}
                }
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

    EscapeAnalysisResult {
        non_escaping,
        scalar_objects,
        alias_map,
    }
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
