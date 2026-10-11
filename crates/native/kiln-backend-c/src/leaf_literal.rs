//! Reads of a small leaf function's fresh object literal, without the call or the object.
//!
//! `function one(i) { return { a: i }; }` called as `one(i).a`: the call is to a closure the
//! unit proved (`direct::known_call_targets`, the same proof direct calls trust), the callee
//! does nothing but build one literal from its parameters and constants and return it, and the
//! caller uses the result only to read a key that literal defines. The read is then exactly the
//! argument (or constant) the literal stored under that key, so the call and the allocation
//! both disappear.
//!
//! This is a proof, not a speculation, so it has no runtime guard and no `SPECULATION_INVARIANTS`
//! row. The language rules it relies on:
//!
//! * ECMA-262 §10.2.1 [[Call]] of an ordinary function: the callee runs only its body; with a
//!   body of `Const`s and `NewObject`s over parameters nothing observable happens except the
//!   allocation (`this`, `arguments`, the environment and defaults are never touched, because
//!   any of them would put another operation, a block or a bridge in the graph).
//! * §13.2.5.4 object literal evaluation with string-literal keys: the fresh object's own data
//!   property `k` holds the evaluated value, and `Get(o, k)` on an own data property answers it
//!   before any prototype lookup, so patched prototypes and built-ins cannot change the answer.
//! * §10.2.1.2 OrdinaryCallBindThis / §10.4.4 parameter binding: with a simple parameter list
//!   parameter `k` holds argument `k`, or `undefined` when fewer arguments were passed.
//!
//! Evaluation order: the callee expression and every argument were already evaluated by
//! operations that stay in the graph; only the pure call and the pure read are removed.
//! `EmissionPolicy::speculate == Some(false)` switches the pass off.

use std::collections::{HashMap, HashSet};

use kiln_ir::{Constant, Program};

use crate::graph::{
    FunctionGraph, GraphOp, Terminator, ValueId, compact_value_ids, remap_graph_operands,
};

/// Where a readable key's value comes from.
#[derive(Clone, Debug)]
enum Field {
    /// Parameter `k` of the leaf.
    Param(usize),
    /// A constant the leaf evaluates (copied into the caller verbatim: the pool is the program's).
    Const(GraphOp),
}

#[derive(Clone, Debug, Default)]
struct Leaf {
    fields: HashMap<String, Field>,
}

fn is_array_index(key: &str) -> bool {
    key.parse::<u32>().is_ok_and(|n| n != u32::MAX)
}

fn terminator_values(terminator: &Terminator) -> Vec<ValueId> {
    match terminator {
        Terminator::Return(Some(value))
        | Terminator::Throw(value)
        | Terminator::Branch {
            condition: value, ..
        }
        | Terminator::BranchNotNullish {
            condition: value, ..
        } => vec![*value],
        Terminator::Suspend {
            value,
            spilled,
            delegate,
            ..
        } => {
            let mut values = vec![*value];
            if let Some((iterator, _, _)) = delegate {
                values.push(*iterator);
            }
            values.extend(spilled.iter().copied());
            values
        }
        _ => Vec::new(),
    }
}

fn string_of(constant: &Constant) -> Option<String> {
    match constant {
        Constant::String(units) => units.to_scalar_string().ok(),
        _ => None,
    }
}

/// The summary of one proto, or `None` when it is anything but "build a literal, return it".
fn summarize_leaf(program: &Program, proto_index: usize, graph: &FunctionGraph) -> Option<Leaf> {
    let proto = &program.functions[proto_index];
    if proto.asynchronous || proto.generator || proto.async_generator || proto.is_class {
        return None;
    }
    if graph.blocks.len() != 1 || graph.seam_block.is_some() {
        return None;
    }
    let block = &graph.blocks[0];
    if block.exception_handler.is_some() {
        return None;
    }
    let Terminator::Return(Some(returned)) = block.terminator else {
        return None;
    };
    let defs: HashMap<ValueId, &GraphOp> =
        block.operations.iter().map(|(v, op)| (*v, op)).collect();
    let mut outer = None;
    for (id, op) in &block.operations {
        match op {
            GraphOp::Const { .. } => {}
            GraphOp::NewObject { pairs, .. } => {
                for (key, _) in pairs {
                    // Static string keys only: a computed key can run user code (ToPropertyKey),
                    // and `__proto__` is not an own data property.
                    let Some(GraphOp::Const { constant, .. }) = defs.get(key) else {
                        return None;
                    };
                    let text = string_of(constant)?;
                    if text == "__proto__" {
                        return None;
                    }
                }
                if *id == returned {
                    outer = Some(pairs);
                }
            }
            _ => return None,
        }
    }
    let pairs = outer?;
    let mut leaf = Leaf::default();
    let mut seen = HashSet::new();
    for (key, value) in pairs {
        let Some(GraphOp::Const { constant, .. }) = defs.get(key) else {
            return None;
        };
        let text = string_of(constant)?;
        if is_array_index(&text) || !seen.insert(text.clone()) {
            return None;
        }
        let param = graph
            .initial_locals
            .iter()
            .position(|local| local == value)
            .filter(|slot| *slot < proto.parameter_count as usize);
        if let Some(slot) = param {
            leaf.fields.insert(text, Field::Param(slot));
        } else if let Some(op @ GraphOp::Const { .. }) = defs.get(value) {
            leaf.fields.insert(text, Field::Const((*op).clone()));
        }
        // Any other value (a nested literal, a non-parameter local) is built but not readable
        // through this pass: the key stays out of `fields` and a read of it is left alone.
    }
    Some(leaf)
}

/// Apply the pass to every body. Returns how many reads were rewritten.
pub(crate) fn inline_leaf_literal_reads(
    program: &Program,
    graphs: &mut [FunctionGraph],
    entry: &mut FunctionGraph,
) -> usize {
    let leaves: Vec<Option<Leaf>> = graphs
        .iter()
        .enumerate()
        .map(|(index, graph)| summarize_leaf(program, index, graph))
        .collect();
    if leaves
        .iter()
        .all(|leaf| leaf.as_ref().is_none_or(|l| l.fields.is_empty()))
    {
        return 0;
    }
    let (function_targets, entry_targets) =
        crate::direct::known_call_targets(program, graphs, entry);
    let undefined_index = program
        .constants
        .iter()
        .position(|constant| matches!(constant, Constant::Undefined));
    let mut total = 0;
    for (index, graph) in graphs.iter_mut().enumerate() {
        total += rewrite_graph(
            program,
            graph,
            &function_targets[index],
            &leaves,
            undefined_index,
        );
    }
    total += rewrite_graph(program, entry, &entry_targets, &leaves, undefined_index);
    total
}

fn rewrite_graph(
    program: &Program,
    graph: &mut FunctionGraph,
    targets: &HashMap<ValueId, u32>,
    leaves: &[Option<Leaf>],
    undefined_index: Option<usize>,
) -> usize {
    if targets.is_empty() {
        return 0;
    }
    // Every use of every value, anywhere: a call result may be rewritten only when its single
    // use is the read.
    let mut uses: HashMap<ValueId, usize> = HashMap::new();
    for block in &graph.blocks {
        for (_, op) in &block.operations {
            for operand in op.operands() {
                *uses.entry(operand).or_default() += 1;
            }
        }
        for value in terminator_values(&block.terminator) {
            *uses.entry(value).or_default() += 1;
        }
    }
    // Pass 1: choose rewrites. `drop_ops` are removed, `replace_ops` swap the read for a
    // constant, `aliases` redirect the read's users to an argument.
    let mut drop_ops: HashSet<ValueId> = HashSet::new();
    let mut replace_ops: HashMap<ValueId, GraphOp> = HashMap::new();
    let mut aliases: HashMap<ValueId, ValueId> = HashMap::new();
    for block in &graph.blocks {
        // Calls of this block whose callee is a known leaf, by result value.
        let mut calls: HashMap<ValueId, (&[ValueId], &Leaf)> = HashMap::new();
        for (value, op) in &block.operations {
            match op {
                GraphOp::Call { callee, args, .. } => {
                    let Some(&proto) = targets.get(callee) else {
                        continue;
                    };
                    let Some(Some(leaf)) = leaves.get(proto as usize) else {
                        continue;
                    };
                    if uses.get(value).copied() == Some(1) {
                        calls.insert(*value, (args.as_slice(), leaf));
                    }
                }
                GraphOp::GetProperty { object, name, .. } => {
                    let Some(&(args, leaf)) = calls.get(object) else {
                        continue;
                    };
                    let Some(Constant::String(units)) = program.constants.get(*name as usize)
                    else {
                        continue;
                    };
                    let Ok(key) = units.to_scalar_string() else {
                        continue;
                    };
                    let Some(field) = leaf.fields.get(&key) else {
                        continue;
                    };
                    match field {
                        Field::Param(slot) => match args.get(*slot) {
                            Some(argument) => {
                                aliases.insert(*value, *argument);
                                drop_ops.insert(*value);
                            }
                            None => {
                                let Some(index) = undefined_index else {
                                    continue;
                                };
                                replace_ops.insert(
                                    *value,
                                    GraphOp::Const {
                                        index: index as u32,
                                        constant: Constant::Undefined,
                                    },
                                );
                            }
                        },
                        Field::Const(op) => {
                            replace_ops.insert(*value, op.clone());
                        }
                    }
                    drop_ops.insert(*object);
                }
                _ => {}
            }
        }
    }
    if drop_ops.is_empty() {
        return 0;
    }
    let rewritten = aliases.len() + replace_ops.len();
    let backup = graph.clone();
    for block in &mut graph.blocks {
        block
            .operations
            .retain(|(value, _)| !drop_ops.contains(value));
        for (value, op) in &mut block.operations {
            if let Some(replacement) = replace_ops.get(value) {
                *op = replacement.clone();
            }
        }
    }
    let resolve = |mut value: ValueId| {
        while let Some(&next) = aliases.get(&value) {
            value = next;
        }
        value
    };
    remap_graph_operands(graph, resolve);
    // Value ids changed: everything keyed by them is rebuilt from the new graph.
    graph.non_escaping_objects.clear();
    graph.scalar_objects.clear();
    graph.scalar_alias_map.clear();
    graph.spread_scalar_objects.clear();
    graph.spread_alias_map.clear();
    graph.spread_sinks.clear();
    compact_value_ids(graph);
    graph.mark_non_escaping_objects(program);
    // Insurance, not the proof: a graph the verifier rejects is put back as it was.
    if crate::verifier::verify_function_graph(graph).is_err() {
        *graph = backup;
        return 0;
    }
    rewritten
}
