//! Direct calls (plan step 2.3): a call whose callee is known at build time is a C call by name.
//!
//! Two questions are answered here, for the whole unit at once, before any body is emitted.
//!
//! **Which callee.** A call's callee value is *known* to be the closure of proto `i` when it is
//! the value `MakeFunction(i)` produced, or a read of a binding that only ever holds that
//! closure. The binding rule has to survive how the compiler lowers a hoisted declaration: a
//! cell stored twice, `undefined` first and the closure second, both at the top of the owner's
//! entry block. So a binding is known when exactly one store gives it a `MakeFunction(i)`, that
//! store sits in the owner's entry block, every other store gives it `undefined` and sits
//! earlier in that same block, and everything earlier in that block is inert: nothing there can
//! run code or read the binding, so no executor -- the owner, a closure it made, a caller --
//! can observe it between the `undefined` and the closure. A read that could see the dead zone
//! is checked before it reaches the call and throws; it never reaches the call with anything
//! but the closure.
//!
//! **Which entry.** A proto that is called directly gets a second clone, the *typed entry*,
//! whose contract is that it is called with exactly its parameter count of arguments and every
//! one a number. Inside it the parameters are proven numbers, so everything that follows from
//! them is typed, and a call from it to another typed entry is typed too. Whether such a clone
//! returns a number is a fixpoint over the unit: assume every typed entry does, and drop the
//! ones whose return is not proven under that assumption until nothing changes. A site calls
//! the typed entry only when its own body proves every argument a number; otherwise it calls
//! the boxed entry directly, and the boxed entry keeps every guarantee the generic call has.
//!
//! Nothing here speculates. A typed entry is reached only from sites that proved its contract,
//! and the boxed entry is unchanged for everyone else, so no guard is needed and none is
//! emitted. This is the "two clones, never more" invariant with the second clone reached by
//! proof rather than by guess.

use std::collections::{HashMap, HashSet};

use kiln_ir::{Constant, FunctionProto, Program, Upvalue};

use crate::graph::{
    BlockId, FunctionGraph, GraphOp, Proven, Terminator, ValueId, proven_types_with,
};
use crate::proto_publishes_frame;

/// Which body a graph belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Owner {
    Entry,
    Function(u32),
}

/// A binding: a slot of an owner, read through a cell, an upvalue, or the unit's globals.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Binding {
    owner: Owner,
    slot: u32,
}

/// How one body's calls are emitted.
#[derive(Default, Clone, Debug)]
pub struct CallPlan {
    /// Call results whose callee is the closure of the named proto.
    pub known: HashMap<ValueId, u32>,
    /// Call results emitted against the callee's typed entry: every argument proven a number.
    pub typed_sites: HashSet<ValueId>,
    /// Every value this body proves a number, with the plan's call verdicts folded in.
    pub numbers: HashSet<ValueId>,
    /// Every value this body proves an integer, with the plan's call verdicts folded in.
    pub ints: HashSet<ValueId>,
    /// Reads of a known binding that is never marked uninitialized: the binding holds
    /// `undefined` and then the closure, so the read needs no dead-zone check.
    pub unchecked: HashSet<ValueId>,
    /// Per proto: whether its typed entry is proven to return a number, and so hands its
    /// result back as a `double`.
    pub typed_returns: Vec<bool>,
}

/// The whole unit's plan: a boxed plan per function, a typed one where the typed entry exists,
/// and the entry point's.
#[derive(Debug)]
pub struct DirectPlan {
    pub functions: Vec<CallPlan>,
    pub typed: Vec<Option<CallPlan>>,
    pub entry: CallPlan,
}

impl DirectPlan {
    pub fn plan(program: &Program, graphs: &[FunctionGraph], entry: &FunctionGraph) -> Self {
        let known = known_callees(program, graphs, entry);
        let direct: Vec<bool> = program.functions.iter().map(direct_callable).collect();
        let typed_eligible: Vec<bool> = program
            .functions
            .iter()
            .zip(&direct)
            .map(|(proto, &direct)| direct && proto.parameter_count > 0)
            .collect();
        let params: Vec<u32> = program
            .functions
            .iter()
            .map(|p| p.parameter_count)
            .collect();

        // The sites of each body: (result, callee proto, arguments).
        let sites_of = |graph: &FunctionGraph, known: &KnownValues| -> Vec<Site> {
            let mut sites = Vec::new();
            for block in &graph.blocks {
                for (vid, op) in &block.operations {
                    if let GraphOp::Call { callee, args, .. } = op
                        && let Some(&target) = known.known.get(callee)
                        && direct[target as usize]
                    {
                        sites.push(Site {
                            result: *vid,
                            target,
                            args: args.clone(),
                        });
                    }
                }
            }
            sites
        };
        let function_sites: Vec<Vec<Site>> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| sites_of(g, &known.functions[i]))
            .collect();
        let entry_sites = sites_of(entry, &known.entry);

        // Greatest fixpoint: every typed entry returns a number until proven otherwise.
        let mut typed_returns: Vec<bool> = typed_eligible.clone();
        loop {
            let mut changed = false;
            for (i, graph) in graphs.iter().enumerate() {
                if !typed_eligible[i] {
                    continue;
                }
                let seeds: HashMap<ValueId, Proven> = graph
                    .initial_locals
                    .iter()
                    .take(params[i] as usize)
                    .map(|&v| (v, Proven::Number))
                    .collect();
                let (proven, _) = settle(
                    graph,
                    seeds,
                    &function_sites[i],
                    &typed_eligible,
                    &typed_returns,
                    &params,
                );
                // Every return a number. A block nothing reaches -- the `return undefined` the
                // compiler leaves after a body's last statement -- returns nothing.
                let returns_number = graph
                    .blocks
                    .iter()
                    .filter(|b| b.id == graph.entry_block || !b.predecessors.is_empty())
                    .all(|b| match &b.terminator {
                        Terminator::Return(Some(v)) => proven.get(v).is_some_and(|k| k.is_number()),
                        Terminator::Return(None) => false,
                        _ => true,
                    });
                if typed_returns[i] && !returns_number {
                    typed_returns[i] = false;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let plan_for = |graph: &FunctionGraph,
                        known: &KnownValues,
                        sites: &[Site],
                        seeds: HashMap<ValueId, Proven>|
         -> CallPlan {
            let (proven, typed_sites) = settle(
                graph,
                seeds,
                sites,
                &typed_eligible,
                &typed_returns,
                &params,
            );
            CallPlan {
                known: known.known.clone(),
                typed_sites,
                numbers: proven
                    .iter()
                    .filter_map(|(&v, &kind)| kind.is_number().then_some(v))
                    .collect(),
                ints: proven
                    .into_iter()
                    .filter_map(|(v, kind)| (kind == Proven::Int).then_some(v))
                    .collect(),
                unchecked: known.unchecked.clone(),
                typed_returns: typed_returns.clone(),
            }
        };
        let functions: Vec<CallPlan> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| plan_for(g, &known.functions[i], &function_sites[i], HashMap::new()))
            .collect();
        let typed: Vec<Option<CallPlan>> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                if !typed_eligible[i] {
                    return None;
                }
                let seeds: HashMap<ValueId, Proven> = g
                    .initial_locals
                    .iter()
                    .take(params[i] as usize)
                    .map(|&v| (v, Proven::Number))
                    .collect();
                Some(plan_for(g, &known.functions[i], &function_sites[i], seeds))
            })
            .collect();
        let entry_plan = plan_for(entry, &known.entry, &entry_sites, HashMap::new());

        // A typed entry exists only where some site calls it; an unreferenced clone is dead C.
        let mut referenced: HashSet<u32> = HashSet::new();
        for (plan, sites) in functions.iter().zip(&function_sites) {
            referenced.extend(
                sites
                    .iter()
                    .filter(|s| plan.typed_sites.contains(&s.result))
                    .map(|s| s.target),
            );
        }
        for (plan, sites) in typed.iter().zip(&function_sites) {
            if let Some(plan) = plan {
                referenced.extend(
                    sites
                        .iter()
                        .filter(|s| plan.typed_sites.contains(&s.result))
                        .map(|s| s.target),
                );
            }
        }
        referenced.extend(
            entry_sites
                .iter()
                .filter(|s| entry_plan.typed_sites.contains(&s.result))
                .map(|s| s.target),
        );
        // A typed entry referenced only from typed entries that are themselves unreferenced is
        // still dead, but only after those are dropped: close the set through the typed sites.
        loop {
            let before = referenced.len();
            let mut next = referenced.clone();
            for (i, plan) in typed.iter().enumerate() {
                if let Some(plan) = plan
                    && referenced.contains(&(i as u32))
                {
                    next.extend(
                        function_sites[i]
                            .iter()
                            .filter(|s| plan.typed_sites.contains(&s.result))
                            .map(|s| s.target),
                    );
                }
            }
            referenced = next;
            if referenced.len() == before {
                break;
            }
        }
        let typed = typed
            .into_iter()
            .enumerate()
            .map(|(i, plan)| plan.filter(|_| referenced.contains(&(i as u32))))
            .collect();

        DirectPlan {
            functions,
            typed,
            entry: entry_plan,
        }
    }

    /// The typed entry of `index` exists in the unit.
    pub fn has_typed_entry(&self, index: u32) -> bool {
        self.typed.get(index as usize).is_some_and(Option::is_some)
    }
}

struct Site {
    result: ValueId,
    target: u32,
    args: Vec<ValueId>,
}

/// A proto whose call needs nothing but the body: no frame published, no suspension.
pub(crate) fn direct_callable(proto: &FunctionProto) -> bool {
    !proto_publishes_frame(proto) && !proto.generator && !proto.asynchronous
}

/// The proven types of one body under `seeds`, with the typed sites settled: a site is typed
/// when every argument is proven a number, and its result is a number when the callee's typed
/// entry returns one. Assuming both optimistically and dropping what fails converges, because
/// dropping a site's verdict can only make fewer values numbers.
fn settle(
    graph: &FunctionGraph,
    seeds: HashMap<ValueId, Proven>,
    sites: &[Site],
    typed_eligible: &[bool],
    typed_returns: &[bool],
    params: &[u32],
) -> (HashMap<ValueId, Proven>, HashSet<ValueId>) {
    let mut typed_sites: HashSet<ValueId> = sites
        .iter()
        .filter(|s| {
            typed_eligible[s.target as usize] && s.args.len() == params[s.target as usize] as usize
        })
        .map(|s| s.result)
        .collect();
    loop {
        let mut assumptions = seeds.clone();
        for site in sites {
            if typed_sites.contains(&site.result) && typed_returns[site.target as usize] {
                assumptions.insert(site.result, Proven::Number);
            }
        }
        let proven = proven_types_with(graph, &assumptions);
        let before = typed_sites.len();
        typed_sites.retain(|result| {
            let site = sites
                .iter()
                .find(|s| s.result == *result)
                .expect("a settled site");
            site.args
                .iter()
                .all(|a| proven.get(a).is_some_and(|k| k.is_number()))
        });
        if typed_sites.len() == before {
            return (proven, typed_sites);
        }
    }
}

/// One body's known callee values, and its reads that need no dead-zone check.
#[derive(Default, Clone)]
struct KnownValues {
    known: HashMap<ValueId, u32>,
    unchecked: HashSet<ValueId>,
}

struct Known {
    functions: Vec<KnownValues>,
    entry: KnownValues,
}

/// What a stored value is, for the binding rule.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stored {
    Closure(u32),
    Undefined,
    Other,
}

/// One store to a binding: which body, which block, at what position, storing what.
struct Store {
    owner: Owner,
    block: BlockId,
    position: usize,
    stored: Stored,
}

fn known_callees(program: &Program, graphs: &[FunctionGraph], entry: &FunctionGraph) -> Known {
    let graph_of = |owner: Owner| -> &FunctionGraph {
        match owner {
            Owner::Entry => entry,
            Owner::Function(i) => &graphs[i as usize],
        }
    };
    let owners: Vec<Owner> = std::iter::once(Owner::Entry)
        .chain((0..graphs.len()).map(|i| Owner::Function(i as u32)))
        .collect();

    // Who makes each closure: the owner of the body holding its `MakeFunction`.
    let mut parent: Vec<Option<Owner>> = vec![None; graphs.len()];
    let mut made: Vec<u32> = vec![0; graphs.len()];
    for &owner in &owners {
        for block in &graph_of(owner).blocks {
            for (_, op) in &block.operations {
                if let GraphOp::MakeFunction { index, .. } = op {
                    parent[*index as usize] = Some(owner);
                    made[*index as usize] += 1;
                }
            }
        }
    }
    // A proto made at two sites has two closures that "the closure of proto `i`" cannot tell
    // apart; nothing below trusts such a proto.
    let unique = |index: u32| made[index as usize] == 1;
    // The upvalue chain, resolved to the binding it ends at.
    fn resolve(
        program: &Program,
        parent: &[Option<Owner>],
        function: u32,
        index: u32,
    ) -> Option<Binding> {
        let owner = parent[function as usize]?;
        match program.functions[function as usize]
            .upvalues
            .get(index as usize)?
        {
            Upvalue::ParentLocal(slot) => Some(Binding { owner, slot: *slot }),
            Upvalue::ParentUpvalue(next) => match owner {
                Owner::Function(p) => resolve(program, parent, p, *next),
                Owner::Entry => None,
            },
        }
    }
    let cell_slots = |owner: Owner| -> HashSet<u32> {
        graph_of(owner)
            .blocks
            .iter()
            .flat_map(|b| b.operations.iter())
            .filter_map(|(_, op)| match op {
                GraphOp::MakeCell(slot) => Some(*slot),
                _ => None,
            })
            .collect()
    };
    let cells: HashMap<Owner, HashSet<u32>> = owners.iter().map(|&o| (o, cell_slots(o))).collect();
    let defines = |owner: Owner, value: ValueId| -> Stored {
        for block in &graph_of(owner).blocks {
            for (vid, op) in &block.operations {
                if *vid == value {
                    return match op {
                        GraphOp::MakeFunction { index, .. } => Stored::Closure(*index),
                        GraphOp::Const {
                            constant: Constant::Undefined,
                            ..
                        } => Stored::Undefined,
                        _ => Stored::Other,
                    };
                }
            }
        }
        Stored::Other
    };

    // Every store to every binding, and every read of one.
    let mut stores: HashMap<Binding, Vec<Store>> = HashMap::new();
    let mut reads: HashMap<Binding, Vec<(Owner, ValueId)>> = HashMap::new();
    let mut poisoned = false;
    for &owner in &owners {
        let graph = graph_of(owner);
        for block in &graph.blocks {
            for (position, (vid, op)) in block.operations.iter().enumerate() {
                let (binding, stored) = match op {
                    GraphOp::StoreCell { slot, value } => {
                        (Some(Binding { owner, slot: *slot }), Some(*value))
                    }
                    GraphOp::StoreLocal { slot, value } if !cells[&owner].contains(slot) => {
                        (Some(Binding { owner, slot: *slot }), Some(*value))
                    }
                    GraphOp::StoreGlobal { slot, value } => (
                        Some(Binding {
                            owner: Owner::Entry,
                            slot: *slot,
                        }),
                        Some(*value),
                    ),
                    GraphOp::StoreUpvalue { index, value } => match owner {
                        Owner::Function(f) => (resolve(program, &parent, f, *index), Some(*value)),
                        Owner::Entry => (None, Some(*value)),
                    },
                    GraphOp::LoadCell(slot) => (Some(Binding { owner, slot: *slot }), None),
                    GraphOp::LoadGlobal(slot) => (
                        Some(Binding {
                            owner: Owner::Entry,
                            slot: *slot,
                        }),
                        None,
                    ),
                    GraphOp::LoadUpvalue(index) => match owner {
                        Owner::Function(f) => (resolve(program, &parent, f, *index), None),
                        Owner::Entry => (None, None),
                    },
                    _ => (None, None),
                };
                match (binding, stored) {
                    (Some(binding), Some(value)) => {
                        stores.entry(binding).or_default().push(Store {
                            owner,
                            block: block.id,
                            position,
                            stored: defines(owner, value),
                        })
                    }
                    (Some(binding), None) => reads.entry(binding).or_default().push((owner, *vid)),
                    (None, Some(_)) => {
                        // A store through an upvalue chain this unit cannot follow could land
                        // on any binding, so none may be trusted.
                        poisoned = true;
                    }
                    (None, None) => {}
                }
            }
        }
    }

    // The binding rule.
    let mut known_bindings: HashMap<Binding, u32> = HashMap::new();
    if poisoned {
        stores.clear();
    }
    for (binding, binding_stores) in &stores {
        // A global slot read as a cell is a box, not a value; leave it alone.
        if binding.owner == Owner::Entry && cells[&Owner::Entry].contains(&binding.slot) {
            let read_raw = reads.get(binding).is_some_and(|rs| {
                rs.iter().any(|(owner, vid)| {
                    graph_of(*owner)
                        .blocks
                        .iter()
                        .flat_map(|b| b.operations.iter())
                        .any(|(v, op)| v == vid && matches!(op, GraphOp::LoadGlobal(_)))
                })
            });
            if read_raw {
                continue;
            }
        }
        let closures: Vec<&Store> = binding_stores
            .iter()
            .filter(|s| matches!(s.stored, Stored::Closure(_)))
            .collect();
        let [defining] = closures[..] else { continue };
        let Stored::Closure(index) = defining.stored else {
            continue;
        };
        if defining.owner != binding.owner || !unique(index) {
            continue;
        }
        let graph = graph_of(binding.owner);
        if defining.block != graph.entry_block {
            continue;
        }
        let others_hoisted = binding_stores.iter().all(|s| {
            std::ptr::eq(s, defining)
                || (s.stored == Stored::Undefined
                    && s.owner == binding.owner
                    && s.block == graph.entry_block
                    && s.position < defining.position)
        });
        if !others_hoisted {
            continue;
        }
        let entry = graph.block(graph.entry_block).expect("entry block");
        let inert = entry.operations[..defining.position]
            .iter()
            .all(|(_, op)| match op {
                GraphOp::Const { .. }
                | GraphOp::MakeCell(_)
                | GraphOp::MarkUninitialized(_)
                | GraphOp::MakeFunction { .. }
                | GraphOp::LoadLocal(_)
                | GraphOp::LoadBuiltin(_)
                | GraphOp::LoadThis
                | GraphOp::LoadNewTarget
                | GraphOp::LoadUpvalue(_)
                | GraphOp::CheckTdz { .. } => true,
                GraphOp::StoreCell { value, .. }
                | GraphOp::StoreLocal { value, .. }
                | GraphOp::StoreGlobal { value, .. } => {
                    matches!(
                        defines(binding.owner, *value),
                        Stored::Undefined | Stored::Closure(_)
                    )
                }
                GraphOp::LoadCell(slot) => *slot != binding.slot,
                GraphOp::LoadGlobal(slot) => binding.owner != Owner::Entry || *slot != binding.slot,
                _ => false,
            });
        if !inert {
            continue;
        }
        known_bindings.insert(*binding, index);
    }

    // Known values: the closures themselves, the reads of known bindings, and what aliases them.
    let mut known = Known {
        functions: vec![KnownValues::default(); graphs.len()],
        entry: KnownValues::default(),
    };
    fn values_of(known: &mut Known, owner: Owner) -> &mut KnownValues {
        match owner {
            Owner::Entry => &mut known.entry,
            Owner::Function(i) => &mut known.functions[i as usize],
        }
    }
    for (binding, index) in &known_bindings {
        // A binding the owner never marks uninitialized holds `undefined` from the first
        // instruction and the closure from the defining store: no read of it can be in the
        // dead zone.
        let marked = graph_of(binding.owner)
            .blocks
            .iter()
            .flat_map(|b| b.operations.iter())
            .any(|(_, op)| matches!(op, GraphOp::MarkUninitialized(slot) if *slot == binding.slot));
        for (owner, vid) in reads.get(binding).into_iter().flatten() {
            let values = values_of(&mut known, *owner);
            values.known.insert(*vid, *index);
            if !marked {
                values.unchecked.insert(*vid);
            }
        }
    }
    for &owner in &owners {
        let graph = graph_of(owner);
        let map = &mut values_of(&mut known, owner).known;
        for block in &graph.blocks {
            for (vid, op) in &block.operations {
                if let GraphOp::MakeFunction { index, .. } = op
                    && unique(*index)
                {
                    map.insert(*vid, *index);
                }
            }
        }
        // Aliases, in order: a check or a store of a known value is that value.
        loop {
            let mut changed = false;
            for block in &graph.blocks {
                for (vid, op) in &block.operations {
                    let source = match op {
                        GraphOp::CheckTdz { value, .. }
                        | GraphOp::StoreLocal { value, .. }
                        | GraphOp::StoreCell { value, .. }
                        | GraphOp::StoreGlobal { value, .. }
                        | GraphOp::StoreUpvalue { value, .. } => Some(*value),
                        _ => None,
                    };
                    if let Some(source) = source
                        && let Some(&index) = map.get(&source)
                        && map.insert(*vid, index).is_none()
                    {
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }
    known
}
