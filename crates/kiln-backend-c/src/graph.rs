//! SSA Value Graph representation and bytecode abstract interpretation.
//!
//! As established in ADR 0056, the Value Graph is a Turboshaft-style linear buffer of SSA operations
//! organized into a Control Flow Graph (CFG) of basic blocks. Abstract interpretation consumes
//! stack bytecode and lowers it into immutable SSA values, eliminating the runtime operand stack
//! and resolving operand dependencies into direct value edges.

use kiln_ir::{BinaryOp, Constant, FunctionProto, HostOp, Instruction, Program, UnaryOp};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Strongly typed identifier for an SSA value in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub u32);

/// Strongly typed identifier for a basic block in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u32);

/// Deterministic, stable identifier for a call site, property access, or allocation site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SiteId(pub u32);

/// Lattice types tracked in the value graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum GraphType {
    #[default]
    Val,
    Number,
    Int,
    Boolean,
    String,
    Object,
    Callable,
    Undefined,
    Null,
}

/// Operation effect category (two-valued invariant per ADR 0056).
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
    MakeCell(u32),
    LoadGlobal(u32),
    StoreGlobal {
        slot: u32,
        value: ValueId,
    },
    LoadThis,
    LoadNewTarget,
    LoadArguments,
    RestArguments(u32),
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
    ToPropertyKey {
        object: ValueId,
        key: ValueId,
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
    CallComputed {
        object: ValueId,
        key: ValueId,
        args: Vec<ValueId>,
        site: SiteId,
    },
    HostCall {
        op: HostOp,
        args: Vec<ValueId>,
        site: SiteId,
    },
    DirectEval {
        site_idx: u32,
        args: Vec<ValueId>,
    },
    NewArray {
        elements: Vec<ValueId>,
        site: SiteId,
    },
    IterableToArray(ValueId),
    SpreadInto {
        target: ValueId,
        iterable: ValueId,
    },
    SetPrototype {
        object: ValueId,
        prototype: ValueId,
    },

    // Super and Private Members
    SuperGet {
        prototype: ValueId,
        name: u32,
    },
    SuperSet {
        prototype: ValueId,
        name: u32,
        value: ValueId,
    },
    SuperCall {
        constructor: ValueId,
        args: Vec<ValueId>,
    },
    PrivateGet {
        object: ValueId,
        slot: u32,
    },
    PrivateSet {
        object: ValueId,
        slot: u32,
        value: ValueId,
    },
    PrivateMethod {
        object: ValueId,
        slot: u32,
    },
    PrivateGetter {
        object: ValueId,
        slot: u32,
    },
    PrivateSetter {
        object: ValueId,
        slot: u32,
    },
    InPrivate {
        object: ValueId,
        slot: u32,
    },

    // Modules
    ImportDynamic(ValueId),
    ImportMeta,

    // Speculation and Invariants
    Guard {
        condition: ValueId,
        invariant_row: u32,
        deopt_target: BlockId,
    },
    UnboxNumber(ValueId),
    BoxNumber(ValueId),
    UnboxInt(ValueId),
    BoxInt(ValueId),

    // SSA Phi
    Phi {
        inputs: Vec<(BlockId, ValueId)>,
    },
}

impl GraphOp {
    /// Effect classification for this operation.
    pub fn effect(&self) -> Effect {
        match self {
            Self::Const { .. }
            | Self::LoadLocal(_)
            | Self::LoadThis
            | Self::LoadNewTarget
            | Self::UnboxNumber(_)
            | Self::BoxNumber(_)
            | Self::UnboxInt(_)
            | Self::BoxInt(_)
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
    Yield {
        value: ValueId,
        resume: BlockId,
    },
    Await {
        promise: ValueId,
        resume: BlockId,
    },
    Unreachable,
}

/// A basic block in the Control Flow Graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicBlock {
    pub id: BlockId,
    pub operations: Vec<(ValueId, GraphOp, GraphType)>,
    pub terminator: Terminator,
    pub predecessors: Vec<BlockId>,
    pub successors: Vec<BlockId>,
    pub exception_handler: Option<BlockId>,
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
        }
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

/// Abstract-interpret stack bytecode into a verified SSA Value Graph.
pub fn build_function_graph(proto: &FunctionProto, program: &Program) -> FunctionGraph {
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
            GraphType::Undefined,
        )];
        graph.blocks.push(BasicBlock {
            id: block_id,
            operations: ops,
            terminator: Terminator::Return(Some(ret_val)),
            predecessors: Vec::new(),
            successors: Vec::new(),
            exception_handler: None,
        });
        return graph;
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
                block_starts.insert(*target as usize);
            }
            Instruction::Yield | Instruction::Await if pc + 1 < code.len() => {
                block_starts.insert(pc + 1);
            }
            _ => {}
        }
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
    }
    block_entry_frames.insert(
        BlockId(0),
        AbstractFrame {
            stack: Vec::new(),
            locals: initial_locals,
        },
    );

    for (b_idx, &(start, end)) in block_ranges.iter().enumerate() {
        let block_id = BlockId(b_idx as u32);
        let mut frame = block_entry_frames
            .get(&block_id)
            .cloned()
            .unwrap_or_else(|| AbstractFrame {
                stack: Vec::new(),
                locals: HashMap::new(),
            });

        let mut operations = Vec::new();
        let mut terminator = Terminator::Unreachable;
        let mut successors = Vec::new();

        for (offset, inst) in code[start..end].iter().enumerate() {
            let pc = start + offset;
            match inst {
                Instruction::Const(c_idx) => {
                    let val = graph.alloc_value();
                    let constant = program
                        .constants
                        .get(*c_idx as usize)
                        .cloned()
                        .unwrap_or(Constant::Undefined);
                    let gtype = match &constant {
                        Constant::Number(_) => GraphType::Number,
                        Constant::String(_) => GraphType::String,
                        Constant::Boolean(_) => GraphType::Boolean,
                        Constant::Null => GraphType::Null,
                        Constant::Undefined => GraphType::Undefined,
                        Constant::BigInt(_) => GraphType::Val,
                    };
                    operations.push((
                        val,
                        GraphOp::Const {
                            index: *c_idx,
                            constant,
                        },
                        gtype,
                    ));
                    frame.stack.push(val);
                }
                Instruction::LoadLocal(slot) => {
                    let val = if let Some(&v) = frame.locals.get(slot) {
                        v
                    } else {
                        let v = graph.alloc_value();
                        operations.push((v, GraphOp::LoadLocal(*slot), GraphType::Val));
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
                            GraphType::Val,
                        ));
                    }
                }
                Instruction::Pop => {
                    frame.stack.pop();
                }
                Instruction::Dup => {
                    if let Some(&top) = frame.stack.last() {
                        frame.stack.push(top);
                    }
                }
                Instruction::Dup2 => {
                    let len = frame.stack.len();
                    if len >= 2 {
                        let a = frame.stack[len - 2];
                        let b = frame.stack[len - 1];
                        frame.stack.push(a);
                        frame.stack.push(b);
                    }
                }
                Instruction::Unary(op) => {
                    let operand = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    let gtype = match op {
                        UnaryOp::Not => GraphType::Boolean,
                        UnaryOp::Plus | UnaryOp::Negate => GraphType::Number,
                        _ => GraphType::Val,
                    };
                    operations.push((
                        res,
                        GraphOp::Unary {
                            op: *op,
                            value: operand,
                        },
                        gtype,
                    ));
                    frame.stack.push(res);
                }
                Instruction::Binary(op) => {
                    let right = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let left = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    let gtype = match op {
                        BinaryOp::StrictEqual
                        | BinaryOp::StrictNotEqual
                        | BinaryOp::LooseEqual
                        | BinaryOp::LooseNotEqual
                        | BinaryOp::LessThan
                        | BinaryOp::LessThanOrEqual
                        | BinaryOp::GreaterThan
                        | BinaryOp::GreaterThanOrEqual
                        | BinaryOp::In
                        | BinaryOp::InstanceOf => GraphType::Boolean,
                        BinaryOp::Subtract
                        | BinaryOp::Multiply
                        | BinaryOp::Divide
                        | BinaryOp::Remainder
                        | BinaryOp::Exponentiate => GraphType::Number,
                        _ => GraphType::Val,
                    };
                    operations.push((
                        res,
                        GraphOp::Binary {
                            op: *op,
                            left,
                            right,
                        },
                        gtype,
                    ));
                    frame.stack.push(res);
                }
                Instruction::Jump(target) => {
                    let target_block = *pc_to_block.get(&(*target as usize)).unwrap();
                    terminator = Terminator::Jump(target_block);
                    successors.push(target_block);
                    propagate_frame(&mut block_entry_frames, target_block, &frame);
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
                    propagate_frame(&mut block_entry_frames, fallthrough_block, &frame);
                    propagate_frame(&mut block_entry_frames, target_block, &frame);
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
                    propagate_frame(&mut block_entry_frames, target_block, &frame);
                    propagate_frame(&mut block_entry_frames, fallthrough_block, &frame);
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
                    propagate_frame(&mut block_entry_frames, target_block, &frame);
                    propagate_frame(&mut block_entry_frames, fallthrough_block, &frame);
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
                    operations.push((result, GraphOp::Call { callee, args, site }, GraphType::Val));
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
                        GraphType::Val,
                    ));
                    frame.stack.push(result);
                }
                Instruction::CallComputed { argc } => {
                    let mut args = Vec::with_capacity(*argc as usize);
                    for _ in 0..*argc {
                        args.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    args.reverse();
                    let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::CallComputed {
                            object,
                            key,
                            args,
                            site,
                        },
                        GraphType::Val,
                    ));
                    frame.stack.push(result);
                }
                Instruction::HostCall { op, argc } => {
                    let mut args = Vec::with_capacity(*argc as usize);
                    for _ in 0..*argc {
                        args.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    args.reverse();
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::HostCall {
                            op: *op,
                            args,
                            site,
                        },
                        GraphType::Val,
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
                        GraphType::Val,
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
                        GraphType::Val,
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
                        GraphType::Val,
                    ));
                    frame.stack.push(result);
                }
                Instruction::GetComputed => {
                    let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::GetComputed { object, key, site },
                        GraphType::Val,
                    ));
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
                        GraphType::Val,
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
                        GraphType::Val,
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
                        GraphType::Callable,
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
                    operations.push((
                        result,
                        GraphOp::NewArray { elements, site },
                        GraphType::Object,
                    ));
                    frame.stack.push(result);
                }
                Instruction::IterableToArray => {
                    let iter = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::IterableToArray(iter), GraphType::Object));
                    frame.stack.push(result);
                }
                Instruction::SpreadInto => {
                    let iter = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let target = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::SpreadInto {
                            target,
                            iterable: iter,
                        },
                        GraphType::Object,
                    ));
                    frame.stack.push(result);
                }
                Instruction::ToPropertyKey => {
                    let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = *frame.stack.last().unwrap_or(&key);
                    let result = graph.alloc_value();
                    operations.push((
                        result,
                        GraphOp::ToPropertyKey { object, key },
                        GraphType::Val,
                    ));
                    frame.stack.push(result);
                }
                Instruction::LoadThis => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadThis, GraphType::Val));
                    frame.stack.push(result);
                }
                Instruction::LoadNewTarget => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadNewTarget, GraphType::Val));
                    frame.stack.push(result);
                }
                Instruction::LoadArguments => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadArguments, GraphType::Object));
                    frame.stack.push(result);
                }
                Instruction::RestArguments(offset) => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::RestArguments(*offset), GraphType::Object));
                    frame.stack.push(result);
                }
                Instruction::LoadCell(slot) => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadCell(*slot), GraphType::Val));
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
                            GraphType::Val,
                        ));
                    }
                }
                Instruction::NewCell(slot) => {
                    let res = graph.alloc_value();
                    operations.push((res, GraphOp::MakeCell(*slot), GraphType::Val));
                }
                Instruction::LoadGlobal(slot) => {
                    let result = graph.alloc_value();
                    operations.push((result, GraphOp::LoadGlobal(*slot), GraphType::Val));
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
                            GraphType::Val,
                        ));
                    }
                }
                Instruction::MarkUninitialized(slot) => {
                    let res = graph.alloc_value();
                    operations.push((res, GraphOp::MarkUninitialized(*slot), GraphType::Val));
                }
                Instruction::Yield => {
                    let val = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let resume_block = pc_to_containing_block(pc + 1);
                    terminator = Terminator::Yield {
                        value: val,
                        resume: resume_block,
                    };
                    successors.push(resume_block);
                    propagate_frame(&mut block_entry_frames, resume_block, &frame);
                }
                Instruction::Await => {
                    let promise = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let resume_block = pc_to_containing_block(pc + 1);
                    terminator = Terminator::Await {
                        promise,
                        resume: resume_block,
                    };
                    successors.push(resume_block);
                    propagate_frame(&mut block_entry_frames, resume_block, &frame);
                }
                Instruction::SetPrototype => {
                    let prototype = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::SetPrototype { object, prototype },
                        GraphType::Object,
                    ));
                    frame.stack.push(res);
                }
                Instruction::LoadSuperProperty(name) => {
                    let proto_val = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::SuperGet {
                            prototype: proto_val,
                            name: *name,
                        },
                        GraphType::Val,
                    ));
                    frame.stack.push(res);
                }
                Instruction::LoadSuperElement => {
                    let proto_val = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let key = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let site = graph.alloc_site();
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::GetComputed {
                            object: proto_val,
                            key,
                            site,
                        },
                        GraphType::Val,
                    ));
                    frame.stack.push(res);
                }
                Instruction::SuperCall { argc } => {
                    let mut args = Vec::with_capacity(*argc as usize);
                    for _ in 0..*argc {
                        args.push(frame.stack.pop().unwrap_or_else(|| graph.alloc_value()));
                    }
                    args.reverse();
                    let ctor = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::SuperCall {
                            constructor: ctor,
                            args,
                        },
                        GraphType::Val,
                    ));
                    frame.stack.push(res);
                }
                Instruction::GetPrivate(slot) => {
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::PrivateGet {
                            object,
                            slot: *slot,
                        },
                        GraphType::Val,
                    ));
                    frame.stack.push(res);
                }
                Instruction::SetPrivate(slot) => {
                    let value = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let object = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::PrivateSet {
                            object,
                            slot: *slot,
                            value,
                        },
                        GraphType::Val,
                    ));
                    frame.stack.push(res);
                }
                Instruction::DirectEval(_) => {
                    let mut args = Vec::new();
                    if let Some(arg) = frame.stack.pop() {
                        args.push(arg);
                    }
                    let res = graph.alloc_value();
                    operations.push((
                        res,
                        GraphOp::DirectEval { site_idx: 0, args },
                        GraphType::Val,
                    ));
                    frame.stack.push(res);
                }
                Instruction::DynamicImport => {
                    let specifier = frame.stack.pop().unwrap_or_else(|| graph.alloc_value());
                    let res = graph.alloc_value();
                    operations.push((res, GraphOp::ImportDynamic(specifier), GraphType::Object));
                    frame.stack.push(res);
                }
                _ => {}
            }
        }

        // Fallthrough if block didn't terminate explicitly
        if terminator == Terminator::Unreachable && b_idx + 1 < num_blocks {
            let next_block = BlockId((b_idx + 1) as u32);
            terminator = Terminator::Jump(next_block);
            successors.push(next_block);
            propagate_frame(&mut block_entry_frames, next_block, &frame);
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
    graph
}

fn propagate_frame(
    frames: &mut HashMap<BlockId, AbstractFrame>,
    target: BlockId,
    outgoing: &AbstractFrame,
) {
    frames.entry(target).or_insert_with(|| outgoing.clone());
}
