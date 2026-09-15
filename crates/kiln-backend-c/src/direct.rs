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

use kiln_ir::{Constant, FunctionProto, Instruction, Program, Upvalue};

use crate::emit_graph::method_candidates;
use crate::emit_graph::sites::this_columns;
use crate::graph::{
    BlockId, FunctionGraph, GraphOp, Proven, Terminator, ValueId, proven_types_with,
};

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

/// What a typed entry's body reaches for beyond its parameters. Each is a parameter of the
/// entry only when needed, so a body that needs none carries nothing across its calls but the
/// depth and its numbers. Found by emitting the body and reading it: what the C names is what
/// it needs, and a typed callee's needs are its caller's.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct TypedNeeds {
    /// `host` and `thrown`: any operation that can call the runtime or throw.
    pub runtime: bool,
    /// The closure's upvalue table.
    pub upvalues: bool,
    /// `this`.
    pub receiver: bool,
    /// The receiver-shape cache row for `this`, when the caller has it; NULL when it does not,
    /// and the entry finds or claims one itself.
    pub row: bool,
}

/// How one body's calls are emitted.
#[derive(Default, Clone, Debug)]
pub struct CallPlan {
    /// Use terse generated operations and declarations.
    pub compact: bool,
    /// Emit only the generic body; native-dev uses this to keep the edit/run loop proportional
    /// to the program instead of multiplying it by speculative clones.
    pub generic_only: bool,
    /// Explicit speculation setting resolved by the native orchestration layer. `None` keeps the
    /// direct emitter's default for backend-only callers.
    pub speculate: Option<bool>,
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
    /// Reads of a settled binding stored exactly once: the value is the same at every read,
    /// so a fact checked about it at entry holds for the whole body.
    pub invariant: HashSet<ValueId>,
    /// Per proto: whether its typed entry is proven to return a number, and so hands its
    /// result back as a `double`.
    pub typed_returns: Vec<bool>,
    /// Per proto: what its typed entry takes beyond its parameters.
    pub typed_needs: Vec<TypedNeeds>,
    /// Per proto: whether its typed entry exists in the unit.
    pub typed_entries: Vec<bool>,
    /// Per proto: its body passes [`crate::parallel::parallel_safe`] and its typed entry returns
    /// a number, so a `parallelMap` site whose callback is proven to be it may run the proto's
    /// parallel clone on a worker thread -- once `set_parallel` has confirmed the clone's emitted
    /// text reaches no host, receiver, upvalue or heap.
    pub parallel_safe: Vec<bool>,
    /// Per proto: how many columns its receiver-shape cache has -- the names its body reads
    /// of `this` -- so a call site can look a row up in the callee's table.
    pub receiver_columns: Vec<usize>,
    /// Per proto: whether a fast clone exists -- the body with every slow path replaced by a
    /// restart through the boxed entry, for a body that can be restarted.
    pub fast: Vec<bool>,
    /// Per proto: whether its boxed body is present in this closed-world translation unit.
    /// Empty means every body is available, preserving plans built outside module emission.
    pub available: Vec<bool>,
    /// Per proto: whether a call into it must publish the per-frame context, from
    /// [`crate::compute_publishes_frame`]'s one whole-program fixpoint. Cloned into every
    /// function's plan the same way `typed_entries` is, so every user reads the same answer
    /// instead of recomputing it -- the emitted C's `plain->publishes_frame` runtime flag must
    /// agree with this at every proto, since call sites branch on both.
    pub publishes_frame: Vec<bool>,
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
    pub fn plan(
        program: &Program,
        graphs: &[FunctionGraph],
        entry: &FunctionGraph,
        publishes_frame: &[bool],
        compact: bool,
        generic_only: bool,
        speculate: Option<bool>,
    ) -> Self {
        if generic_only {
            // None of the known-callee, numeric fixpoint, typed-entry, callback, or receiver
            // analyses can affect a generic body. Large dependency modules choose this path
            // specifically to keep emission proportional to their bytecode, so do not compute
            // speculative facts only to erase them below.
            let function_count = graphs.len();
            let plan = || CallPlan {
                compact,
                generic_only: true,
                speculate,
                typed_returns: vec![false; function_count],
                typed_needs: vec![TypedNeeds::default(); function_count],
                typed_entries: vec![false; function_count],
                parallel_safe: vec![false; function_count],
                receiver_columns: vec![0; function_count],
                fast: vec![false; function_count],
                available: vec![true; function_count],
                publishes_frame: publishes_frame.to_vec(),
                ..CallPlan::default()
            };
            return Self {
                functions: (0..function_count).map(|_| plan()).collect(),
                typed: vec![None; function_count],
                entry: plan(),
            };
        }
        let known = known_callees(program, graphs, entry);
        let direct: Vec<bool> = program
            .functions
            .iter()
            .zip(publishes_frame)
            .map(|(proto, &publishes)| direct_callable(proto, publishes))
            .collect();
        // A typed entry's parameters are its declared ones, unpacked as `double`s -- no `args`
        // array, no `argc`, because its contract is "called with exactly its parameter count of
        // arguments" (see the module doc). `RestArguments` is emitted against the *boxed*
        // calling convention's `args`/`argc` (`tail.rs`), which a typed entry does not have, so a
        // proto that reads a rest parameter keeps its boxed and fast-clone direct-call
        // eligibility (`direct`, computed above) but is excluded here: entering it typed would be
        // emitting `args`/`argc` in a body that never declares them, a compile error rather than
        // a wrong answer, but still not something to emit.
        let has_rest_parameter: Vec<bool> = program
            .functions
            .iter()
            .map(|proto| {
                proto
                    .code
                    .iter()
                    .any(|instruction| matches!(instruction, Instruction::RestArguments(_)))
            })
            .collect();
        let typed_eligible: Vec<bool> = direct
            .iter()
            .zip(&has_rest_parameter)
            .zip(&program.functions)
            .map(|((&direct, &has_rest), proto)| {
                direct
                    && !has_rest
                    && !generic_only
                    && proto.code.len() <= crate::MAX_SPECIALIZED_INSTRUCTIONS
            })
            .collect();
        let intrinsics: Vec<HashMap<ValueId, Proven>> = graphs
            .iter()
            .map(|g| intrinsic_results(g, program))
            .collect();
        let entry_intrinsics = intrinsic_results(entry, program);
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
                let mut seeds: HashMap<ValueId, Proven> = graph
                    .initial_locals
                    .iter()
                    .take(params[i] as usize)
                    .map(|&v| (v, Proven::Number))
                    .collect();
                let (this_cols, _) = this_columns(graph);
                for (vid, _) in this_cols {
                    seeds.insert(vid, Proven::Number);
                }
                seeds.extend(intrinsics[i].iter().map(|(&v, &k)| (v, k)));
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

        let parallel_safe: Vec<bool> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                typed_eligible[i]
                    && typed_returns[i]
                    && crate::parallel::parallel_safe(g, &program.functions[i], program)
            })
            .collect();
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
                compact,
                generic_only,
                speculate,
                known: if generic_only {
                    HashMap::new()
                } else {
                    known.known.clone()
                },
                typed_sites,
                numbers: (!generic_only)
                    .then(|| {
                        proven
                            .iter()
                            .filter_map(|(&v, &kind)| kind.is_number().then_some(v))
                            .collect()
                    })
                    .unwrap_or_default(),
                ints: (!generic_only)
                    .then(|| {
                        proven
                            .into_iter()
                            .filter_map(|(v, kind)| (kind == Proven::Int).then_some(v))
                            .collect()
                    })
                    .unwrap_or_default(),
                unchecked: known.unchecked.clone(),
                invariant: known.invariant.clone(),
                typed_returns: typed_returns.clone(),
                typed_needs: vec![TypedNeeds::default(); graphs.len()],
                typed_entries: vec![false; graphs.len()],
                parallel_safe: parallel_safe.clone(),
                receiver_columns: vec![0; graphs.len()],
                fast: vec![false; graphs.len()],
                available: vec![true; graphs.len()],
                publishes_frame: publishes_frame.to_vec(),
            }
        };
        let functions: Vec<CallPlan> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                plan_for(
                    g,
                    &known.functions[i],
                    &function_sites[i],
                    intrinsics[i].clone(),
                )
            })
            .collect();
        let typed: Vec<Option<CallPlan>> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                if !typed_eligible[i] {
                    return None;
                }
                let mut seeds: HashMap<ValueId, Proven> = g
                    .initial_locals
                    .iter()
                    .take(params[i] as usize)
                    .map(|&v| (v, Proven::Number))
                    .collect();
                let (this_cols, _) = this_columns(g);
                for (vid, _) in this_cols {
                    seeds.insert(vid, Proven::Number);
                }
                seeds.extend(intrinsics[i].iter().map(|(&v, &k)| (v, k)));
                Some(plan_for(g, &known.functions[i], &function_sites[i], seeds))
            })
            .collect();
        let entry_plan = plan_for(entry, &known.entry, &entry_sites, entry_intrinsics);

        // A method call reaches a typed entry by name when the callee the cache remembers is
        // one of the unit's own methods of that name, so every such method a site could enter
        // typed -- the arguments proven numbers, the count exact -- is referenced.
        let method_targets = |graph: &FunctionGraph, plan: &CallPlan| -> Vec<u32> {
            let mut targets = Vec::new();
            for (_, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
                if let GraphOp::CallMethod { name, args, .. } = op
                    && let Some(Constant::String(key)) = program.constants.get(*name as usize)
                    && args.iter().all(|a| plan.numbers.contains(a))
                {
                    targets.extend(
                        method_candidates(program, key, publishes_frame)
                            .into_iter()
                            .filter(|&i| params[i as usize] as usize == args.len()),
                    );
                }
            }
            targets
        };

        // An intrinsic array loop (map, filter, reduce) whose one callback argument is a value
        // this body proves is the closure of proto `i` -- not a cached guess, a proof -- reaches
        // that proto's typed entry the same way a by-name candidate does when the callback's own
        // declared arity is one the loop can call typed: element, or element and index, for
        // map/filter; accumulator and element for reduce. An entry only these sites would use is
        // otherwise dead C, so it is referenced here exactly as `method_targets` references a
        // by-name candidate.
        let callback_targets = |graph: &FunctionGraph, plan: &CallPlan| -> Vec<u32> {
            let mut targets = Vec::new();
            for (_, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
                if let GraphOp::CallMethod { name, args, .. } = op
                    && let Some(Constant::String(key)) = program.constants.get(*name as usize)
                {
                    let arities: &[u32] = match (key.to_scalar_string().ok().as_deref(), args.len())
                    {
                        (Some("map"), 1) | (Some("filter"), 1) => &[1, 2],
                        (Some("reduce"), 2) => &[2],
                        // `parallelMap`'s pooled lowering calls the typed entry from its chunk
                        // worker (ADR 0058, application 2); the callback is referenced only when
                        // its body may run there, so a non-qualifying one gets no dead entry.
                        (Some("parallelMap"), 1)
                            if plan
                                .known
                                .get(&args[0])
                                .is_some_and(|&t| parallel_safe[t as usize]) =>
                        {
                            &[1, 2]
                        }
                        _ => continue,
                    };
                    if let Some(&target) = plan.known.get(&args[0])
                        && arities.contains(&params[target as usize])
                    {
                        targets.push(target);
                    }
                }
            }
            targets
        };

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
        for (graph, plan) in graphs.iter().zip(&functions) {
            referenced.extend(method_targets(graph, plan));
            referenced.extend(callback_targets(graph, plan));
        }
        referenced.extend(method_targets(entry, &entry_plan));
        referenced.extend(callback_targets(entry, &entry_plan));
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
                    next.extend(method_targets(&graphs[i], plan));
                    next.extend(callback_targets(&graphs[i], plan));
                }
            }
            referenced = next;
            if referenced.len() == before {
                break;
            }
        }
        let typed: Vec<Option<CallPlan>> = typed
            .into_iter()
            .enumerate()
            .map(|(i, plan)| plan.filter(|_| referenced.contains(&(i as u32))))
            .collect();
        let typed_entries: Vec<bool> = typed.iter().map(Option::is_some).collect();

        let mut plan = DirectPlan {
            functions,
            typed,
            entry: entry_plan,
        };
        for plan in plan
            .functions
            .iter_mut()
            .chain(plan.typed.iter_mut().flatten())
            .chain(std::iter::once(&mut plan.entry))
        {
            plan.typed_entries = typed_entries.clone();
        }
        plan
    }

    /// Record what every typed entry needs, in every body's plan.
    /// Record which protos have a fast clone, in every body's plan.
    pub fn set_fast(&mut self, fast: &[bool]) {
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.fast = fast.to_vec();
        }
    }

    /// Restrict static dispatch and by-name specialization to bodies emitted in this unit.
    pub fn set_available(&mut self, available: &[bool]) {
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.available = available.to_vec();
            for (index, typed) in plan.typed_entries.iter_mut().enumerate() {
                *typed &= available.get(index).copied().unwrap_or(false);
            }
        }
    }

    /// Record every proto's receiver-shape cache width, in every body's plan.
    pub fn set_receiver_columns(&mut self, columns: &[usize]) {
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.receiver_columns = columns.to_vec();
        }
    }

    pub fn set_typed_needs(&mut self, needs: &[TypedNeeds]) {
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.typed_needs = needs.to_vec();
        }
    }

    /// The typed entry of `index` may serve as a `parallelMap` chunk worker: it exists, returns a
    /// number, needs nothing but its parameters, and its body passes
    /// [`crate::parallel::parallel_safe`]. Read after `set_typed_needs`, which is what settles
    /// the needs.
    pub fn parallel_chunk(&self, index: u32) -> bool {
        let i = index as usize;
        self.has_typed_entry(index)
            && self.entry.typed_returns.get(i).copied().unwrap_or(false)
            && self.entry.parallel_safe.get(i).copied().unwrap_or(false)
    }

    /// Narrow `parallel_safe` to the protos whose parallel clone was emitted and scanned clean
    /// (`lib.rs`), in every body's plan, so a `parallelMap` site anywhere reads the final answer.
    pub fn set_parallel(&mut self, admitted: &[bool]) {
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            for (i, ok) in plan.parallel_safe.iter_mut().enumerate() {
                *ok = *ok && admitted.get(i).copied().unwrap_or(false);
            }
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
///
/// `publishes_frame` is this proto's entry in the whole-program
/// [`crate::compute_publishes_frame`] vector, not recomputed here -- it already folds in
/// `generator`/`asynchronous`/`async_generator`, so the two extra checks below are redundant with
/// it in practice and kept only so this reads as a complete answer on its own.
pub(crate) fn direct_callable(proto: &FunctionProto, publishes_frame: bool) -> bool {
    !publishes_frame && !proto.generator && !proto.asynchronous
}

/// What the unit knows about the built-ins it lowers as intrinsics: a `Math` method of the
/// emitter's list, in a program that cannot redefine it, answers a number whenever it answers.
/// The proofs are seeds for every plan of the body, so a body that ends in `Math.sqrt(..)`
/// returns a number wherever its inputs came from.
fn intrinsic_results(graph: &FunctionGraph, program: &Program) -> HashMap<ValueId, Proven> {
    if program.has_direct_eval() || program.has_dynamic_import() {
        return HashMap::new();
    }
    let math: HashSet<ValueId> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| {
            matches!(op, GraphOp::LoadBuiltin(kiln_ir::Builtin::Math)).then_some(*v)
        })
        .collect();
    graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| match op {
            GraphOp::CallMethod {
                object, name, args, ..
            } if math.contains(object) => {
                let key = match program.constants.get(*name as usize) {
                    Some(Constant::String(s)) => s.to_scalar_string().ok()?,
                    _ => return None,
                };
                let intrinsic = crate::parallel::PARALLEL_MATH
                    .iter()
                    .any(|(k, n)| key == *k && *n == args.len());
                intrinsic.then_some((*v, Proven::Number))
            }
            _ => None,
        })
        .collect()
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
    invariant: HashSet<ValueId>,
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
            Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } => {
                Some(Binding { owner, slot: *slot })
            }
            Upvalue::ParentUpvalue(next) | Upvalue::NamedParentUpvalue { index: next, .. } => {
                match owner {
                    Owner::Function(p) => resolve(program, parent, p, *next),
                    Owner::Entry => None,
                }
            }
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

    // A binding whose first store sits behind an inert prefix of its owner's entry block -- an
    // entry block nothing jumps back to -- is initialized before any executor can read it:
    // the owner reads it later in program order, and a closure the prefix made runs only after
    // a call, which the prefix has none of. Every read of such a binding needs no dead-zone
    // check, whatever it holds.
    let mut settled: HashSet<Binding> = HashSet::new();
    for (binding, binding_stores) in &stores {
        let graph = graph_of(binding.owner);
        let Some(entry) = graph.block(graph.entry_block) else {
            continue;
        };
        if !entry.predecessors.is_empty() {
            continue;
        }
        let Some(first) = binding_stores
            .iter()
            .filter(|s| s.owner == binding.owner && s.block == graph.entry_block)
            .map(|s| s.position)
            .min()
        else {
            continue;
        };
        // A value made in this prefix -- a closure, a literal -- has no accessor and no proxy
        // behind it, so defining or reading its properties runs nothing of the program's.
        // That is what a class body does before the code after it can run.
        fn fresh(prefix: &[(ValueId, GraphOp)], value: ValueId, depth: u32) -> bool {
            if depth > 4 {
                return false;
            }
            let Some(at) = prefix.iter().position(|(vid, _)| *vid == value) else {
                return false;
            };
            match &prefix[at].1 {
                GraphOp::MakeFunction { .. }
                | GraphOp::NewObject { .. }
                | GraphOp::NewArray { .. } => true,
                GraphOp::CheckTdz { value, .. } => fresh(prefix, *value, depth + 1),
                // A cell read is the value the prefix last stored there, if it stored one.
                GraphOp::LoadCell(slot) => prefix[..at]
                    .iter()
                    .rev()
                    .find_map(|(_, op)| match op {
                        GraphOp::StoreCell { slot: s, value } if s == slot => Some(*value),
                        _ => None,
                    })
                    .is_some_and(|stored| fresh(prefix, stored, depth + 1)),
                _ => false,
            }
        }
        let prefix = &entry.operations[..first];
        let inert_op = |op: &GraphOp| match op {
            GraphOp::Const { .. }
            | GraphOp::MakeCell(_)
            | GraphOp::MarkUninitialized(_)
            | GraphOp::MakeFunction { .. }
            | GraphOp::LoadLocal(_)
            | GraphOp::LoadBuiltin(_)
            | GraphOp::LoadThis
            | GraphOp::LoadNewTarget
            | GraphOp::LoadUpvalue(_)
            | GraphOp::CheckTdz { .. }
            | GraphOp::StoreCell { .. }
            | GraphOp::StoreLocal { .. }
            | GraphOp::StoreGlobal { .. }
            | GraphOp::NewArray { .. }
            | GraphOp::NewObject { .. } => true,
            GraphOp::GetProperty { object, .. }
            | GraphOp::SetProperty { object, .. }
            | GraphOp::SetPropertyStrict { object, .. } => fresh(prefix, *object, 0),
            GraphOp::Bridge { instruction, .. } => matches!(
                instruction,
                Instruction::DefineMethod
                    | Instruction::DefineAccessor { .. }
                    | Instruction::DefineNamedProperty
                    | Instruction::DefinePrivate { .. }
                    | Instruction::SetPrototype
                    | Instruction::PushHole
                    | Instruction::NewRegExp { .. }
            ),
            GraphOp::LoadCell(slot) => *slot != binding.slot,
            GraphOp::LoadGlobal(slot) => binding.owner != Owner::Entry || *slot != binding.slot,
            _ => false,
        };
        let inert = entry.operations[..first].iter().all(|(_, op)| inert_op(op));
        if inert {
            settled.insert(*binding);
        }
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
    for binding in &settled {
        for (owner, vid) in reads.get(binding).into_iter().flatten() {
            values_of(&mut known, *owner).unchecked.insert(*vid);
        }
        // A settled binding with one store holds one value for the rest of the program: every
        // read of it, from wherever, answers the same thing.
        if stores.get(binding).is_some_and(|s| s.len() == 1) {
            for (owner, vid) in reads.get(binding).into_iter().flatten() {
                values_of(&mut known, *owner).invariant.insert(*vid);
            }
        }
    }
    for (binding, index) in &known_bindings {
        for (owner, vid) in reads.get(binding).into_iter().flatten() {
            values_of(&mut known, *owner).known.insert(*vid, *index);
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
