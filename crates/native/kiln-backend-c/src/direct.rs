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

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use kiln_ir::{Constant, FunctionProto, Instruction, Program, UnaryOp, Upvalue};

use crate::emit_graph::method_candidates_map;
use crate::emit_graph::sites::this_columns;
use crate::graph::{
    BlockId, FunctionGraph, GraphOp, Proven, Terminator, ValueId, proven_types_with_bounds,
};

/// Generic-only release modules skip whole-program specialization to bound analysis and code
/// size. Preserve a small direct-call island for modules whose entire graph is still cheap to
/// inspect: it emits no extra body, and the existing `direct_callable` check retains the boxed
/// runtime fallback for anything that publishes a frame or can suspend.
const MAX_GENERIC_DIRECT_ANALYSIS_FUNCTIONS: usize = 128;
const MAX_GENERIC_DIRECT_ANALYSIS_INSTRUCTIONS: usize = 8_192;
// Proto count is bounded independently from total IR: real dependency modules may contain many
// tiny closures, while graph work remains capped by the instruction, caller, and edge limits.
pub(crate) const MAX_GENERIC_LOOP_CALL_FUNCTIONS: usize = 1_024;
pub(crate) const MAX_GENERIC_LOOP_CALL_INSTRUCTIONS: usize = 32_768;
pub(crate) const MAX_GENERIC_LOOP_CALLERS: usize = 256;
const MAX_GENERIC_LOOP_OPTIMIZED_FUNCTIONS: usize = 512;
const MAX_GENERIC_LOOP_CALL_EDGES: usize = 4_096;
const MAX_GENERIC_LOOP_CALL_DEPTH: usize = 4;
const MAX_GENERIC_LOOP_PROFILE_CALLER_INSTRUCTIONS: usize = 1_536;

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

fn binding_for_global(slot: u32) -> Binding {
    match crate::module_cells::published_origin(slot) {
        Some((function, local)) => Binding {
            owner: Owner::Function(function),
            slot: local,
        },
        None => Binding {
            owner: Owner::Entry,
            slot,
        },
    }
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
    /// Whether to compile static regex literals into direct C matchers.
    pub compile_regex: bool,
    /// Emit only the generic body; native-dev uses this to keep the edit/run loop proportional
    /// to the program instead of multiplying it by speculative clones.
    pub generic_only: bool,
    /// This boxed body is selected for the bounded mixed optimization tier used by large
    /// generic-only release modules. The ABI and body are unchanged; only the compiler's
    /// optimization level differs from the O0 generic functions beside it.
    pub generic_function_optimized: bool,
    /// With `generic_only`: the unit still lowers its qualifying `parallelMap` sites (see
    /// [`DirectPlan::plan_parallel_sites`]); without it, a site says `dev-profile`.
    pub parallel_in_generic: bool,
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
    /// Reads of a binding initialized once: an entry guard accepting its initialized value
    /// proves that value stays fixed for the whole body. This does not waive dead-zone checks.
    pub invariant: HashSet<ValueId>,
    /// Per proto: whether its typed entry is proven to return a number, and so hands its
    /// result back as a `double`.
    pub typed_returns: std::sync::Arc<Vec<bool>>,
    /// Per proto: what its typed entry takes beyond its parameters.
    pub typed_needs: std::sync::Arc<Vec<TypedNeeds>>,
    /// Per proto: the built-in guard a caller evaluates before entering its typed entry, or
    /// empty when the entry relies on no built-in it cannot fall back from (issue 20). A typed
    /// entry has no generic path of its own: an inlined `Math.sqrt` whose guard fails inside it
    /// can only refuse, and by then the body may have had effects. So the entry's call sites ask
    /// first -- about every member the entry and its typed callees speculate on -- and when the
    /// answer is no they take the boxed call they would have made without a typed entry. Past
    /// that check the guard inside the entry fails only if the call itself modified the built-in.
    pub typed_guards: std::sync::Arc<Vec<String>>,
    /// Per proto: whether its typed entry exists in the unit.
    pub typed_entries: std::sync::Arc<Vec<bool>>,
    /// Per proto: its body passes [`crate::parallel::parallel_safe`] and its typed entry returns
    /// a number, so a `parallelMap` site whose callback is proven to be it may run the proto's
    /// parallel clone on a worker thread -- once `set_parallel` has confirmed the clone's emitted
    /// text reaches no host, receiver, upvalue or heap.
    pub parallel_safe: std::sync::Arc<Vec<bool>>,
    /// Per proto: the `parallelMap` verdict behind `parallel_safe` -- the captures a qualifying
    /// callback reads, or the first rule it failed -- shared by every plan, so the call site
    /// that acts on the decision and `--explain-parallel` read one answer.
    pub parallel:
        std::sync::Arc<Vec<Result<crate::parallel::ParallelShape, crate::parallel::Refusal>>>,
    /// Per proto: how many columns its receiver-shape cache has -- the names its body reads
    /// of `this` -- so a call site can look a row up in the callee's table.
    pub receiver_columns: std::sync::Arc<Vec<usize>>,
    /// Per proto: whether a fast clone exists -- the body with every slow path replaced by a
    /// restart through the boxed entry, for a body that can be restarted.
    pub fast: std::sync::Arc<Vec<bool>>,
    /// Per proto: whether its boxed body is present in this closed-world translation unit.
    /// Empty means every body is available, preserving plans built outside module emission.
    pub available: std::sync::Arc<Vec<bool>>,
    /// Per proto: whether a call into it must publish the per-frame context, from
    /// [`crate::compute_publishes_frame`]'s one whole-program fixpoint. Cloned into every
    /// function's plan through a shared immutable table, so every user reads the same answer
    /// instead of recomputing it -- the emitted C's `plain->publishes_frame` runtime flag must
    /// agree with this at every proto, since call sites branch on both.
    pub publishes_frame: std::sync::Arc<Vec<bool>>,
    /// Whether the translation unit is chunked across multiple C files.
    pub should_chunk: bool,
}

/// The whole unit's plan: a boxed plan per function, a typed one where the typed entry exists,
/// and the entry point's.
#[derive(Debug)]
pub struct DirectPlan {
    pub functions: Vec<CallPlan>,
    pub typed: Vec<Option<CallPlan>>,
    /// Per proto: the plan its parallel clone (`_p`) is emitted from, where the callback
    /// qualifies for `parallelMap`. The typed plan's seeds plus the captured reads of
    /// `parallel::ParallelShape`, which the clone receives as `double`s the call site guarded.
    pub parallel: Vec<Option<CallPlan>>,
    pub entry: CallPlan,
    /// Bounded proven call edges used only to rank the supplemental generic loop tier after
    /// module liveness settles. This does not enable direct calls or change any ABI.
    generic_loop_calls: Option<Arc<LoopCallProfile>>,
}

impl DirectPlan {
    pub fn plan(
        program: &Program,
        graphs: &[FunctionGraph],
        entry: &FunctionGraph,
        publishes_frame: &[bool],
        compact: bool,
        compile_regex: bool,
        generic_only: bool,
        parallel_in_generic: bool,
        speculate: Option<bool>,
    ) -> Self {
        Self::plan_with_loop_profile(
            program,
            graphs,
            entry,
            publishes_frame,
            compact,
            compile_regex,
            generic_only,
            parallel_in_generic,
            speculate,
            None,
        )
    }

    pub(crate) fn plan_with_loop_profile(
        program: &Program,
        graphs: &[FunctionGraph],
        entry: &FunctionGraph,
        publishes_frame: &[bool],
        compact: bool,
        compile_regex: bool,
        generic_only: bool,
        parallel_in_generic: bool,
        speculate: Option<bool>,
        loop_profile_override: Option<Arc<LoopCallProfile>>,
    ) -> Self {
        if generic_only {
            let function_count = graphs.len();
            let total_instructions = program
                .functions
                .iter()
                .map(|proto| proto.code.len())
                .chain(std::iter::once(program.code.len()))
                .fold(0usize, usize::saturating_add);
            // Known-callee facts only replace a dynamic call with a call by C function name;
            // they add no clone and no type assumption. Keep this analysis bounded across the
            // full module so very large dependencies still take the cheap all-generic path.
            let known = (function_count <= MAX_GENERIC_DIRECT_ANALYSIS_FUNCTIONS
                && total_instructions <= MAX_GENERIC_DIRECT_ANALYSIS_INSTRUCTIONS)
                .then(|| known_callees(program, graphs, entry));
            let generic_loop_calls = loop_profile_override.or_else(|| {
                if let Some(known) = known.as_ref() {
                    LoopCallProfile::from_known(program, graphs, entry, known).map(Arc::new)
                } else if function_count <= MAX_GENERIC_LOOP_CALL_FUNCTIONS
                    && total_instructions <= MAX_GENERIC_LOOP_CALL_INSTRUCTIONS
                    && loop_caller_count(program) <= MAX_GENERIC_LOOP_CALLERS
                {
                    let known = known_callees(program, graphs, entry);
                    LoopCallProfile::from_known(program, graphs, entry, &known).map(Arc::new)
                } else {
                    None
                }
            });
            let optimized_functions = if let Some(profile) = generic_loop_calls.as_ref() {
                select_generic_optimization_functions_with_profile(
                    program,
                    publishes_frame,
                    None,
                    profile,
                )
            } else {
                select_generic_optimization_functions_with_weights(
                    program,
                    publishes_frame,
                    None,
                    None,
                )
            };
            let generic_verdicts = std::sync::Arc::new(vec![
                Err(crate::parallel::Refusal::new(
                    if parallel_in_generic {
                        crate::parallel::Reason::CallbackUnknown
                    } else {
                        crate::parallel::Reason::DevProfile
                    }
                ));
                function_count
            ]);
            // Unit-wide tables are immutable snapshots shared by every body's plan. Copying
            // one function-count table per function made large npm modules quadratic in memory.
            let false_flags = std::sync::Arc::new(vec![false; function_count]);
            let typed_needs = std::sync::Arc::new(vec![TypedNeeds::default(); function_count]);
            let columns = std::sync::Arc::new(vec![0; function_count]);
            let available = std::sync::Arc::new(vec![true; function_count]);
            let publishes_frame = std::sync::Arc::new(publishes_frame.to_vec());
            let plan = |generic_function_optimized: bool, known: HashMap<ValueId, u32>| CallPlan {
                compact,
                compile_regex,
                generic_only: true,
                generic_function_optimized,
                parallel_in_generic,
                speculate,
                known,
                typed_returns: false_flags.clone(),
                typed_needs: typed_needs.clone(),
                typed_entries: false_flags.clone(),
                parallel_safe: false_flags.clone(),
                parallel: generic_verdicts.clone(),
                receiver_columns: columns.clone(),
                fast: false_flags.clone(),
                available: available.clone(),
                publishes_frame: publishes_frame.clone(),
                ..CallPlan::default()
            };
            let mut generic = Self {
                functions: (0..function_count)
                    .map(|index| {
                        plan(
                            optimized_functions[index],
                            known
                                .as_ref()
                                .map(|known| known.functions[index].known.clone())
                                .unwrap_or_default(),
                        )
                    })
                    .collect(),
                typed: vec![None; function_count],
                parallel: vec![None; function_count],
                generic_loop_calls,
                entry: plan(
                    false,
                    known
                        .as_ref()
                        .map(|known| known.entry.known.clone())
                        .unwrap_or_default(),
                ),
            };
            if parallel_in_generic {
                generic.plan_parallel_sites(program, graphs, entry, &publishes_frame, speculate);
            }
            return generic;
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
        let has_open_world = program.has_direct_eval() || program.has_dynamic_import();
        let intrinsics: Vec<HashMap<ValueId, Proven>> = if has_open_world {
            vec![HashMap::new(); graphs.len()]
        } else {
            graphs
                .iter()
                .map(|g| intrinsic_results(g, program))
                .collect()
        };
        let entry_intrinsics = if has_open_world {
            HashMap::new()
        } else {
            intrinsic_results(entry, program)
        };
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

        let length_bounds_of = |g: &FunctionGraph| -> HashSet<ValueId> {
            g.blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .filter_map(|(v, op)| match op {
                    GraphOp::GetProperty { name, .. } => {
                        match program.constants.get(*name as usize) {
                            Some(Constant::String(s)) if s == "length" => Some(*v),
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .collect()
        };
        let length_bounds: Vec<HashSet<ValueId>> =
            graphs.iter().map(|g| length_bounds_of(g)).collect();
        let entry_length_bounds = length_bounds_of(entry);

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
                    &length_bounds[i],
                );
                // Every return a number. A block nothing reaches -- the `return undefined` the
                // compiler leaves after a body's last statement -- returns nothing.
                let returns_number = graph
                    .blocks
                    .iter()
                    .filter(|b| b.is_reachable_from_entry(graph.entry_block))
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

        // The `parallelMap` verdict per proto (ADR 0058 application 2, ADR 0069 tier 1): the
        // source rules of `parallel::parallel_verdict`, then a typed entry, then a number
        // returned on every path with the parameters and the captured reads assumed numbers --
        // the assumption the call site's run-time guard makes true before any chunk runs.
        //
        // Only a proto some `parallelMap` site is known to be called with is considered: every
        // other would carry a parallel clone and its chunk workers that nothing calls.
        let mut parallel_callbacks: HashSet<u32> = HashSet::new();
        for (graph, known) in graphs
            .iter()
            .zip(&known.functions)
            .chain(std::iter::once((entry, &known.entry)))
        {
            for (_, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
                if let GraphOp::CallMethod { name, args, .. } = op
                    && let Some(Constant::String(key)) = program.constants.get(*name as usize)
                    && let Some(method) = key
                        .to_scalar_string()
                        .ok()
                        .and_then(|k| crate::parallel::ParallelMethod::from_name(&k))
                {
                    for &(position, _) in method.callbacks(args.len()) {
                        if let Some(&target) = known.known.get(&args[position]) {
                            parallel_callbacks.insert(target);
                        }
                    }
                }
            }
        }
        let mut parallel_seeds: Vec<Option<HashMap<ValueId, Proven>>> = vec![None; graphs.len()];
        let parallel_verdicts: Vec<
            Result<crate::parallel::ParallelShape, crate::parallel::Refusal>,
        > = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                use crate::parallel::{Reason, Refusal};
                if !parallel_callbacks.contains(&(i as u32)) {
                    return Err(Refusal::new(Reason::CallbackUnknown));
                }
                // A callback tier 1 refuses may still read the heap in parallel (tier 2).
                let tier_2 = |refusal| {
                    let lookup = |f: u32, v: ValueId| known.parallel_callee(graphs, f, v);
                    let calls = crate::parallel::CallGraph {
                        graphs,
                        captures: &known.captures,
                        known: &lookup,
                        root: i as u32,
                    };
                    crate::parallel::tier_2(
                        g,
                        &program.functions[i],
                        program,
                        &known.captures[i],
                        refusal,
                        Some(&calls),
                    )
                };
                let shape = match crate::parallel::parallel_verdict(
                    g,
                    &program.functions[i],
                    program,
                    &known.captures[i],
                ) {
                    Ok(shape) => shape,
                    Err(refusal) => return tier_2(refusal),
                };
                if !typed_eligible[i] {
                    return tier_2(Refusal::new(Reason::NotTyped));
                }
                let mut seeds: HashMap<ValueId, Proven> = g
                    .initial_locals
                    .iter()
                    .take(params[i] as usize)
                    .map(|&v| (v, Proven::Number))
                    .collect();
                seeds.extend(shape.captured_values.iter().map(|&v| (v, Proven::Number)));
                seeds.extend(intrinsics[i].iter().map(|(&v, &k)| (v, k)));
                let (proven, _) = settle(
                    g,
                    seeds.clone(),
                    &function_sites[i],
                    &typed_eligible,
                    &typed_returns,
                    &params,
                    &length_bounds[i],
                );
                let Some(boolean_result) = crate::parallel::parallel_result(g, &proven) else {
                    return Err(Refusal::new(Reason::ReturnsNonNumber));
                };
                parallel_seeds[i] = Some(seeds);
                Ok(crate::parallel::ParallelShape {
                    boolean_result,
                    ..shape
                })
            })
            .collect();
        // A tier-2 verdict has no typed clone: `parallel_safe` is tier 1's.
        let parallel_safe: Vec<bool> = parallel_verdicts
            .iter()
            .map(|v| matches!(v, Ok(shape) if shape.heap.is_none()))
            .collect();
        let parallel_verdicts = std::sync::Arc::new(parallel_verdicts);
        let shared_returns = std::sync::Arc::new(typed_returns.clone());
        let shared_needs = std::sync::Arc::new(vec![TypedNeeds::default(); graphs.len()]);
        let shared_guards = std::sync::Arc::new(vec![String::new(); graphs.len()]);
        let shared_false = std::sync::Arc::new(vec![false; graphs.len()]);
        let shared_parallel_safe = std::sync::Arc::new(parallel_safe.clone());
        let shared_columns = std::sync::Arc::new(vec![0; graphs.len()]);
        let shared_available = std::sync::Arc::new(vec![true; graphs.len()]);
        let shared_publishes = std::sync::Arc::new(publishes_frame.to_vec());
        let plan_for = |graph: &FunctionGraph,
                        known: &KnownValues,
                        sites: &[Site],
                        seeds: HashMap<ValueId, Proven>,
                        lb: &HashSet<ValueId>|
         -> CallPlan {
            let (proven, typed_sites) = settle(
                graph,
                seeds,
                sites,
                &typed_eligible,
                &typed_returns,
                &params,
                lb,
            );
            CallPlan {
                compact,
                compile_regex,
                generic_only,
                generic_function_optimized: false,
                parallel_in_generic,
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
                typed_returns: shared_returns.clone(),
                typed_needs: shared_needs.clone(),
                typed_guards: shared_guards.clone(),
                typed_entries: shared_false.clone(),
                parallel_safe: shared_parallel_safe.clone(),
                parallel: parallel_verdicts.clone(),
                receiver_columns: shared_columns.clone(),
                fast: shared_false.clone(),
                available: shared_available.clone(),
                publishes_frame: shared_publishes.clone(),
                should_chunk: false,
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
                    &length_bounds[i],
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
                Some(plan_for(
                    g,
                    &known.functions[i],
                    &function_sites[i],
                    seeds,
                    &length_bounds[i],
                ))
            })
            .collect();
        let parallel: Vec<Option<CallPlan>> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let seeds = parallel_seeds[i].clone()?;
                Some(plan_for(
                    g,
                    &known.functions[i],
                    &function_sites[i],
                    seeds,
                    &length_bounds[i],
                ))
            })
            .collect();
        let entry_plan = plan_for(
            entry,
            &known.entry,
            &entry_sites,
            entry_intrinsics,
            &entry_length_bounds,
        );

        // A method call reaches a typed entry by name when the callee the cache remembers is
        // one of the unit's own methods of that name, so every such method a site could enter
        // typed -- the arguments proven numbers, the count exact -- is referenced.
        let method_candidates_by_name = if generic_only {
            HashMap::new()
        } else {
            method_candidates_map(program, publishes_frame)
        };
        let method_targets = |graph: &FunctionGraph, plan: &CallPlan| -> Vec<u32> {
            let mut targets = Vec::new();
            if plan.generic_only {
                return targets;
            }
            for (_, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
                if let GraphOp::CallMethod { name, args, .. } = op
                    && let Some(Constant::String(key)) = program.constants.get(*name as usize)
                    && args.iter().all(|a| plan.numbers.contains(a))
                {
                    if let Some(candidates) = method_candidates_by_name.get(key) {
                        targets.extend(
                            candidates
                                .iter()
                                .copied()
                                .filter(|&i| params[i as usize] as usize == args.len()),
                        );
                    }
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
            if plan.generic_only {
                return targets;
            }
            for (_, op) in graph.blocks.iter().flat_map(|b| b.operations.iter()) {
                if let GraphOp::CallMethod { name, args, .. } = op
                    && let Some(Constant::String(key)) = program.constants.get(*name as usize)
                {
                    // A family site (parallel issues 10 and 15) calls the typed entries of its
                    // callbacks twice over: the pooled lowering's clones hang off them, and when
                    // the pool declines the site runs the sequential method's own loop --
                    // `map`'s, `filter`'s, or `parallelReduce`'s tree -- which enters them like
                    // any `map` site does. `parallelFlatMap` has only the pooled lowering (its
                    // callback must be parallel-safe to be referenced), and a `parallelSort`
                    // comparator is never called there, only recognised.
                    if let Some(method) = key
                        .to_scalar_string()
                        .ok()
                        .and_then(|k| crate::parallel::ParallelMethod::from_name(&k))
                    {
                        let sequential: &[(usize, &[u32])] = match (method, args.len()) {
                            (
                                crate::parallel::ParallelMethod::Map
                                | crate::parallel::ParallelMethod::Filter,
                                1 | 2,
                            ) => &[(0, &[1, 2])],
                            (crate::parallel::ParallelMethod::Reduce, 3) => {
                                &[(0, &[2, 3]), (1, &[2])]
                            }
                            (crate::parallel::ParallelMethod::FlatMap, _) => {
                                method.callbacks(args.len())
                            }
                            _ => &[],
                        };
                        for &(position, arities) in sequential {
                            if let Some(&target) = plan.known.get(&args[position])
                                && arities.contains(&params[target as usize])
                                && (method != crate::parallel::ParallelMethod::FlatMap
                                    || parallel_safe[target as usize])
                            {
                                targets.push(target);
                            }
                        }
                        continue;
                    }
                    let arities: &[u32] = match (key.to_scalar_string().ok().as_deref(), args.len())
                    {
                        (Some("map"), 1 | 2) | (Some("filter"), 1 | 2) => &[1, 2],
                        (Some("reduce"), 2) => &[2],
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
        let typed_entries =
            std::sync::Arc::new(typed.iter().map(Option::is_some).collect::<Vec<_>>());

        let mut plan = DirectPlan {
            functions,
            typed,
            parallel,
            entry: entry_plan,
            generic_loop_calls: None,
        };
        for plan in plan
            .functions
            .iter_mut()
            .chain(plan.typed.iter_mut().flatten())
            .chain(plan.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut plan.entry))
        {
            plan.typed_entries = typed_entries.clone();
        }
        plan
    }

    /// The parallel lowering of a generic-only unit (issue 13): the `parallelMap` sites whose
    /// callback is proven to be one proto, that proto's verdict, and -- where it qualifies -- its
    /// typed entry and parallel clone. Nothing else in the unit is planned: every body stays
    /// generic, and the only facts added to a generic body's plan are the callback values of its
    /// own `parallelMap` sites.
    ///
    /// A generic-only unit is one `kiln-native` judged too large to specialize (or a size
    /// profile), and the budget that judgement protects is emission and C compile time across
    /// the whole unit. This spends it only where a site exists: a unit without a `parallelMap`
    /// call pays one scan of its operations, and one with sites pays the binding walk
    /// (`known_callees`) and one settle per callback -- not the whole-unit fixpoint, the
    /// per-body settles, the method candidates, or any clone of a body that is not a callback.
    ///
    /// The verdict is the full plan's, step for step (`plan` above): the source rules, a typed
    /// entry the full plan would give it, and a number returned with the parameters and captured
    /// reads seeded as numbers. A qualifying callback calls nothing but listed `Math`
    /// functions, so its proof does not depend on any other proto's typed return.
    fn plan_parallel_sites(
        &mut self,
        program: &Program,
        graphs: &[FunctionGraph],
        entry: &FunctionGraph,
        publishes_frame: &[bool],
        speculate: Option<bool>,
    ) {
        use crate::parallel::{Reason, Refusal};
        let parallel_method = |name: u32| match program.constants.get(name as usize) {
            Some(Constant::String(key)) => key
                .to_scalar_string()
                .ok()
                .and_then(|k| crate::parallel::ParallelMethod::from_name(&k)),
            _ => None,
        };
        // Every callback argument of a parallel site (`parallelMap` and the family): the
        // positions `ParallelMethod::callbacks` names for the call's argument count.
        let site_arguments = |graph: &FunctionGraph| -> Vec<ValueId> {
            graph
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .flat_map(|(_, op)| match op {
                    GraphOp::CallMethod { name, args, .. } => parallel_method(*name)
                        .map(|method| {
                            method
                                .callbacks(args.len())
                                .iter()
                                .map(|&(position, _)| args[position])
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                })
                .collect()
        };
        let function_arguments: Vec<Vec<ValueId>> = graphs.iter().map(site_arguments).collect();
        let entry_arguments = site_arguments(entry);
        if entry_arguments.is_empty() && function_arguments.iter().all(Vec::is_empty) {
            return;
        }
        let known = known_callees(program, graphs, entry);
        // A generic body calls a value in `known` by name (`GraphOp::Call`'s direct arm), so a
        // callback value that is also called directly in the same body is left out: the site
        // then says `callback-unknown`, and the call stays generic.
        let site_known = |graph: &FunctionGraph,
                          arguments: &[ValueId],
                          known: &KnownValues|
         -> HashMap<ValueId, u32> {
            let called: HashSet<ValueId> = graph
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .filter_map(|(_, op)| match op {
                    GraphOp::Call { callee, .. } => Some(*callee),
                    _ => None,
                })
                .collect();
            arguments
                .iter()
                .filter(|a| !called.contains(a))
                .filter_map(|a| known.known.get(a).map(|&t| (*a, t)))
                .collect()
        };
        let function_known: Vec<HashMap<ValueId, u32>> = graphs
            .iter()
            .enumerate()
            .map(|(i, g)| site_known(g, &function_arguments[i], &known.functions[i]))
            .collect();
        let entry_known = site_known(entry, &entry_arguments, &known.entry);
        let targets: std::collections::BTreeSet<u32> = function_known
            .iter()
            .chain(std::iter::once(&entry_known))
            .flat_map(|k| k.values().copied())
            .collect();
        if targets.is_empty() {
            return;
        }

        let count = graphs.len();
        let params: Vec<u32> = program
            .functions
            .iter()
            .map(|p| p.parameter_count)
            .collect();
        let has_open_world = program.has_direct_eval() || program.has_dynamic_import();
        // The full plan's typed eligibility, asked of the callbacks only.
        let mut typed_eligible = vec![false; count];
        for &t in &targets {
            let proto = &program.functions[t as usize];
            typed_eligible[t as usize] = direct_callable(proto, publishes_frame[t as usize])
                && !proto
                    .code
                    .iter()
                    .any(|instruction| matches!(instruction, Instruction::RestArguments(_)))
                && proto.code.len() <= crate::MAX_SPECIALIZED_INSTRUCTIONS;
        }
        let direct_targets: Vec<bool> = program
            .functions
            .iter()
            .zip(publishes_frame)
            .map(|(proto, &publishes)| direct_callable(proto, publishes))
            .collect();
        let returns_number = |g: &FunctionGraph, proven: &HashMap<ValueId, Proven>| {
            g.blocks
                .iter()
                .filter(|b| b.is_reachable_from_entry(g.entry_block))
                .all(|b| match &b.terminator {
                    Terminator::Return(Some(v)) => proven.get(v).is_some_and(|k| k.is_number()),
                    Terminator::Return(None) => false,
                    _ => true,
                })
        };
        struct Callback {
            sites: Vec<Site>,
            intrinsics: HashMap<ValueId, Proven>,
            length_bounds: HashSet<ValueId>,
        }
        let callback_of = |i: usize| -> Callback {
            let g = &graphs[i];
            let mut sites = Vec::new();
            for block in &g.blocks {
                for (vid, op) in &block.operations {
                    if let GraphOp::Call { callee, args, .. } = op
                        && let Some(&target) = known.functions[i].known.get(callee)
                        && direct_targets[target as usize]
                    {
                        sites.push(Site {
                            result: *vid,
                            target,
                            args: args.clone(),
                        });
                    }
                }
            }
            let length_bounds = g
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .filter_map(|(v, op)| match op {
                    GraphOp::GetProperty { name, .. } => {
                        match program.constants.get(*name as usize) {
                            Some(Constant::String(s)) if s == "length" => Some(*v),
                            _ => None,
                        }
                    }
                    _ => None,
                })
                .collect();
            Callback {
                sites,
                intrinsics: if has_open_world {
                    HashMap::new()
                } else {
                    intrinsic_results(g, program)
                },
                length_bounds,
            }
        };
        // Typed returns, as the full plan's fixpoint would find them for these protos: a
        // callback's settle reads another proto's typed return only through a direct call, which
        // a qualifying callback does not make, so a site that would need one is simply untyped.
        let callbacks: HashMap<u32, Callback> = targets
            .iter()
            .filter(|&&t| typed_eligible[t as usize])
            .map(|&t| (t, callback_of(t as usize)))
            .collect();
        let no_returns = vec![false; count];
        let typed_seeds = |i: usize, cb: &Callback| -> HashMap<ValueId, Proven> {
            let g = &graphs[i];
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
            seeds.extend(cb.intrinsics.iter().map(|(&v, &k)| (v, k)));
            seeds
        };
        let mut typed_returns = vec![false; count];
        for (&t, cb) in &callbacks {
            let i = t as usize;
            let (proven, _) = settle(
                &graphs[i],
                typed_seeds(i, cb),
                &cb.sites,
                &typed_eligible,
                &no_returns,
                &params,
                &cb.length_bounds,
            );
            typed_returns[i] = returns_number(&graphs[i], &proven);
        }

        let mut verdicts = (*self.entry.parallel).clone();
        let mut parallel_seeds: HashMap<u32, HashMap<ValueId, Proven>> = HashMap::new();
        for &t in &targets {
            let i = t as usize;
            verdicts[i] = (|| {
                // A callback tier 1 refuses may still read the heap in parallel (tier 2).
                let tier_2 = |refusal| {
                    let lookup = |f: u32, v: ValueId| known.parallel_callee(graphs, f, v);
                    let calls = crate::parallel::CallGraph {
                        graphs,
                        captures: &known.captures,
                        known: &lookup,
                        root: i as u32,
                    };
                    crate::parallel::tier_2(
                        &graphs[i],
                        &program.functions[i],
                        program,
                        &known.captures[i],
                        refusal,
                        Some(&calls),
                    )
                };
                let shape = match crate::parallel::parallel_verdict(
                    &graphs[i],
                    &program.functions[i],
                    program,
                    &known.captures[i],
                ) {
                    Ok(shape) => shape,
                    Err(refusal) => return tier_2(refusal),
                };
                let Some(cb) = callbacks.get(&t) else {
                    return tier_2(Refusal::new(Reason::NotTyped));
                };
                let mut seeds: HashMap<ValueId, Proven> = graphs[i]
                    .initial_locals
                    .iter()
                    .take(params[i] as usize)
                    .map(|&v| (v, Proven::Number))
                    .collect();
                seeds.extend(shape.captured_values.iter().map(|&v| (v, Proven::Number)));
                seeds.extend(cb.intrinsics.iter().map(|(&v, &k)| (v, k)));
                let (proven, _) = settle(
                    &graphs[i],
                    seeds.clone(),
                    &cb.sites,
                    &typed_eligible,
                    &typed_returns,
                    &params,
                    &cb.length_bounds,
                );
                let Some(boolean_result) = crate::parallel::parallel_result(&graphs[i], &proven)
                else {
                    return Err(Refusal::new(Reason::ReturnsNonNumber));
                };
                parallel_seeds.insert(t, seeds);
                Ok(crate::parallel::ParallelShape {
                    boolean_result,
                    ..shape
                })
            })();
        }
        // A tier-2 verdict has no typed clone: `parallel_safe` is tier 1's.
        let parallel_safe: Vec<bool> = verdicts
            .iter()
            .map(|v| matches!(v, Ok(shape) if shape.heap.is_none()))
            .collect();
        let verdicts = std::sync::Arc::new(verdicts);

        // The callback's own plans: its typed entry (which the site's typed-callback check and
        // the clone's references hang off) and its parallel clone, specialized like the full
        // plan's. Only a qualifying callback gets them; any other runs its generic body.
        let base = self.entry.clone();
        let specialized = |i: usize, seeds: HashMap<ValueId, Proven>| -> CallPlan {
            let cb = &callbacks[&(i as u32)];
            let (proven, typed_sites) = settle(
                &graphs[i],
                seeds,
                &cb.sites,
                &typed_eligible,
                &typed_returns,
                &params,
                &cb.length_bounds,
            );
            CallPlan {
                generic_only: false,
                speculate,
                known: known.functions[i].known.clone(),
                typed_sites,
                numbers: proven
                    .iter()
                    .filter_map(|(&v, &kind)| kind.is_number().then_some(v))
                    .collect(),
                ints: proven
                    .iter()
                    .filter_map(|(&v, &kind)| (kind == Proven::Int).then_some(v))
                    .collect(),
                unchecked: known.functions[i].unchecked.clone(),
                invariant: known.functions[i].invariant.clone(),
                ..base.clone()
            }
        };
        for &t in &targets {
            let i = t as usize;
            let Some(seeds) = parallel_seeds.remove(&t) else {
                continue;
            };
            let cb = &callbacks[&t];
            self.typed[i] = Some(specialized(i, typed_seeds(i, cb)));
            self.parallel[i] = Some(specialized(i, seeds));
        }
        let typed_entries =
            std::sync::Arc::new(self.typed.iter().map(Option::is_some).collect::<Vec<_>>());
        let typed_returns = std::sync::Arc::new(typed_returns);
        let parallel_safe = std::sync::Arc::new(parallel_safe);
        for (i, plan) in self.functions.iter_mut().enumerate() {
            plan.known = function_known[i].clone();
        }
        self.entry.known = entry_known;
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.typed_returns = typed_returns.clone();
            plan.typed_entries = typed_entries.clone();
            plan.parallel_safe = parallel_safe.clone();
            plan.parallel = verdicts.clone();
        }
    }

    /// Record what every typed entry needs, in every body's plan.
    /// Record which protos have a fast clone, in every body's plan.
    pub fn set_fast(&mut self, fast: &[bool]) {
        let shared = std::sync::Arc::new(fast.to_vec());
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.fast = shared.clone();
        }
    }

    /// Restrict static dispatch and by-name specialization to bodies emitted in this unit.
    pub fn set_available(&mut self, program: &Program, available: &[bool]) {
        // Liveness settles after the initial plan. Spend the generic compiler budget only on
        // retained bodies: selecting first and masking later lets dead declarations displace
        // executable functions from the bounded tier.
        if self.entry.generic_only {
            let selected = if let Some(profile) = self.generic_loop_calls.as_ref() {
                select_generic_optimization_functions_with_profile(
                    program,
                    &self.entry.publishes_frame,
                    Some(available),
                    profile,
                )
            } else {
                select_generic_optimization_functions_with_weights(
                    program,
                    &self.entry.publishes_frame,
                    Some(available),
                    None,
                )
            };
            for (plan, selected) in self.functions.iter_mut().zip(selected) {
                plan.generic_function_optimized = selected;
            }
            for plan in self
                .functions
                .iter_mut()
                .chain(self.typed.iter_mut().flatten())
                .chain(self.parallel.iter_mut().flatten())
                .chain(std::iter::once(&mut self.entry))
            {
                plan.known
                    .retain(|_, target| available.get(*target as usize).copied().unwrap_or(false));
            }
        }
        let typed_entries = std::sync::Arc::new(
            self.entry
                .typed_entries
                .iter()
                .enumerate()
                .map(|(index, typed)| *typed && available.get(index).copied().unwrap_or(false))
                .collect::<Vec<_>>(),
        );
        let available = std::sync::Arc::new(available.to_vec());
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.available = available.clone();
            plan.typed_entries = typed_entries.clone();
        }
    }

    /// Record every proto's receiver-shape cache width, in every body's plan.
    pub fn set_receiver_columns(&mut self, columns: &[usize]) {
        let shared = std::sync::Arc::new(columns.to_vec());
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.receiver_columns = shared.clone();
        }
    }

    pub fn set_typed_needs(&mut self, needs: &[TypedNeeds]) {
        let shared = std::sync::Arc::new(needs.to_vec());
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.typed_needs = shared.clone();
        }
    }

    pub fn set_typed_guards(&mut self, guards: &[String]) {
        let shared = std::sync::Arc::new(guards.to_vec());
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.typed_guards = shared.clone();
        }
    }

    pub fn set_should_chunk(&mut self, should_chunk: bool) {
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.should_chunk = should_chunk;
        }
    }

    /// The typed entry of `index` may serve as a `parallelMap` chunk worker: it exists, returns a
    /// number, needs nothing but its parameters, and its body passes
    /// [`crate::parallel::parallel_safe`]. Read after `set_typed_needs`, which is what settles
    /// the needs.
    pub fn parallel_chunk(&self, index: u32) -> bool {
        let i = index as usize;
        self.has_typed_entry(index)
            && self.parallel.get(i).is_some_and(Option::is_some)
            && self.entry.parallel_safe.get(i).copied().unwrap_or(false)
    }

    /// Narrow `parallel_safe` to the protos whose parallel clone was emitted and scanned clean
    /// (`lib.rs`), in every body's plan, so a parallel site anywhere reads the final answer: a
    /// site whose callback runs on the pool refuses with `clone-rejected` when its proto is not
    /// `parallel_safe`. The verdict itself is kept -- a `parallelSort` comparator runs nowhere,
    /// so it needs no clone, only the order its verdict proved.
    pub fn set_parallel(&mut self, admitted: &[bool]) {
        let verdicts = self.entry.parallel.clone();
        let safe = std::sync::Arc::new(
            self.entry
                .parallel_safe
                .iter()
                .enumerate()
                .map(|(i, ok)| *ok && admitted.get(i).copied().unwrap_or(false))
                .collect::<Vec<_>>(),
        );
        for plan in self
            .functions
            .iter_mut()
            .chain(self.typed.iter_mut().flatten())
            .chain(self.parallel.iter_mut().flatten())
            .chain(std::iter::once(&mut self.entry))
        {
            plan.parallel_safe = safe.clone();
            plan.parallel = verdicts.clone();
        }
    }

    /// The typed entry of `index` exists in the unit.
    pub fn has_typed_entry(&self, index: u32) -> bool {
        self.typed.get(index as usize).is_some_and(Option::is_some)
    }

    #[cfg(test)]
    pub(crate) fn generic_loop_profile_weights(&self) -> Option<Vec<usize>> {
        self.generic_loop_calls
            .as_ref()
            .map(|profile| profile.weights(None))
    }
}

/// Select bounded, deterministic generic bodies to compile at O1 inside otherwise O0 large
/// modules. This is only a compiler tier: every selected function retains its original boxed ABI
/// and generic emitted body. The original small frame-free bucket remains source ordered; a
/// separate, smaller bucket admits larger loop bodies and the bounded set of known callees they
/// invoke. Proven loop-call context estimates repeated body work, and explicit instruction, edge,
/// and function caps make analysis and object growth predictable without a library-specific
/// allowlist.
pub(crate) fn select_generic_optimization_functions(
    program: &Program,
    publishes_frame: &[bool],
    available: Option<&[bool]>,
) -> Vec<bool> {
    select_generic_optimization_functions_with_weights(program, publishes_frame, available, None)
}

pub(crate) fn select_generic_optimization_functions_with_weights(
    program: &Program,
    publishes_frame: &[bool],
    available: Option<&[bool]>,
    loop_call_weights: Option<&[usize]>,
) -> Vec<bool> {
    select_generic_optimization_functions_internal(
        program,
        publishes_frame,
        available,
        loop_call_weights,
        None,
    )
}

pub(crate) fn select_generic_optimization_functions_with_profile(
    program: &Program,
    publishes_frame: &[bool],
    available: Option<&[bool]>,
    profile: &LoopCallProfile,
) -> Vec<bool> {
    select_generic_optimization_functions_internal(
        program,
        publishes_frame,
        available,
        None,
        Some(profile),
    )
}

fn select_generic_optimization_functions_internal(
    program: &Program,
    publishes_frame: &[bool],
    available: Option<&[bool]>,
    loop_call_weights: Option<&[usize]>,
    loop_profile: Option<&LoopCallProfile>,
) -> Vec<bool> {
    const MAX_FUNCTIONS: usize = 64;
    const MAX_FUNCTION_INSTRUCTIONS: usize = 320;
    const MAX_TOTAL_INSTRUCTIONS: usize = 4096;
    const MAX_LOOP_FUNCTIONS: usize = 2;
    const MAX_LOOP_GROUPS: usize = MAX_GENERIC_LOOP_CALLERS;
    const MAX_LOOP_PROFILE_FUNCTIONS: usize = MAX_GENERIC_LOOP_OPTIMIZED_FUNCTIONS;
    const MAX_LOOP_FUNCTION_INSTRUCTIONS: usize = MAX_GENERIC_LOOP_PROFILE_CALLER_INSTRUCTIONS;
    const MAX_LOOP_TOTAL_INSTRUCTIONS: usize = 2048;
    const MAX_LOOP_PROFILE_TOTAL_INSTRUCTIONS: usize = MAX_GENERIC_LOOP_CALL_INSTRUCTIONS;
    const MAX_LOOP_GROUP_FUNCTIONS: usize = 4;

    let mut selected = vec![false; program.functions.len()];
    let mut count = 0usize;
    let mut instructions = 0usize;
    for (index, proto) in program.functions.iter().enumerate() {
        if available.is_some_and(|available| !available.get(index).copied().unwrap_or(false)) {
            continue;
        }
        let publishes = publishes_frame.get(index).copied().unwrap_or(true);
        let size = proto.code.len();
        if count >= MAX_FUNCTIONS || size == 0 || size > MAX_FUNCTION_INSTRUCTIONS {
            continue;
        }
        if !direct_callable(proto, publishes) {
            continue;
        }
        if instructions.saturating_add(size) > MAX_TOTAL_INSTRUCTIONS {
            continue;
        }
        selected[index] = true;
        count += 1;
        instructions += size;
    }

    // Large bodies, generators and other frame-publishing bodies may dominate repeated work
    // while remaining outside the legacy bucket above. O1 only changes the host compiler's
    // treatment of the existing boxed body; it does not introduce a direct or typed entry. Admit
    // a separately-budgeted set of such loop bodies and proven callees. The module profile is
    // bounded at 512 bodies / 32,768 total instructions before this tier is entered; reuse those
    // bounds here instead of imposing a second eight-body ceiling. Each connected group remains
    // limited to four bodies and each body to 1,536 instructions. With call evidence,
    // rank bounded callee groups by repeated body work (context weight times body size), then
    // weight and body size. Without known edges,
    // preserve the previous span/backedge ranking. Proto index is the final stable tie break.
    if let Some(profile) = loop_profile {
        let loop_weights = profile.weights(available);
        if profile.seed_edges.is_empty() {
            return select_generic_optimization_functions_internal(
                program,
                publishes_frame,
                available,
                Some(&loop_weights),
                None,
            );
        }
        let mut groups = Vec::new();
        for &(caller, target, weight) in &profile.seed_edges {
            let target = target as usize;
            if target >= program.functions.len()
                || weight == 0
                || caller.is_some_and(|caller| {
                    available
                        .is_some_and(|available| !available.get(caller).copied().unwrap_or(false))
                })
                || available
                    .is_some_and(|available| !available.get(target).copied().unwrap_or(false))
            {
                continue;
            }
            let group = profile.loop_group(
                program,
                caller,
                target,
                available,
                &selected,
                &loop_weights,
                MAX_LOOP_GROUP_FUNCTIONS,
                MAX_LOOP_FUNCTION_INSTRUCTIONS,
                MAX_LOOP_PROFILE_TOTAL_INSTRUCTIONS,
            );
            if group.is_empty() {
                continue;
            }
            let instructions = group
                .iter()
                .filter(|index| !selected[**index])
                .map(|index| program.functions[*index].code.len())
                .sum::<usize>();
            let body_work = group
                .iter()
                .map(|index| program.functions[*index].code.len())
                .sum::<usize>();
            groups.push((caller, target, group, instructions, body_work, weight));
        }
        groups.sort_by(|left, right| {
            let left_work = (left.4 as u128) * (left.5 as u128);
            let right_work = (right.4 as u128) * (right.5 as u128);
            right_work
                .cmp(&left_work)
                .then_with(|| right.4.cmp(&left.4))
                .then_with(|| right.5.cmp(&left.5))
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.0.cmp(&right.0))
        });
        // Several call edges from one loop can each describe a plausible group. Give the best
        // group from every distinct loop caller a chance before a caller's additional edges can
        // consume the bounded module budget. The second pass preserves the ordinary score order
        // for any capacity left over.
        let mut first_by_caller = Vec::new();
        let mut deferred_groups = Vec::new();
        let mut seen_callers = Vec::new();
        for group in groups {
            if seen_callers.contains(&group.0) {
                deferred_groups.push(group);
            } else {
                seen_callers.push(group.0);
                first_by_caller.push(group);
            }
        }
        first_by_caller.extend(deferred_groups);
        let mut loop_count = 0usize;
        let mut loop_instructions = 0usize;
        let mut loop_functions = 0usize;
        for (_, _, group, _, _, _) in first_by_caller {
            let new_functions = group.iter().filter(|index| !selected[**index]).count();
            let new_instructions = group
                .iter()
                .filter(|index| !selected[**index])
                .map(|index| program.functions[*index].code.len())
                .sum::<usize>();
            if new_functions == 0
                || loop_count == MAX_LOOP_GROUPS
                || loop_functions.saturating_add(new_functions) > MAX_LOOP_PROFILE_FUNCTIONS
                || loop_instructions.saturating_add(new_instructions)
                    > MAX_LOOP_PROFILE_TOTAL_INSTRUCTIONS
            {
                continue;
            }
            for index in group {
                if !selected[index] {
                    selected[index] = true;
                    loop_functions += 1;
                }
            }
            loop_count += 1;
            loop_instructions += new_instructions;
        }
        if loop_count == 0 {
            return select_generic_optimization_functions_internal(
                program,
                publishes_frame,
                available,
                Some(&loop_weights),
                None,
            );
        }
        return selected;
    }

    let mut loop_candidates = Vec::new();
    for (index, proto) in program.functions.iter().enumerate() {
        if selected[index]
            || available.is_some_and(|available| !available.get(index).copied().unwrap_or(false))
        {
            continue;
        }
        if proto.code.is_empty() || proto.code.len() > MAX_LOOP_FUNCTION_INSTRUCTIONS {
            continue;
        }
        let call_weight = loop_call_weights
            .and_then(|weights| weights.get(index))
            .copied()
            .unwrap_or(0);
        let own_loop = loop_work(proto);
        if own_loop.is_none() && call_weight == 0 {
            continue;
        }
        let (loop_span, backedges) = own_loop.unwrap_or_default();
        loop_candidates.push((index, proto.code.len(), loop_span, backedges));
    }
    loop_candidates.sort_by(|left, right| {
        let left_call_weight = loop_call_weights
            .and_then(|weights| weights.get(left.0))
            .copied()
            .unwrap_or(0);
        let right_call_weight = loop_call_weights
            .and_then(|weights| weights.get(right.0))
            .copied()
            .unwrap_or(0);
        let left_repeated_work = (left_call_weight as u128) * (left.1 as u128);
        let right_repeated_work = (right_call_weight as u128) * (right.1 as u128);
        (right_call_weight > 0)
            .cmp(&(left_call_weight > 0))
            .then_with(|| right_repeated_work.cmp(&left_repeated_work))
            .then_with(|| right_call_weight.cmp(&left_call_weight))
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| right.3.cmp(&left.3))
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut loop_count = 0usize;
    let mut loop_instructions = 0usize;
    for (index, size, _, _) in loop_candidates {
        if loop_count == MAX_LOOP_FUNCTIONS
            || loop_instructions.saturating_add(size) > MAX_LOOP_TOTAL_INSTRUCTIONS
        {
            continue;
        }
        selected[index] = true;
        loop_count += 1;
        loop_instructions += size;
    }
    selected
}

/// A compact ranking profile for the supplemental O1 tier. `seed_edges` includes known direct
/// callees and conservative computed-method candidates whose key provenance matches a definition
/// in this graph; known direct calls inside those bodies propagate weight for a few edges. A
/// computed-key match is only a ranking hint: it does not prove the receiver, property value, or
/// eventual call target, and never changes dispatch. Phase availability is still applied by the
/// selector before a body can be selected.
#[derive(Debug)]
pub(crate) struct LoopCallProfile {
    seed_edges: Vec<(Option<usize>, u32, usize)>,
    edges: Vec<Vec<u32>>,
}

/// A ranking-only identity for a property key whose source is statically traceable within one
/// graph. This is deliberately not used for direct-call or typed-entry decisions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum MethodKeySource {
    String(kiln_ir::StringUnits),
    Binding(Binding),
}

#[derive(Default)]
struct ComputedMethodCandidates {
    strings: HashMap<kiln_ir::StringUnits, Vec<u32>>,
    bindings: HashMap<Binding, Vec<u32>>,
}

impl ComputedMethodCandidates {
    fn get(&self, source: &MethodKeySource) -> Option<&Vec<u32>> {
        match source {
            MethodKeySource::String(key) => self.strings.get(key),
            MethodKeySource::Binding(binding) => self.bindings.get(binding),
        }
    }
}

/// Recover only direct string constants, same-module global reads, and values passed through
/// ToPropertyKey. The latter preserves provenance, not a value guarantee: this analysis only
/// ranks which existing boxed bodies receive O1 and never changes runtime key conversion.
fn method_key_sources(
    graph: &FunctionGraph,
    owner: Owner,
    known: &Known,
) -> HashMap<ValueId, MethodKeySource> {
    let mut sources: HashMap<ValueId, MethodKeySource> = HashMap::new();
    for block in &graph.blocks {
        for (value, op) in &block.operations {
            let source = match op {
                GraphOp::Const {
                    constant: Constant::String(value),
                    ..
                } => Some(MethodKeySource::String(value.clone())),
                GraphOp::LoadGlobal(slot) => {
                    Some(MethodKeySource::Binding(binding_for_global(*slot)))
                }
                GraphOp::LoadCell(slot) => {
                    Some(MethodKeySource::Binding(Binding { owner, slot: *slot }))
                }
                GraphOp::LoadUpvalue(index) => match owner {
                    Owner::Function(function) => known
                        .capture_bindings
                        .get(function as usize)
                        .and_then(|bindings| bindings.get(index))
                        .copied()
                        .map(MethodKeySource::Binding),
                    Owner::Entry => None,
                },
                GraphOp::Bridge {
                    instruction: Instruction::ToPropertyKey,
                    operands,
                    ..
                } => operands.get(1).and_then(|key| sources.get(key)).cloned(),
                _ => None,
            };
            if let Some(source) = source {
                sources.insert(*value, source);
            }
        }
    }
    sources
}

/// Methods defined in this module, indexed by the provenance of their key. Multiple definitions
/// remain candidates because O1 selection must never decide which implementation a dynamic
/// receiver dispatches to.
fn computed_method_candidates(
    graphs: &[FunctionGraph],
    entry: &FunctionGraph,
    known: &Known,
) -> Option<ComputedMethodCandidates> {
    // This pass is part of the same bounded analysis as loop-call propagation. Do not let
    // candidate indexing scan an oversized graph or retain an unbounded number of target edges.
    if graphs.len() > MAX_GENERIC_LOOP_CALL_FUNCTIONS {
        return None;
    }
    std::iter::once(entry)
        .chain(graphs.iter())
        .flat_map(|graph| graph.blocks.iter())
        .try_fold(0usize, |count, block| {
            let count = count.checked_add(block.operations.len())?;
            (count <= MAX_GENERIC_LOOP_CALL_INSTRUCTIONS).then_some(count)
        })?;
    let mut candidate_edges = 0usize;
    let mut candidates: HashMap<MethodKeySource, HashSet<u32>> = HashMap::new();
    for (owner, graph) in std::iter::once((Owner::Entry, entry)).chain(
        graphs
            .iter()
            .enumerate()
            .map(|(index, graph)| (Owner::Function(index as u32), graph)),
    ) {
        let sources = method_key_sources(graph, owner, known);
        let functions: HashMap<ValueId, u32> = graph
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter_map(|(value, op)| match op {
                GraphOp::MakeFunction { index, .. } => Some((*value, *index)),
                _ => None,
            })
            .collect();
        for block in &graph.blocks {
            for (_, op) in &block.operations {
                let GraphOp::Bridge {
                    instruction: Instruction::DefineMethod | Instruction::DefineNamedProperty { .. },
                    operands,
                    ..
                } = op
                else {
                    continue;
                };
                let (Some(key), Some(function)) = (operands.get(1), operands.get(2)) else {
                    continue;
                };
                let (Some(source), Some(&target)) = (sources.get(key), functions.get(function))
                else {
                    continue;
                };
                let targets = candidates.entry(source.clone()).or_default();
                if targets.insert(target) {
                    candidate_edges = candidate_edges.saturating_add(1);
                    if candidate_edges > MAX_GENERIC_LOOP_CALL_EDGES {
                        return None;
                    }
                }
            }
        }
    }
    let mut indexed = ComputedMethodCandidates::default();
    for (key, targets) in candidates {
        let mut targets: Vec<_> = targets.into_iter().collect();
        targets.sort_unstable();
        match key {
            MethodKeySource::String(key) => {
                indexed.strings.insert(key, targets);
            }
            MethodKeySource::Binding(binding) => {
                indexed.bindings.insert(binding, targets);
            }
        }
    }
    Some(indexed)
}

/// Build the bounded loop-callee ranking facts from an unmodified module program. Cycle-module
/// emission reuses these facts for its declaration-instantiation and evaluation entrypoints,
/// whose entry bytecode is intentionally truncated or nop-filled. The facts only rank boxed O1
/// bodies; phase-specific direct-call and liveness decisions continue to come from each unit.
pub(crate) fn generic_loop_profile_for_program(program: &Program) -> Option<Arc<LoopCallProfile>> {
    if program.functions.len() > MAX_GENERIC_LOOP_CALL_FUNCTIONS
        || program
            .functions
            .iter()
            .map(|proto| proto.code.len())
            .chain(std::iter::once(program.code.len()))
            .fold(0usize, usize::saturating_add)
            > MAX_GENERIC_LOOP_CALL_INSTRUCTIONS
        || loop_caller_count(program) > MAX_GENERIC_LOOP_CALLERS
    {
        return None;
    }
    let graphs = program
        .functions
        .iter()
        .map(|proto| crate::graph::build_function_graph(proto, program).ok())
        .collect::<Option<Vec<_>>>()?;
    let body = crate::entry_proto(program);
    let entry = crate::graph::build_function_graph(&body, program).ok()?;
    let known = known_callees(program, &graphs, &entry);
    LoopCallProfile::from_known(program, &graphs, &entry, &known).map(Arc::new)
}

impl LoopCallProfile {
    pub(crate) fn loop_group(
        &self,
        program: &Program,
        loop_caller: Option<usize>,
        root: usize,
        available: Option<&[bool]>,
        selected: &[bool],
        loop_weights: &[usize],
        max_functions: usize,
        max_function_instructions: usize,
        max_total_instructions: usize,
    ) -> Vec<usize> {
        let Some(root_proto) = program.functions.get(root) else {
            return Vec::new();
        };
        let root_size = root_proto.code.len();
        if root_size == 0
            || root_size > max_function_instructions
            || available.is_some_and(|available| !available.get(root).copied().unwrap_or(false))
        {
            return Vec::new();
        }

        // Keep traversal state separate from group membership. A loop caller can already be
        // selected by another group (or be too large to admit here) and still lead to useful
        // known callees through a cycle in this group's graph.
        let mut traversed = vec![false; program.functions.len()];
        let mut included = vec![false; program.functions.len()];
        let mut members = Vec::new();
        let mut instructions = 0usize;

        // A seed edge originates in a loop in `loop_caller`. Keep that repeated-work driver
        // with its known callee closure when it fits the same bounded group. Ranking-only O1
        // selection changes no call targets or runtime semantics; an unavailable, oversized or
        // already-selected caller does not consume this group's budget.
        if let Some(caller) = loop_caller
            && caller < program.functions.len()
            && !selected[caller]
            && !available.is_some_and(|available| !available.get(caller).copied().unwrap_or(false))
        {
            let proto = &program.functions[caller];
            let size = proto.code.len();
            let root_needs_admission = !selected[root] && caller != root;
            let required_functions = 1 + usize::from(root_needs_admission);
            let required_instructions =
                size.saturating_add(if root_needs_admission { root_size } else { 0 });
            if loop_work(proto).is_some()
                && size > 0
                && size <= max_function_instructions
                && members.len().saturating_add(required_functions) <= max_functions
                && instructions.saturating_add(required_instructions) <= max_total_instructions
            {
                members.push(caller);
                included[caller] = true;
                instructions = instructions.saturating_add(size);
            }
        }

        let mut frontier = vec![root];
        traversed[root] = true;
        if !selected[root] && !included[root] {
            if members.len() >= max_functions
                || instructions.saturating_add(root_size) > max_total_instructions
            {
                return Vec::new();
            }
            members.push(root);
            included[root] = true;
            instructions = instructions.saturating_add(root_size);
        }
        for _ in 0..MAX_GENERIC_LOOP_CALL_DEPTH {
            let mut next = Vec::new();
            for caller in frontier {
                let mut candidates = self.edges[caller]
                    .iter()
                    .map(|target| *target as usize)
                    .filter(|target| {
                        *target < program.functions.len()
                            && !traversed[*target]
                            && !program.functions[*target].code.is_empty()
                            && program.functions[*target].code.len() <= max_function_instructions
                            && !available.is_some_and(|available| {
                                !available.get(*target).copied().unwrap_or(false)
                            })
                    })
                    .collect::<Vec<_>>();
                candidates.sort_unstable_by(|left, right| {
                    let left_weight = loop_weights.get(*left).copied().unwrap_or(0);
                    let right_weight = loop_weights.get(*right).copied().unwrap_or(0);
                    let left_work = (left_weight as u128)
                        .saturating_mul(program.functions[*left].code.len() as u128);
                    let right_work = (right_weight as u128)
                        .saturating_mul(program.functions[*right].code.len() as u128);
                    right_work
                        .cmp(&left_work)
                        .then_with(|| right_weight.cmp(&left_weight))
                        .then_with(|| left.cmp(right))
                });
                candidates.dedup();
                for target in candidates {
                    traversed[target] = true;
                    let size = program.functions[target].code.len();
                    if !selected[target] && !included[target] {
                        if members.len() < max_functions
                            && instructions.saturating_add(size) <= max_total_instructions
                        {
                            members.push(target);
                            included[target] = true;
                            instructions = instructions.saturating_add(size);
                        }
                    }
                    next.push(target);
                }
            }
            if next.is_empty() || members.len() >= max_functions {
                break;
            }
            frontier = next;
        }
        members
    }

    fn from_known(
        program: &Program,
        graphs: &[FunctionGraph],
        entry: &FunctionGraph,
        known: &Known,
    ) -> Option<Self> {
        if graphs.len() > MAX_GENERIC_LOOP_CALL_FUNCTIONS
            || program
                .functions
                .iter()
                .map(|proto| proto.code.len())
                .chain(std::iter::once(program.code.len()))
                .fold(0usize, usize::saturating_add)
                > MAX_GENERIC_LOOP_CALL_INSTRUCTIONS
            || loop_caller_count(program) > MAX_GENERIC_LOOP_CALLERS
        {
            return None;
        }

        let mut seed_edges = Vec::new();
        let mut edges = vec![Vec::new(); graphs.len()];
        let mut edge_count = 0usize;
        // Computed keys add optional ranking hints. An oversized candidate table must never
        // discard the pre-existing known-call profile, so collection failure means no hints.
        let method_candidates = computed_method_candidates(graphs, entry, known);
        let mut method_seed_edges = Vec::new();
        let mut method_edges = vec![Vec::new(); graphs.len()];
        let mut method_edge_count = 0usize;
        let mut method_edge_overflow = false;
        let mut add_owner = |graph: &FunctionGraph,
                             code: &[Instruction],
                             known_values: &KnownValues,
                             caller: Option<usize>| {
            let contexts = loop_context_weights(code);
            let owner = caller.map_or(Owner::Entry, |index| Owner::Function(index as u32));
            let key_sources = method_candidates
                .as_ref()
                .map(|_| method_key_sources(graph, owner, known))
                .unwrap_or_default();
            for block in &graph.blocks {
                for (result, op) in &block.operations {
                    // Each candidate here is only evidence that its existing boxed body might
                    // serve a dynamic site. No call target is changed, and key provenance is
                    // allowed to be conservative because it controls ranking only.
                    let mut direct_target = None;
                    match op {
                        GraphOp::Call { callee, .. } => {
                            direct_target = known_values.known.get(callee).copied();
                        }
                        _ => {}
                    };
                    if let Some(target) = direct_target {
                        edge_count = edge_count.saturating_add(1);
                        if edge_count > MAX_GENERIC_LOOP_CALL_EDGES {
                            return false;
                        }
                        if let Some(caller) = caller {
                            edges[caller].push(target);
                        }
                        if let Some(pc) = graph.origins.get(result).map(|origin| origin.pc as usize)
                            && let Some(weight) = contexts.get(pc)
                            && *weight > 0
                        {
                            seed_edges.push((caller, target, *weight));
                        }
                    }
                    let method_targets = match op {
                        GraphOp::CallMethod { name, .. } => {
                            match program.constants.get(*name as usize) {
                                Some(Constant::String(key)) => method_candidates
                                    .as_ref()
                                    .and_then(|candidates| candidates.strings.get(key)),
                                _ => None,
                            }
                        }
                        GraphOp::Bridge {
                            instruction: Instruction::CallComputed { .. },
                            operands,
                            ..
                        } => operands
                            .get(1)
                            .and_then(|key| key_sources.get(key))
                            .and_then(|source| {
                                method_candidates
                                    .as_ref()
                                    .and_then(|candidates| candidates.get(source))
                            }),
                        _ => None,
                    };
                    if !method_edge_overflow && let Some(targets) = method_targets {
                        for &target in targets {
                            if method_edge_count >= MAX_GENERIC_LOOP_CALL_EDGES {
                                method_edge_overflow = true;
                                break;
                            }
                            method_edge_count += 1;
                            if let Some(caller) = caller {
                                method_edges[caller].push(target);
                            }
                            if let Some(pc) =
                                graph.origins.get(result).map(|origin| origin.pc as usize)
                                && let Some(weight) = contexts.get(pc)
                                && *weight > 0
                            {
                                method_seed_edges.push((caller, target, *weight));
                            }
                        }
                    }
                }
            }
            true
        };

        for (index, graph) in graphs.iter().enumerate() {
            if !add_owner(
                graph,
                &program.functions[index].code,
                &known.functions[index],
                Some(index),
            ) {
                return None;
            }
        }
        if !add_owner(entry, &program.code, &known.entry, None) {
            return None;
        }
        // Keep the old known-call profile intact if the optional method hints would exceed
        // the same edge budget. Do not partially bias selection from a truncated candidate set.
        if !method_edge_overflow
            && method_edge_count <= MAX_GENERIC_LOOP_CALL_EDGES.saturating_sub(edge_count)
        {
            seed_edges.extend(method_seed_edges);
            for (edges, methods) in edges.iter_mut().zip(method_edges) {
                edges.extend(methods);
            }
        }
        Some(Self { seed_edges, edges })
    }

    pub(crate) fn weights(&self, available: Option<&[bool]>) -> Vec<usize> {
        let mut weights = vec![0usize; self.edges.len()];
        for &(caller, target, weight) in &self.seed_edges {
            let target = target as usize;
            let caller_available = caller.is_none_or(|caller| {
                available.is_none_or(|available| available.get(caller).copied().unwrap_or(false))
            });
            let target_available =
                available.is_none_or(|available| available.get(target).copied().unwrap_or(false));
            if caller_available && target_available {
                weights[target] = weights[target].saturating_add(weight);
            }
        }
        let mut propagated = vec![vec![0usize; MAX_GENERIC_LOOP_CALL_DEPTH + 1]; weights.len()];
        let mut queue = std::collections::VecDeque::new();
        for (index, &weight) in weights.iter().enumerate() {
            if weight > 0 {
                propagated[index][0] = weight;
                queue.push_back((index, weight, 0usize));
            }
        }
        while let Some((caller, weight, depth)) = queue.pop_front() {
            if depth == MAX_GENERIC_LOOP_CALL_DEPTH {
                continue;
            }
            if available.is_some_and(|available| !available.get(caller).copied().unwrap_or(false)) {
                continue;
            }
            for &target in &self.edges[caller] {
                let target = target as usize;
                if available
                    .is_some_and(|available| !available.get(target).copied().unwrap_or(false))
                {
                    continue;
                }
                let next_depth = depth + 1;
                if weight > propagated[target][next_depth] {
                    propagated[target][next_depth] = weight;
                    weights[target] = weights[target].max(weight);
                    queue.push_back((target, weight, next_depth));
                }
            }
        }
        weights
    }
}

/// Whether the bounded supplemental profile can run on these inputs. This lets `known_callees`
/// avoid building ranking-only capture identities for modules that the profile will reject.
fn loop_profile_inputs_within_limits(
    program: &Program,
    graphs: &[FunctionGraph],
    entry: &FunctionGraph,
) -> bool {
    if graphs.len() > MAX_GENERIC_LOOP_CALL_FUNCTIONS
        || loop_caller_count(program) > MAX_GENERIC_LOOP_CALLERS
    {
        return false;
    }
    std::iter::once(entry)
        .chain(graphs.iter())
        .flat_map(|graph| graph.blocks.iter())
        .try_fold(0usize, |count, block| {
            let count = count.checked_add(block.operations.len())?;
            (count <= MAX_GENERIC_LOOP_CALL_INSTRUCTIONS).then_some(count)
        })
        .is_some()
}

#[cfg(test)]
mod loop_call_profile_tests {
    use super::{Known, KnownValues, LoopCallProfile};
    use crate::graph::{
        BasicBlock, BlockId, FunctionGraph, GraphOp, Origin, SiteId, Terminator, ValueId,
    };
    use std::collections::HashMap;

    fn proto(code: Vec<kiln_ir::Instruction>, is_method: bool) -> kiln_ir::FunctionProto {
        kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method,
            is_class: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code,
            upvalues: Vec::new(),
            type_annotations: None,
        }
    }

    fn graph(operations: Vec<(ValueId, GraphOp)>, origins: &[(u32, u32)]) -> FunctionGraph {
        let mut graph = FunctionGraph::new(None, 0, 0);
        graph.next_value_id = operations
            .iter()
            .map(|(value, _)| value.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        graph.blocks.push(BasicBlock {
            id: BlockId(0),
            operations,
            terminator: Terminator::Return(None),
            predecessors: Vec::new(),
            successors: Vec::new(),
            exception_handler: None,
        });
        for &(value, pc) in origins {
            graph.origins.insert(
                ValueId(value),
                Origin {
                    pc,
                    label: "fixture".into(),
                },
            );
        }
        graph
    }

    #[test]
    fn looped_computed_call_ranks_same_import_binding_method_without_devirtualizing() {
        use kiln_ir::Instruction;

        // Both the computed method definition and computed call use the same module-global slot
        // and pass its value through ToPropertyKey.
        let program = kiln_ir::Program {
            functions: vec![
                proto(vec![Instruction::Return], true),
                proto(vec![Instruction::Nop, Instruction::Jump(0)], false),
            ],
            ..Default::default()
        };
        let target_graph = FunctionGraph::new(None, 0, 0);
        let caller_graph = graph(
            vec![
                (ValueId(0), GraphOp::LoadGlobal(7)),
                (
                    ValueId(1),
                    GraphOp::Bridge {
                        instruction: Instruction::ToPropertyKey,
                        operands: vec![ValueId(8), ValueId(0)],
                        pushes: 2,
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::Bridge {
                        instruction: Instruction::CallComputed { argc: 0 },
                        operands: vec![ValueId(9), ValueId(1), ValueId(10)],
                        pushes: 1,
                    },
                ),
            ],
            &[(2, 0)],
        );
        let entry = graph(
            vec![
                (ValueId(0), GraphOp::LoadGlobal(7)),
                (
                    ValueId(1),
                    GraphOp::MakeFunction {
                        index: 0,
                        site: SiteId(0),
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::Bridge {
                        instruction: Instruction::DefineMethod,
                        operands: vec![ValueId(8), ValueId(0), ValueId(1)],
                        pushes: 1,
                    },
                ),
            ],
            &[],
        );
        let graphs = vec![target_graph, caller_graph];
        let known = Known {
            functions: vec![KnownValues::default(); 2],
            entry: KnownValues::default(),
            captures: vec![Vec::new(); 2],
            capture_bindings: vec![HashMap::new(); 2],
            captured_closures: vec![Default::default(); 2],
        };
        let profile = LoopCallProfile::from_known(&program, &graphs, &entry, &known).unwrap();
        assert!(
            profile
                .seed_edges
                .iter()
                .any(|edge| edge.0 == Some(1) && edge.1 == 0)
        );

        // The target publishes a frame, so it is excluded from the old direct-callable bucket.
        // The ranking-only group can still optimize its existing boxed body when both caller and
        // target are in the phase's emitted set; it never makes the computed call direct.
        let selected = super::select_generic_optimization_functions_with_profile(
            &program,
            &[true, false],
            Some(&[true, true]),
            &profile,
        );
        assert!(
            selected[0],
            "frame-publishing boxed method should enter O1 tier"
        );
        assert!(
            selected[1],
            "the loop driver remains selected under existing caps"
        );

        let unavailable = super::select_generic_optimization_functions_with_profile(
            &program,
            &[true, false],
            Some(&[false, true]),
            &profile,
        );
        assert!(
            !unavailable[0],
            "phase-unavailable method cannot be selected"
        );
    }

    #[test]
    fn captured_computed_key_uses_resolved_cell_identity_across_upvalue_indices() {
        use kiln_ir::{Instruction, Upvalue};

        let program = kiln_ir::Program {
            functions: vec![
                proto(vec![Instruction::Return], true),
                proto(vec![Instruction::Return], false),
                proto(vec![Instruction::Nop, Instruction::Jump(0)], true),
            ],
            ..Default::default()
        };
        // fn 1 defines fn 0 under its upvalue 1; fn 2 calls a computed member through
        // upvalue 4. Both descriptors resolve to the same entry binding slot 40, matching the
        // cached Effect module's different-index capture chain. The runtime remains dynamic.
        let mut program = program;
        program.functions[1].upvalues = vec![Upvalue::ParentLocal(24), Upvalue::ParentLocal(40)];
        program.functions[2].upvalues = vec![
            Upvalue::ParentLocal(10),
            Upvalue::ParentLocal(11),
            Upvalue::ParentLocal(12),
            Upvalue::ParentLocal(13),
            Upvalue::ParentLocal(40),
        ];
        let target = FunctionGraph::new(None, 0, 0);
        let definition = graph(
            vec![
                (ValueId(0), GraphOp::LoadUpvalue(1)),
                (
                    ValueId(1),
                    GraphOp::Bridge {
                        instruction: Instruction::ToPropertyKey,
                        operands: vec![ValueId(8), ValueId(0)],
                        pushes: 2,
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::MakeFunction {
                        index: 0,
                        site: SiteId(0),
                    },
                ),
                (
                    ValueId(3),
                    GraphOp::Bridge {
                        instruction: Instruction::DefineNamedProperty {
                            name_function: true,
                        },
                        operands: vec![ValueId(9), ValueId(1), ValueId(2)],
                        pushes: 1,
                    },
                ),
            ],
            &[],
        );
        let caller = graph(
            vec![
                (ValueId(0), GraphOp::LoadUpvalue(4)),
                (
                    ValueId(1),
                    GraphOp::Bridge {
                        instruction: Instruction::ToPropertyKey,
                        operands: vec![ValueId(8), ValueId(0)],
                        pushes: 2,
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::Bridge {
                        instruction: Instruction::CallComputed { argc: 0 },
                        operands: vec![ValueId(9), ValueId(1), ValueId(10)],
                        pushes: 1,
                    },
                ),
            ],
            &[(2, 0)],
        );
        let entry = graph(
            vec![
                (
                    ValueId(0),
                    GraphOp::MakeFunction {
                        index: 1,
                        site: SiteId(1),
                    },
                ),
                (
                    ValueId(1),
                    GraphOp::MakeFunction {
                        index: 2,
                        site: SiteId(2),
                    },
                ),
            ],
            &[],
        );
        let mut graphs = vec![target, definition, caller];
        // The actual Effect module has 739 proto indices and 20,004 instructions in its
        // generated metadata. Its instruction and edge budgets fit this analysis; a 512-proto
        // admission ceiling alone was preventing the computed-key profile from running.
        for _ in 3..739 {
            program
                .functions
                .push(proto(vec![Instruction::Return], false));
            graphs.push(FunctionGraph::new(None, 0, 0));
        }
        assert_eq!(program.functions.len(), 739);
        let known = super::known_callees(&program, &graphs, &entry);
        let profile = LoopCallProfile::from_known(&program, &graphs, &entry, &known).unwrap();
        assert!(
            profile
                .seed_edges
                .iter()
                .any(|edge| edge.0 == Some(2) && edge.1 == 0),
            "captured key slots must join by resolved lexical cell, not upvalue index"
        );

        // Different lexical cells cannot be linked even when the call and definition have the
        // same shape and both happen to use imported/captured keys.
        program.functions[2].upvalues[4] = Upvalue::ParentLocal(24);
        let known = super::known_callees(&program, &graphs, &entry);
        let profile = LoopCallProfile::from_known(&program, &graphs, &entry, &known).unwrap();
        assert!(
            !profile
                .seed_edges
                .iter()
                .any(|edge| edge.0 == Some(2) && edge.1 == 0),
            "distinct lexical cells must not share a computed-key candidate"
        );

        // If the defining proto can be created from multiple parents, its upvalue index no
        // longer identifies one lexical binding. The new ranking hint must decline the match.
        let ambiguous_entry = graph(
            vec![
                (
                    ValueId(0),
                    GraphOp::MakeFunction {
                        index: 1,
                        site: SiteId(1),
                    },
                ),
                (
                    ValueId(1),
                    GraphOp::MakeFunction {
                        index: 1,
                        site: SiteId(3),
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::MakeFunction {
                        index: 2,
                        site: SiteId(2),
                    },
                ),
            ],
            &[],
        );
        let known = super::known_callees(&program, &graphs, &ambiguous_entry);
        let profile =
            LoopCallProfile::from_known(&program, &graphs, &ambiguous_entry, &known).unwrap();
        assert!(
            !profile
                .seed_edges
                .iter()
                .any(|edge| edge.0 == Some(2) && edge.1 == 0),
            "ambiguous closure creation must not lend one capture's key identity to another"
        );

        for _ in program.functions.len()..=super::MAX_GENERIC_LOOP_CALL_FUNCTIONS {
            program
                .functions
                .push(proto(vec![Instruction::Return], false));
            graphs.push(FunctionGraph::new(None, 0, 0));
        }
        assert_eq!(program.functions.len(), 1_025);
        let known = super::known_callees(&program, &graphs, &ambiguous_entry);
        assert!(
            LoopCallProfile::from_known(&program, &graphs, &ambiguous_entry, &known).is_none(),
            "the profile must continue to reject modules beyond the bounded proto cap"
        );
    }

    #[test]
    fn oversized_optional_method_index_preserves_known_call_profile() {
        use kiln_ir::{Constant, Instruction};

        let candidate_count = super::MAX_GENERIC_LOOP_CALL_EDGES + 1;
        let program = kiln_ir::Program {
            functions: vec![
                proto(vec![Instruction::Return], true),
                proto(vec![Instruction::Nop, Instruction::Jump(0)], false),
            ],
            constants: (0..candidate_count)
                .map(|index| {
                    Constant::String(kiln_ir::StringUnits::from(
                        format!("method_{index}").as_str(),
                    ))
                })
                .collect(),
            ..Default::default()
        };
        let target = FunctionGraph::new(None, 0, 0);
        let caller = graph(
            vec![
                (ValueId(0), GraphOp::LoadLocal(0)),
                (
                    ValueId(1),
                    GraphOp::Call {
                        callee: ValueId(0),
                        args: Vec::new(),
                        site: SiteId(0),
                        line: 0,
                        column: 0,
                    },
                ),
            ],
            &[(1, 0)],
        );
        let mut definitions = Vec::with_capacity(candidate_count * 3);
        for index in 0..candidate_count {
            let key = ValueId((index * 3) as u32);
            let function = ValueId((index * 3 + 1) as u32);
            definitions.push((
                key,
                GraphOp::Const {
                    index: index as u32,
                    constant: program.constants[index].clone(),
                },
            ));
            definitions.push((
                function,
                GraphOp::MakeFunction {
                    index: 0,
                    site: SiteId(index as u32),
                },
            ));
            definitions.push((
                ValueId((index * 3 + 2) as u32),
                GraphOp::Bridge {
                    instruction: Instruction::DefineNamedProperty {
                        name_function: false,
                    },
                    operands: vec![ValueId((candidate_count * 3) as u32), key, function],
                    pushes: 1,
                },
            ));
        }
        let entry = graph(definitions, &[]);
        let graphs = vec![target, caller];
        let mut known = Known {
            functions: vec![KnownValues::default(); 2],
            entry: KnownValues::default(),
            captures: vec![Vec::new(); 2],
            capture_bindings: vec![HashMap::new(); 2],
            captured_closures: vec![HashMap::new(); 2],
        };
        known.functions[1].known.insert(ValueId(0), 0);

        // The optional method index exceeds its edge bound, but the old direct-call seed remains.
        let profile = LoopCallProfile::from_known(&program, &graphs, &entry, &known).unwrap();
        assert_eq!(profile.seed_edges, [(Some(1), 0, 2)]);
        assert_eq!(profile.edges[1], [0]);
    }

    #[test]
    fn optional_method_fanout_cannot_consume_known_call_edge_budget() {
        use kiln_ir::{Constant, Instruction};

        let site_count = super::MAX_GENERIC_LOOP_CALL_EDGES + 1;
        let key = Constant::String(kiln_ir::StringUnits::from("evaluate"));
        let program = kiln_ir::Program {
            functions: vec![
                proto(vec![Instruction::Return], true),
                proto(vec![Instruction::Nop, Instruction::Jump(0)], false),
            ],
            constants: vec![key.clone()],
            ..Default::default()
        };
        let target = FunctionGraph::new(None, 0, 0);
        let mut caller_ops = vec![
            (ValueId(0), GraphOp::LoadLocal(0)),
            (
                ValueId(1),
                GraphOp::Call {
                    callee: ValueId(0),
                    args: Vec::new(),
                    site: SiteId(0),
                    line: 0,
                    column: 0,
                },
            ),
            (
                ValueId(2),
                GraphOp::Const {
                    index: 0,
                    constant: key,
                },
            ),
        ];
        let mut origins = vec![(1, 0)];
        for index in 0..site_count {
            let value = ValueId(index as u32 + 3);
            caller_ops.push((
                value,
                GraphOp::Bridge {
                    instruction: Instruction::CallComputed { argc: 0 },
                    operands: vec![ValueId(90000), ValueId(2), ValueId(90001)],
                    pushes: 1,
                },
            ));
            origins.push((value.0, 0));
        }
        let caller = graph(caller_ops, &origins);
        let entry = graph(
            vec![
                (
                    ValueId(0),
                    GraphOp::Const {
                        index: 0,
                        constant: program.constants[0].clone(),
                    },
                ),
                (
                    ValueId(1),
                    GraphOp::MakeFunction {
                        index: 0,
                        site: SiteId(1),
                    },
                ),
                (
                    ValueId(2),
                    GraphOp::Bridge {
                        instruction: Instruction::DefineNamedProperty {
                            name_function: false,
                        },
                        operands: vec![ValueId(80000), ValueId(0), ValueId(1)],
                        pushes: 1,
                    },
                ),
            ],
            &[],
        );
        let graphs = vec![target, caller];
        let mut known = Known {
            functions: vec![KnownValues::default(); 2],
            entry: KnownValues::default(),
            captures: vec![Vec::new(); 2],
            capture_bindings: vec![HashMap::new(); 2],
            captured_closures: vec![HashMap::new(); 2],
        };
        known.functions[1].known.insert(ValueId(0), 0);

        let profile = LoopCallProfile::from_known(&program, &graphs, &entry, &known).unwrap();
        assert_eq!(profile.seed_edges, [(Some(1), 0, 2)]);
        assert_eq!(profile.edges[1], [0]);
    }

    #[test]
    fn unavailable_loop_callers_cannot_select_live_callee_groups() {
        // Large non-loop bodies do not qualify for either legacy tier. Only a live loop
        // seed may select the otherwise live target's callee group.
        let body = kiln_ir::FunctionProto {
            inferred_name: None,
            span: kiln_ir::Span::default(),
            prologue_end: 0,
            asynchronous: false,
            name: None,
            arrow: false,
            generator: false,
            async_generator: false,
            is_method: false,
            is_class: false,
            parameter_count: 0,
            declared_length: 0,
            mapped_arguments: false,
            local_count: 0,
            code: vec![kiln_ir::Instruction::Return; 500],
            upvalues: Vec::new(),
            type_annotations: None,
        };
        let program = kiln_ir::Program {
            functions: vec![body.clone(), body],
            ..Default::default()
        };
        let mut profile = LoopCallProfile {
            seed_edges: vec![(Some(0), 1, 7)],
            edges: vec![vec![1], vec![]],
        };
        let select = |profile: &LoopCallProfile, available: &[bool]| {
            super::select_generic_optimization_functions_with_profile(
                &program,
                &[false, false],
                Some(available),
                profile,
            )
        };
        assert_eq!(select(&profile, &[false, true]), [false, false]);
        assert_eq!(select(&profile, &[true, true]), [false, true]);
        profile.seed_edges[0].0 = None;
        assert_eq!(
            select(&profile, &[false, true]),
            [false, true],
            "the module entry remains a live seed without a function caller"
        );
    }

    #[test]
    fn bounded_depth_propagates_known_edges_through_cycles_and_filters_unavailable_nodes() {
        let profile = LoopCallProfile {
            seed_edges: vec![(Some(2), 0, 7)],
            edges: vec![vec![1], vec![0], vec![]],
        };
        assert_eq!(profile.weights(None), [7, 7, 0]);
        assert_eq!(
            profile.weights(Some(&[true, true, false])),
            [0, 0, 0],
            "an unavailable loop caller cannot seed an otherwise live target"
        );
        assert_eq!(
            profile.weights(Some(&[true, false, true])),
            [7, 0, 0],
            "propagation stops at an unavailable intermediate body"
        );

        let chain = LoopCallProfile {
            seed_edges: vec![(None, 0, 3)],
            edges: vec![vec![1], vec![2], vec![3], vec![4], vec![5], vec![]],
        };
        assert_eq!(
            chain.weights(None),
            [3, 3, 3, 3, 3, 0],
            "transitive propagation stops at the configured depth"
        );
    }
}

fn loop_caller_count(program: &Program) -> usize {
    program
        .functions
        .iter()
        // Only loop bodies that can enter the bounded supplemental tier need graph call facts.
        // Large unrelated loops must not disable profiling of a few eligible callers.
        .filter(|proto| {
            proto.code.len() <= MAX_GENERIC_LOOP_CALL_INSTRUCTIONS
                && proto.code.len() <= MAX_GENERIC_LOOP_PROFILE_CALLER_INSTRUCTIONS
                && loop_work(proto).is_some()
        })
        .count()
        .saturating_add(usize::from(
            loop_context_weights(&program.code).iter().any(|w| *w > 0),
        ))
}

/// For each bytecode instruction, sum the spans of backward-edge loops containing it.
fn loop_context_weights(code: &[Instruction]) -> Vec<usize> {
    let mut delta = vec![0isize; code.len().saturating_add(1)];
    for (pc, instruction) in code.iter().enumerate() {
        let target = match instruction {
            Instruction::Jump(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::JumpIfReturnSignal(target) => *target as usize,
            _ => continue,
        };
        if target <= pc && pc < code.len() {
            let span = pc.saturating_sub(target).saturating_add(1) as isize;
            delta[target] = delta[target].saturating_add(span);
            delta[pc + 1] = delta[pc + 1].saturating_sub(span);
        }
    }
    let mut current = 0isize;
    delta
        .into_iter()
        .take(code.len())
        .map(|change| {
            current = current.saturating_add(change);
            current.max(0) as usize
        })
        .collect()
}

/// Return the total static span of backward control-flow edges and their count, if any.
fn loop_work(proto: &FunctionProto) -> Option<(usize, usize)> {
    let mut span = 0usize;
    let mut count = 0usize;
    for (pc, instruction) in proto.code.iter().enumerate() {
        let target = match instruction {
            Instruction::Jump(target)
            | Instruction::JumpIfFalse(target)
            | Instruction::JumpIfTruthy(target)
            | Instruction::JumpIfNotNullish(target)
            | Instruction::JumpIfReturnSignal(target) => *target as usize,
            _ => continue,
        };
        if target <= pc {
            span = span.saturating_add(pc.saturating_sub(target).saturating_add(1));
            count = count.saturating_add(1);
        }
    }
    (count > 0).then_some((span, count))
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
/// The proven results of static built-in method calls whose return type is guaranteed by the
/// language regardless of argument types.
pub(crate) fn intrinsic_results(
    graph: &FunctionGraph,
    program: &Program,
) -> HashMap<ValueId, Proven> {
    let defs: HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let resolve_alias = |mut v: ValueId| -> ValueId {
        for _ in 0..10 {
            match defs.get(&v) {
                Some(GraphOp::Unary {
                    op: UnaryOp::ToNumeric | UnaryOp::Plus,
                    value,
                }) => v = *value,
                Some(GraphOp::StoreLocal { value, .. })
                | Some(GraphOp::StoreCell { value, .. })
                | Some(GraphOp::CheckTdz { value, .. }) => v = *value,
                _ => break,
            }
        }
        v
    };
    let math: HashSet<ValueId> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| {
            matches!(op, GraphOp::LoadBuiltin(kiln_ir::Builtin::Math)).then_some(*v)
        })
        .collect();
    let number: HashSet<ValueId> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| {
            matches!(op, GraphOp::LoadBuiltin(kiln_ir::Builtin::Number)).then_some(*v)
        })
        .collect();
    let array: HashSet<ValueId> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| {
            matches!(op, GraphOp::LoadBuiltin(kiln_ir::Builtin::Array)).then_some(*v)
        })
        .collect();
    graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(v, op)| match op {
            GraphOp::CallMethod {
                object,
                name,
                args,
                prepared_callee,
                ..
            } if math.contains(object)
                && prepared_callee.is_none_or(|callee| {
                    matches!(
                        defs.get(&callee),
                        Some(GraphOp::GetProperty {
                            object: getter_receiver,
                            name: getter_name,
                            ..
                        }) if getter_receiver == object && getter_name == name
                    )
                }) =>
            {
                let key = match program.constants.get(*name as usize) {
                    Some(Constant::String(s)) => s.to_scalar_string().ok()?,
                    _ => return None,
                };
                // A prepared Math member is only typed when it is the exact `Math.sqrt` read
                // the prepared-call emitter can guard by method identity. Other prepared calls
                // keep their dynamic result type until their intrinsic fast paths become
                // prepared-aware too.
                if prepared_callee.is_some() && (key != "sqrt" || args.len() != 1) {
                    return None;
                }
                if matches!(key.as_str(), "clz32" if args.len() == 1)
                    || matches!(key.as_str(), "imul" if args.len() == 2)
                {
                    Some((*v, Proven::Int))
                } else if crate::parallel::PARALLEL_MATH
                    .iter()
                    .any(|(k, n)| key == *k && *n == args.len())
                {
                    Some((*v, Proven::Number))
                } else {
                    None
                }
            }
            GraphOp::CallMethod {
                object,
                name,
                prepared_callee: None,
                ..
            } if number.contains(object) => {
                let key = match program.constants.get(*name as usize) {
                    Some(Constant::String(s)) => s.to_scalar_string().ok()?,
                    _ => return None,
                };
                matches!(
                    key.as_str(),
                    "isFinite" | "isNaN" | "isInteger" | "isSafeInteger"
                )
                .then_some((*v, Proven::Boolean))
            }
            GraphOp::CallMethod {
                object,
                name,
                prepared_callee: None,
                ..
            } if array.contains(object) => {
                let key = match program.constants.get(*name as usize) {
                    Some(Constant::String(s)) => s.to_scalar_string().ok()?,
                    _ => return None,
                };
                (key == "isArray").then_some((*v, Proven::Boolean))
            }
            GraphOp::CallMethod {
                name,
                prepared_callee: None,
                ..
            } => {
                let key = match program.constants.get(*name as usize) {
                    Some(Constant::String(s)) => s.to_scalar_string().ok()?,
                    _ => return None,
                };
                match key.as_str() {
                    "indexOf" | "lastIndexOf" | "push" => Some((*v, Proven::Int)),
                    "includes" | "startsWith" | "endsWith" => Some((*v, Proven::Boolean)),
                    "substring" | "trim" | "toLowerCase" | "toUpperCase" | "join" => {
                        Some((*v, Proven::String))
                    }
                    _ => None,
                }
            }
            GraphOp::GetProperty { object, name, .. } => {
                let key = match program.constants.get(*name as usize) {
                    Some(Constant::String(s)) => s.to_scalar_string().ok()?,
                    _ => return None,
                };
                if key != "length" {
                    return None;
                }
                let resolved = resolve_alias(*object);
                let is_definite_array_or_string = match defs.get(&resolved) {
                    Some(GraphOp::NewArray { .. }) => true,
                    Some(GraphOp::Const {
                        constant: Constant::String(_),
                        ..
                    }) => true,
                    Some(GraphOp::Unary {
                        op: UnaryOp::TypeOf,
                        ..
                    }) => true,
                    Some(GraphOp::CallMethod {
                        name,
                        prepared_callee: None,
                        ..
                    }) => match program.constants.get(*name as usize) {
                        Some(Constant::String(s)) => match s.to_scalar_string().ok().as_deref() {
                            Some(
                                "map" | "filter" | "slice" | "concat" | "split" | "flat"
                                | "flatMap" | "substring" | "substr" | "trim" | "trimStart"
                                | "trimEnd" | "toLowerCase" | "toUpperCase" | "join" | "replace"
                                | "replaceAll",
                            ) => true,
                            _ => false,
                        },
                        _ => false,
                    },
                    _ => false,
                };
                if is_definite_array_or_string {
                    Some((*v, Proven::Int))
                } else {
                    None
                }
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
    length_bounds: &HashSet<ValueId>,
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
        let proven = proven_types_with_bounds(graph, &assumptions, length_bounds);
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
    /// Per proto, per upvalue index: what the unit knows about the binding it captures.
    captures: Vec<Vec<crate::parallel::Capture>>,
    /// Per proto, per upvalue index: the lexical binding cell captured by that slot, when the
    /// closure's creation site is unique enough to resolve conservatively.
    capture_bindings: Vec<HashMap<u32, Binding>>,
    /// Per proto: the upvalues whose binding's one real store is a closure of a proto made at
    /// one site (tier 4's calls through a captured `const f = x => ...`, body-checked per call).
    captured_closures: Vec<HashMap<u32, u32>>,
}

impl Known {
    /// Tier 4's answer to "which proto would a call through value `v` of proto `f`'s body run":
    /// a known closure, or a captured binding holding one closure (checked again at run time).
    fn parallel_callee(&self, graphs: &[FunctionGraph], f: u32, v: ValueId) -> Option<u32> {
        if let Some(&t) = self.functions.get(f as usize)?.known.get(&v) {
            return Some(t);
        }
        let graph = graphs.get(f as usize)?;
        let def = |v: ValueId| {
            graph
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter())
                .find(|(id, _)| *id == v)
                .map(|(_, op)| op)
        };
        let upvalue = match def(v)? {
            GraphOp::LoadUpvalue(k) => *k,
            GraphOp::CheckTdz { value, .. } => match def(*value)? {
                GraphOp::LoadUpvalue(k) => *k,
                _ => return None,
            },
            _ => return None,
        };
        self.captured_closures
            .get(f as usize)?
            .get(&upvalue)
            .copied()
    }
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
    // A ranking identity may cross nested captures only when every proto on that lexical chain
    // has one creation site. The existing call analyses keep their broader resolver behavior;
    // this extra restriction applies only to the new ranking hint.
    fn resolve_unique(
        program: &Program,
        parent: &[Option<Owner>],
        made: &[u32],
        function: u32,
        index: u32,
        remaining: usize,
    ) -> Option<Binding> {
        if remaining == 0 {
            return None;
        }
        if made.get(function as usize).copied() != Some(1) {
            return None;
        }
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
                    Owner::Function(p) => {
                        resolve_unique(program, parent, made, p, *next, remaining - 1)
                    }
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
    // A block slot is the top level's binding, unless a function the runtime bootstrap calls once
    // published its own cell there (`module_cells`): then it is that function's binding, as the
    // upvalue it replaced resolved to.
    let global_binding = binding_for_global;
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
                    GraphOp::StoreGlobal { slot, value } => {
                        (Some(global_binding(*slot)), Some(*value))
                    }
                    GraphOp::StoreUpvalue { index, value } => match owner {
                        Owner::Function(f) => (resolve(program, &parent, f, *index), Some(*value)),
                        Owner::Entry => (None, Some(*value)),
                    },
                    GraphOp::LoadCell(slot) => (Some(Binding { owner, slot: *slot }), None),
                    GraphOp::LoadGlobal(slot) => (Some(global_binding(*slot)), None),
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
            // (The runtime bootstrap's cell globals are read through the cell: `module_cells`.)
            let read_raw = !crate::module_cells::is_cell_global(binding.slot)
                && reads.get(binding).is_some_and(|rs| {
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
    // entry block nothing jumps back to -- is initialized before a later read in that same
    // body. A closure created in the prefix may be invoked as part of class construction or
    // another initializer, though, so this proof cannot erase checks in a different function.
    let mut settled: HashSet<Binding> = HashSet::new();
    for (binding, binding_stores) in &stores {
        if reads
            .get(binding)
            .is_some_and(|uses| uses.iter().any(|(owner, _)| owner != &binding.owner))
        {
            continue;
        }
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
            | GraphOp::SetPropertyStrict { object, .. }
            | GraphOp::DefineOwnProperty { object, .. } => fresh(prefix, *object, 0),
            GraphOp::Bridge { instruction, .. } => matches!(
                instruction,
                Instruction::DefineMethod
                    | Instruction::DefineAccessor { .. }
                    | Instruction::DefineNamedProperty { .. }
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
    // Every captured binding's writes, for `parallelMap`'s tier 1: a declaration's hoisted
    // `undefined` (a store of `undefined` in the owner's entry block ahead of every other
    // store) is not a write; any other store is. A poisoned unit -- a write this walk cannot
    // place -- resolves nothing.
    let mut capture_bindings = vec![HashMap::new(); graphs.len()];
    if loop_profile_inputs_within_limits(program, graphs, entry) {
        for (f, graph) in graphs.iter().enumerate() {
            for (_, op) in graph.blocks.iter().flat_map(|block| &block.operations) {
                if let GraphOp::LoadUpvalue(index) = op
                    && let Some(binding) =
                        resolve_unique(program, &parent, &made, f as u32, *index, graphs.len())
                {
                    capture_bindings[f].insert(*index, binding);
                }
            }
        }
    }
    let captures: Vec<Vec<crate::parallel::Capture>> = (0..graphs.len() as u32)
        .map(|f| {
            (0..program.functions[f as usize].upvalues.len() as u32)
                .map(|k| {
                    let Some(binding) = resolve(program, &parent, f, k) else {
                        return crate::parallel::Capture::Unresolved;
                    };
                    if poisoned {
                        return crate::parallel::Capture::Unresolved;
                    }
                    let all = stores.get(&binding).map(Vec::as_slice).unwrap_or(&[]);
                    let entry_block = graph_of(binding.owner).entry_block;
                    let in_entry = |s: &Store| s.owner == binding.owner && s.block == entry_block;
                    // The first store in the owner's entry block that is not `undefined`; a
                    // hoisted `undefined` ahead of it initializes rather than writes.
                    let first_real = all
                        .iter()
                        .filter(|s| in_entry(s) && s.stored != Stored::Undefined)
                        .map(|s| s.position)
                        .min()
                        .unwrap_or(usize::MAX);
                    let writes = all
                        .iter()
                        .filter(|s| {
                            !(in_entry(s)
                                && s.stored == Stored::Undefined
                                && s.position < first_real)
                        })
                        .count();
                    crate::parallel::Capture::Writes(writes)
                })
                .collect()
        })
        .collect();
    // Tier 4 of the parallel built-ins: a captured binding whose one real store is a closure of a
    // proto made at one site -- the function a call through it would run, if it runs any. Weaker
    // than a known binding (a read may still be in the dead zone, or before the store), which is
    // why a parallel clone never calls through one without comparing the closure's body with the
    // proto's first (`parallel_heap`'s body check).
    let captured_closures: Vec<HashMap<u32, u32>> = (0..graphs.len() as u32)
        .map(|f| {
            let mut out = HashMap::new();
            if poisoned {
                return out;
            }
            for k in 0..program.functions[f as usize].upvalues.len() as u32 {
                let Some(binding) = resolve(program, &parent, f, k) else {
                    continue;
                };
                let real: Vec<&Store> = stores
                    .get(&binding)
                    .map(|all| {
                        all.iter()
                            .filter(|s| s.stored != Stored::Undefined)
                            .collect()
                    })
                    .unwrap_or_default();
                if let [one] = real[..]
                    && let Stored::Closure(index) = one.stored
                    && unique(index)
                {
                    out.insert(k, index);
                }
            }
            out
        })
        .collect();
    let mut known = Known {
        functions: vec![KnownValues::default(); graphs.len()],
        entry: KnownValues::default(),
        captures,
        capture_bindings,
        captured_closures,
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
    }
    // An effectful prefix may call a closure before its captured binding is initialized.
    // That prevents unconditional dead-zone elimination, but not guarded specialization:
    // an array entry guard fails on the uninitialized sentinel. Once the sole store has
    // supplied an array, no later execution can change this cell. Require that store outside
    // every control-flow cycle, and reject any subsequent uninitialization as well.
    let mut store_repeats = HashMap::new();
    for (binding, binding_stores) in &stores {
        let [store] = binding_stores.as_slice() else {
            continue;
        };
        let graph = graph_of(binding.owner);
        // Parameters already contain an argument before their first explicit store. An
        // entry guard could accept that argument even though the sole store is still ahead.
        if store.owner != binding.owner || binding.slot < graph.parameter_count {
            continue;
        }
        let Some(entry) = graph.block(graph.entry_block) else {
            continue;
        };
        let repeats = *store_repeats
            .entry((binding.owner, store.block))
            .or_insert_with(|| {
                let Some(block) = graph.block(store.block) else {
                    return true;
                };
                let mut pending = block.successors.clone();
                let mut visited = HashSet::new();
                while let Some(next) = pending.pop() {
                    if next == store.block {
                        return true;
                    }
                    if visited.insert(next) {
                        let Some(block) = graph.block(next) else {
                            return true;
                        };
                        pending.extend(block.successors.iter().copied());
                    }
                }
                false
            });
        if repeats
            || !entry.predecessors.is_empty()
            || graph.blocks.iter().any(|block| {
                block
                    .operations
                    .iter()
                    .enumerate()
                    .any(|(position, (_, op))| {
                        matches!(op, GraphOp::MarkUninitialized(slot) if *slot == binding.slot)
                            && (block.id != graph.entry_block
                                || (store.block == graph.entry_block && position > store.position))
                    })
            })
        {
            continue;
        }
        for (owner, vid) in reads.get(binding).into_iter().flatten() {
            values_of(&mut known, *owner).invariant.insert(*vid);
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

#[cfg(test)]
#[path = "tests/direct_known_calls.rs"]
mod effect_proto_diagnostic;
