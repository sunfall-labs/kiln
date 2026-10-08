//! The runtime bootstrap's captured bindings, read from the realm's unit block instead of from
//! the closure (ADR 0070 primitive B; realms issue 08).
//!
//! A closure in the shared immutable region is one object for every realm, so it may carry
//! nothing that belongs to one: no upvalue. kiln compiles a module binding a function reads as a
//! *captured cell*, and most of the bootstrap's methods are made inside an `install*()` function
//! the top level calls once and capture that function's locals; both put the realm's cells into
//! the closure. This pass, run only for a unit emitted with `EmissionPolicy::shared_region`,
//! moves such a cell out of the closure and into the realm's block for the unit
//! (`{symbol}_globals`, reached through the generation-checked cache every module global already
//! uses): the binding keeps its cell -- its dead zone, its writes, every closure sharing one box --
//! and a function reaches the box by the realm it runs in rather than by what it was handed.
//!
//! Which cells may move: a cell made exactly once per realm, because the block has one place for
//! it. That is a cell made
//!
//! - by the module's top-level body (a *once body*), or by a function a once body calls at one
//!   site and never otherwise lets out (the `(function install() { ... })()` shape): its body runs
//!   once per realm too;
//! - on no cycle of that body's blocks -- a `let` in a loop gets a fresh cell per iteration, and
//!   closures made in different iterations must keep different ones;
//! - by the only `NewCell` for its slot in that body, whose slot the body never also uses as a
//!   plain local.
//!
//! A cell the top level makes already lives in the block (a unit with module globals keeps its
//! whole top-level frame there), so it keeps its slot. A cell a called-once function makes is
//! *published*: the emitter copies it into a slot of its own past the top level's, right after
//! making it (`GraphOp::MakeCell`, [`published_slot`]).
//!
//! In the rewritten program a function reads and writes such a binding with `LoadGlobal` /
//! `StoreGlobal` of its block slot, and loses the upvalue; the emitter lowers `LoadGlobal` of a
//! slot in [`RegionPlan::cell_globals`] to a read through the cell, with the dead-zone check an
//! upvalue read has. The region sites are the closure-making operations of the once bodies, on no
//! cycle, of functions that capture nothing after the rewrite.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use kiln_ir::{FunctionProto, Instruction, Program, Upvalue};

use crate::graph::{FunctionGraph, GraphOp, Terminator, ValueId, build_function_graph};

/// What the rewrite decided, for the emitter.
#[derive(Debug, Default, Clone)]
pub(crate) struct RegionPlan {
    /// Block slots that hold a cell: a function's `LoadGlobal`/`StoreGlobal` of one reads or
    /// writes through it.
    pub cell_globals: HashSet<u32>,
    /// `(function, local slot)` -> the block slot a called-once function publishes the cell it
    /// makes in that slot to.
    pub publish: HashMap<(u32, u32), u32>,
    /// The reverse of `publish`, for the analyses that follow a binding to its owner.
    pub published_from: HashMap<u32, (u32, u32)>,
    /// Function index -> its region site, for the functions whose one closure-making operation
    /// is in a once body, on no cycle, capturing nothing.
    pub sites: HashMap<u32, usize>,
    /// Once-made cells stored in compact, realm-owned unit slots instead of GC slab blocks.
    /// The key is the body and its local; the value indexes appended unit slots.
    pub compact_cells: HashMap<(Option<u32>, u32), usize>,
}

impl RegionPlan {
    pub fn site_count(&self) -> usize {
        self.sites.len()
    }
}

thread_local! {
    /// The plan of the unit being emitted, while one emitted with `shared_region` is; a
    /// thread-local for the reason `emit_graph`'s `PARALLEL_CLONE` is.
    static PLAN: RefCell<Option<Rc<RegionPlan>>> = const { RefCell::new(None) };
    /// Module locals are seeded with live cells by graph linking and split instantiation.
    /// Their first `NewCell` must preserve that identity; ordinary lexical cells are newly
    /// allocated each time their declaration executes, including on every loop pass.
    static LINKED_CELLS: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
}

/// Run `f` with `plan` as the plan of the unit it emits, restoring the previous one.
pub(crate) fn with_plan<R>(plan: Option<Rc<RegionPlan>>, f: impl FnOnce() -> R) -> R {
    let before = PLAN.with(|cell| cell.replace(plan));
    let out = f();
    PLAN.with(|cell| *cell.borrow_mut() = before);
    out
}

/// Run module-body emission with the slots whose cells precede module evaluation.
pub(crate) fn with_linked_cells<R>(slots: HashSet<u32>, f: impl FnOnce() -> R) -> R {
    let before = LINKED_CELLS.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), slots));
    let out = f();
    LINKED_CELLS.with(|cell| *cell.borrow_mut() = before);
    out
}

pub(crate) fn is_linked_cell(slot: u32) -> bool {
    LINKED_CELLS.with(|cell| cell.borrow().contains(&slot))
}

fn with_current<R>(f: impl FnOnce(&RegionPlan) -> R) -> Option<R> {
    PLAN.with(|cell| cell.borrow().as_deref().map(f))
}

/// Whether block slot `slot` holds a cell in the unit being emitted.
pub(crate) fn is_cell_global(slot: u32) -> bool {
    with_current(|plan| plan.cell_globals.contains(&slot)).unwrap_or(false)
}

/// The block slot function `function` publishes the cell it makes in `slot` to.
pub(crate) fn published_slot(function: Option<u32>, slot: u32) -> Option<u32> {
    let function = function?;
    with_current(|plan| plan.publish.get(&(function, slot)).copied()).flatten()
}

/// The `(function, local slot)` whose cell block slot `slot` holds, when a function published it.
pub(crate) fn published_origin(slot: u32) -> Option<(u32, u32)> {
    with_current(|plan| plan.published_from.get(&slot).copied()).flatten()
}

/// The region site of function `index`'s closures in the unit being emitted.
pub(crate) fn region_site(index: u32) -> Option<usize> {
    with_current(|plan| plan.sites.get(&index).copied()).flatten()
}

/// How many region sites the unit being emitted has.
pub(crate) fn region_site_count() -> usize {
    with_current(RegionPlan::site_count).unwrap_or(0)
}

pub(crate) fn compact_cell_count() -> usize {
    with_current(|plan| plan.compact_cells.len()).unwrap_or(0)
}

pub(crate) fn compact_cell_slot(owner: Option<u32>, local: u32) -> Option<usize> {
    with_current(|plan| plan.compact_cells.get(&(owner, local)).copied()).flatten()
}

fn entry_proto(program: &Program) -> FunctionProto {
    FunctionProto {
        inferred_name: None,
        asynchronous: false,
        name: None,
        arrow: false,
        generator: false,
        async_generator: false,
        is_method: false,
        is_class: false,
        parameter_count: 0,
        mapped_arguments: false,
        declared_length: 0,
        local_count: program.local_count,
        prologue_end: 0,
        code: program.code.clone(),
        upvalues: Vec::new(),
        span: kiln_ir::Span::default(),
        type_annotations: None,
    }
}

/// The body that makes each function's closures: `None` for the top level. `Err` when some
/// function is made at more than one site (hand-written IR), which this pass does not follow.
fn parents(program: &Program) -> Result<Vec<Option<Option<u32>>>, ()> {
    let mut parents: Vec<Option<Option<u32>>> = vec![None; program.functions.len()];
    let bodies = std::iter::once((None, program.code.as_slice())).chain(
        program
            .functions
            .iter()
            .enumerate()
            .map(|(index, function)| (Some(index as u32), function.code.as_slice())),
    );
    for (owner, code) in bodies {
        for instruction in code {
            if let Instruction::MakeFunction(index) = instruction {
                let slot = parents.get_mut(*index as usize).ok_or(())?;
                if slot.is_some() {
                    return Err(());
                }
                *slot = Some(owner);
            }
        }
    }
    Ok(parents)
}

/// Every use of each value of `graph`, as operations and terminators that name it.
fn uses(graph: &FunctionGraph) -> HashMap<ValueId, Vec<Use<'_>>> {
    let mut uses: HashMap<ValueId, Vec<Use<'_>>> = HashMap::new();
    for block in &graph.blocks {
        for (_, op) in &block.operations {
            for operand in op.operands() {
                uses.entry(operand)
                    .or_default()
                    .push(Use::Op(block.id.0, op));
            }
        }
        let named: Vec<ValueId> = match &block.terminator {
            Terminator::Branch { condition, .. }
            | Terminator::BranchNotNullish { condition, .. } => {
                vec![*condition]
            }
            Terminator::Return(Some(value)) | Terminator::Throw(value) => vec![*value],
            Terminator::Suspend {
                value,
                spilled,
                delegate,
                delegate_wrapped,
                ..
            } => {
                let mut named = vec![*value];
                named.extend(spilled.iter().copied());
                if let Some((delegated, _, _)) = delegate {
                    named.push(*delegated);
                }
                named.extend(delegate_wrapped.iter().copied());
                named
            }
            Terminator::Jump(_) | Terminator::Return(None) | Terminator::Unreachable => Vec::new(),
        };
        for value in named {
            uses.entry(value).or_default().push(Use::Terminator);
        }
    }
    uses
}

enum Use<'a> {
    Op(u32, &'a GraphOp),
    Terminator,
}

/// One body the analysis reads: its graph, the blocks on a cycle, and what its slots are used for.
struct Body {
    graph: FunctionGraph,
    on_cycle: HashSet<u32>,
    /// Slot -> how many `MakeCell`s the body has for it, and whether one is on a cycle.
    made_cells: HashMap<u32, (usize, bool)>,
    /// Slots the body reads or writes as plain locals.
    plain: HashSet<u32>,
}

impl Body {
    fn new(graph: FunctionGraph, code: &[Instruction]) -> Self {
        let on_cycle = crate::emit_graph::blocks_on_a_cycle(&graph);
        let mut made_cells: HashMap<u32, (usize, bool)> = HashMap::new();
        let mut plain = HashSet::new();
        for instruction in code {
            if let Instruction::LoadLocal(slot) | Instruction::StoreLocal(slot) = instruction {
                plain.insert(*slot);
            }
        }
        for block in &graph.blocks {
            let cyclic = on_cycle.contains(&block.id.0);
            for (_, op) in &block.operations {
                if let GraphOp::MakeCell(slot) = op {
                    let entry = made_cells.entry(*slot).or_insert((0, false));
                    entry.0 += 1;
                    entry.1 |= cyclic;
                }
            }
        }
        Self {
            graph,
            on_cycle,
            made_cells,
            plain,
        }
    }

    /// Whether `slot`'s cell is made once per activation of this body, and only ever holds it.
    fn cell_made_once(&self, slot: u32) -> bool {
        matches!(self.made_cells.get(&slot), Some((1, false))) && !self.plain.contains(&slot)
    }

    /// The closure-making operations on no cycle of this body, reachable from its entry.
    fn acyclic_makes(&self) -> Vec<(ValueId, u32)> {
        let mut blocks: Vec<&crate::graph::BasicBlock> = self.graph.blocks.iter().collect();
        blocks.sort_by_key(|block| block.id.0);
        let mut makes = Vec::new();
        for block in blocks {
            if self.on_cycle.contains(&block.id.0)
                || !block.is_reachable_from_entry(self.graph.entry_block)
            {
                continue;
            }
            for (vid, op) in &block.operations {
                if let GraphOp::MakeFunction { index, .. } = op {
                    makes.push((*vid, *index));
                }
            }
        }
        makes
    }
}

/// Rewrite `program` for the shared region, or `None` when it has nothing to gain or a shape this
/// pass does not follow (a direct `eval`, which hands cells to source compiled later; a function
/// made at two sites).
pub(crate) fn plan(program: &Program) -> Option<(Program, RegionPlan)> {
    let every_code = || {
        std::iter::once(program.code.as_slice()).chain(
            program
                .functions
                .iter()
                .map(|function| function.code.as_slice()),
        )
    };
    if every_code().any(|code| {
        code.iter()
            .any(|instruction| matches!(instruction, Instruction::DirectEval(_)))
    }) {
        return None;
    }
    let parents = parents(program).ok()?;
    let children_capture = |owner: Option<u32>, slot: u32| {
        program
            .functions
            .iter()
            .enumerate()
            .any(|(index, function)| {
                parents[index] == Some(owner)
                    && function.upvalues.iter().any(|upvalue| {
                        matches!(
                            upvalue,
                            Upvalue::ParentLocal(s) | Upvalue::NamedParentLocal { slot: s, .. }
                                if *s == slot
                        )
                    })
            })
    };
    // Slots a function reads as module globals already (hand-written IR); the top level's
    // "dead store" rule below must not call such a slot unread.
    let mut read_globally: HashSet<u32> = HashSet::new();
    for function in &program.functions {
        for instruction in &function.code {
            if let Instruction::LoadGlobal(slot) = instruction {
                read_globally.insert(*slot);
            }
        }
    }

    let body_code = |owner: Option<u32>| match owner {
        None => program.code.as_slice(),
        Some(index) => program.functions[index as usize].code.as_slice(),
    };
    // The once bodies, from the top level down.
    let entry = Body::new(
        build_function_graph(&entry_proto(program), program).ok()?,
        &program.code,
    );
    let mut bodies: HashMap<Option<u32>, Body> = HashMap::new();
    bodies.insert(None, entry);
    let mut order: Vec<Option<u32>> = vec![None];
    let mut candidates: Vec<(Option<u32>, u32)> = Vec::new();
    let mut at = 0;
    while at < order.len() {
        let owner = order[at];
        at += 1;
        let body = &bodies[&owner];
        let uses = uses(&body.graph);
        let mut called_once: Vec<u32> = Vec::new();
        for (value, index) in body.acyclic_makes() {
            candidates.push((owner, index));
            let Some(value_uses) = uses.get(&value) else {
                continue;
            };
            let mut calls = 0usize;
            let mut escapes = false;
            for used in value_uses {
                match used {
                    Use::Op(block, GraphOp::Call { callee, args, .. })
                        if *callee == value && !args.contains(&value) =>
                    {
                        if body.on_cycle.contains(block) {
                            escapes = true;
                        }
                        calls += 1;
                    }
                    // `(function f() { ... })()` keeps the closure in `f`'s own binding too; a
                    // binding nothing reads is not a second way to call it.
                    Use::Op(_, GraphOp::StoreLocal { slot, .. })
                        if !body_code(owner).iter().any(|instruction| {
                            matches!(instruction, Instruction::LoadLocal(s) | Instruction::LoadCell(s)
                                | Instruction::StoreCell(s) | Instruction::NewCell(s)
                                if s == slot)
                        }) && !children_capture(owner, *slot)
                            && !(owner.is_none() && read_globally.contains(slot)) => {}
                    _ => escapes = true,
                }
            }
            if calls == 1 && !escapes {
                called_once.push(index);
            }
        }
        for index in called_once {
            let Some(function) = program.functions.get(index as usize) else {
                continue;
            };
            let Ok(graph) = build_function_graph(function, program) else {
                continue;
            };
            if bodies.contains_key(&Some(index)) {
                continue;
            }
            bodies.insert(Some(index), Body::new(graph, &function.code));
            order.push(Some(index));
        }
    }

    // The cells that move: a once body's cell made once, that some function captures.
    let mut block_slot: HashMap<(Option<u32>, u32), u32> = HashMap::new();
    let mut next_slot = program.local_count;
    let mut plan = RegionPlan::default();
    for owner in &order {
        let body = &bodies[owner];
        let mut slots: Vec<u32> = body.made_cells.keys().copied().collect();
        slots.sort_unstable();
        for slot in slots {
            if !body.cell_made_once(slot) || !children_capture(*owner, slot) {
                continue;
            }
            let global = match owner {
                None => slot,
                Some(function) => {
                    let global = next_slot;
                    next_slot += 1;
                    plan.publish.insert((*function, slot), global);
                    plan.published_from.insert(global, (*function, slot));
                    global
                }
            };
            block_slot.insert((*owner, slot), global);
        }
    }
    // These cells are made once for this realm, and every closure capture is rewritten below
    // to the realm's unit block. Keep their payloads in that same stable block. Assign storage
    // deterministically so the emitted descriptor and every chunk agree on its size.
    let mut compact: Vec<_> = block_slot.iter().collect();
    compact.sort_by_key(|(_, global)| **global);
    for (index, (key, _)) in compact.into_iter().enumerate() {
        plan.compact_cells.insert(*key, index);
    }
    // Each upvalue, resolved to the block slot its cell moved to.
    let count = program.functions.len();
    let mut moved: Vec<Vec<Option<u32>>> = program
        .functions
        .iter()
        .map(|function| vec![None; function.upvalues.len()])
        .collect();
    // Parents come before children in no particular index order; iterate to a fixpoint.
    loop {
        let mut changed = false;
        for index in 0..count {
            let Some(parent) = parents[index] else {
                continue;
            };
            for (position, upvalue) in program.functions[index].upvalues.iter().enumerate() {
                let resolved = match upvalue {
                    Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } => {
                        block_slot.get(&(parent, *slot)).copied()
                    }
                    Upvalue::ParentUpvalue(up) | Upvalue::NamedParentUpvalue { index: up, .. } => {
                        parent.and_then(|p| {
                            moved
                                .get(p as usize)
                                .and_then(|slots| slots.get(*up as usize))
                                .copied()
                                .flatten()
                        })
                    }
                };
                if resolved.is_some() && moved[index][position] != resolved {
                    moved[index][position] = resolved;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }

    // The new upvalue numbering: what is left, in order.
    let renumber: Vec<Vec<Option<u32>>> = moved
        .iter()
        .map(|slots| {
            let mut next = 0u32;
            slots
                .iter()
                .map(|slot| {
                    slot.is_none().then(|| {
                        next += 1;
                        next - 1
                    })
                })
                .collect()
        })
        .collect();
    let mut rewritten = program.clone();
    rewritten.local_count = next_slot;
    for (index, function) in rewritten.functions.iter_mut().enumerate() {
        let parent = parents[index].flatten();
        let mut upvalues = Vec::with_capacity(function.upvalues.len());
        for (position, upvalue) in function.upvalues.iter().enumerate() {
            if let Some(global) = moved[index][position] {
                plan.cell_globals.insert(global);
                continue;
            }
            upvalues.push(match upvalue {
                Upvalue::ParentUpvalue(up) => {
                    Upvalue::ParentUpvalue(parent_index(&renumber, parent, *up)?)
                }
                Upvalue::NamedParentUpvalue { index: up, name } => Upvalue::NamedParentUpvalue {
                    index: parent_index(&renumber, parent, *up)?,
                    name: *name,
                },
                other => *other,
            });
        }
        function.upvalues = upvalues;
        for instruction in &mut function.code {
            match instruction {
                Instruction::LoadUpvalue(up) => {
                    *instruction = match moved[index].get(*up as usize).copied().flatten() {
                        Some(global) => Instruction::LoadGlobal(global),
                        None => Instruction::LoadUpvalue(renumber[index][*up as usize]?),
                    };
                }
                Instruction::StoreUpvalue(up) => {
                    *instruction = match moved[index].get(*up as usize).copied().flatten() {
                        Some(global) => Instruction::StoreGlobal(global),
                        None => Instruction::StoreUpvalue(renumber[index][*up as usize]?),
                    };
                }
                _ => {}
            }
        }
    }
    // Every region site: capturing nothing now.
    for (_, index) in candidates {
        if rewritten.functions[index as usize].upvalues.is_empty() {
            let next = plan.sites.len();
            plan.sites.entry(index).or_insert(next);
        }
    }
    Some((rewritten, plan))
}

fn parent_index(renumber: &[Vec<Option<u32>>], parent: Option<u32>, up: u32) -> Option<u32> {
    renumber
        .get(parent? as usize)?
        .get(up as usize)
        .copied()
        .flatten()
}
