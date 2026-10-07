use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use kiln_ir::{
    Constant, FunctionProto, Instruction, ModuleProgram, Program, RemovableDeclaration, Upvalue,
};

use crate::graph::{FunctionGraph, GraphOp, Terminator, ValueId, build_function_graph};

fn graph_for(proto: &FunctionProto, program: &Program) -> Option<FunctionGraph> {
    let graph = build_function_graph(proto, program).ok()?;
    crate::verify_function_graph(&graph).ok()?;
    Some(graph)
}

fn entry_proto(program: &Program) -> FunctionProto {
    FunctionProto {
        inferred_name: None,
        asynchronous: program.code.iter().any(|instruction| {
            matches!(
                instruction,
                Instruction::Await | Instruction::Yield | Instruction::YieldStar(_)
            )
        }),
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

fn function_parent_candidates(program: &Program) -> Vec<Vec<Option<usize>>> {
    let mut parents = vec![Vec::<Option<usize>>::new(); program.functions.len()];
    for instruction in &program.code {
        if let Instruction::MakeFunction(index) = instruction
            && let Some(found) = parents.get_mut(*index as usize)
        {
            found.push(None);
        }
    }
    for (parent, function) in program.functions.iter().enumerate() {
        for instruction in &function.code {
            if let Instruction::MakeFunction(index) = instruction
                && let Some(found) = parents.get_mut(*index as usize)
            {
                found.push(Some(parent));
            }
        }
    }
    for found in &mut parents {
        found.sort_unstable();
        found.dedup();
    }
    parents
}

fn module_upvalue_slots_with_parents(
    program: &Program,
    parent_candidates: &[Vec<Option<usize>>],
) -> Vec<Vec<Option<u32>>> {
    let parents = parent_candidates
        .iter()
        .map(|found| (found.len() == 1).then(|| found[0]))
        .collect::<Vec<_>>();
    let mut module_upvalues = program
        .functions
        .iter()
        .map(|function| vec![None; function.upvalues.len()])
        .collect::<Vec<Vec<Option<u32>>>>();
    for _ in 0..program.functions.len().max(1) {
        let mut changed = false;
        for (index, function) in program.functions.iter().enumerate() {
            for (upvalue_index, upvalue) in function.upvalues.iter().enumerate() {
                let resolved = match (parents[index], upvalue) {
                    (
                        Some(None),
                        Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. },
                    ) => Some(*slot),
                    (
                        Some(Some(parent)),
                        Upvalue::ParentUpvalue(parent_index)
                        | Upvalue::NamedParentUpvalue {
                            index: parent_index,
                            ..
                        },
                    ) => module_upvalues
                        .get(parent)
                        .and_then(|slots| slots.get(*parent_index as usize))
                        .copied()
                        .flatten(),
                    _ => None,
                };
                if resolved.is_some() && module_upvalues[index][upvalue_index] != resolved {
                    module_upvalues[index][upvalue_index] = resolved;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    module_upvalues
}

fn captured_slot_owner(
    program: &Program,
    parents: &[Vec<Option<usize>>],
    function: usize,
    upvalue: usize,
) -> Option<(Option<usize>, u32)> {
    let mut function = function;
    let mut upvalue = upvalue;
    let mut seen = HashSet::new();
    loop {
        if !seen.insert((function, upvalue)) {
            return None;
        }
        let parent = *parents.get(function)?.as_slice().first()?;
        if parents[function].len() != 1 {
            return None;
        }
        match *program.functions.get(function)?.upvalues.get(upvalue)? {
            Upvalue::ParentLocal(slot) | Upvalue::NamedParentLocal { slot, .. } => {
                return Some((parent, slot));
            }
            Upvalue::ParentUpvalue(index) | Upvalue::NamedParentUpvalue { index, .. } => {
                function = parent?;
                upvalue = index as usize;
            }
        }
    }
}

fn stored_declaration(code: &[Instruction], pc: usize) -> Option<(u32, bool)> {
    match code.get(pc + 1) {
        Some(Instruction::StoreLocal(slot)) | Some(Instruction::StoreCell(slot)) => {
            Some((*slot, false))
        }
        Some(Instruction::StoreGlobal(slot)) => Some((*slot, true)),
        _ => None,
    }
}

/// Functions a closed-world module can instantiate, and the dead declaration stores that made
/// the others unreachable. The same analysis runs in every reachable lexical scope: bundled
/// modules commonly put thousands of helper declarations inside one live wrapper, and treating
/// every nested `MakeFunction` as live defeats closed-world linking almost completely.
///
/// Only the compiler's simplest declaration shape, `MakeFunction; Store*`, is removable. Class
/// methods and functions installed into objects remain live. A hand-authored program whose
/// capture parent is ambiguous falls back to the complete function table.
///
/// `pub(crate)` so the facts analysis can read retained code for member-name demand.
pub fn closed_world_function_liveness(
    program: &Program,
    demanded: &BTreeSet<u32>,
    _direct_eval: bool,
    declarations: &[RemovableDeclaration],
) -> (HashSet<usize>, Vec<HashSet<u32>>, BTreeSet<u32>) {
    let parents = function_parent_candidates(program);
    let mut module_live = demanded.clone();
    let mut function_live = vec![BTreeSet::<u32>::new(); program.functions.len()];
    let mut reachable = HashSet::new();
    let mut declaration_at_pc = vec![None; program.code.len()];
    for (index, declaration) in declarations.iter().enumerate() {
        let start = declaration.pc_start as usize;
        let end = declaration.pc_end as usize;
        if start > end || end > declaration_at_pc.len() {
            continue;
        }
        for owner in &mut declaration_at_pc[start..end] {
            *owner = Some(index);
        }
    }
    let mut active_declarations = vec![false; declarations.len()];

    loop {
        let mut changed = false;
        for (index, declaration) in declarations.iter().enumerate() {
            if !active_declarations[index]
                && declaration
                    .local_slots
                    .iter()
                    .any(|slot| module_live.contains(slot))
            {
                active_declarations[index] = true;
                changed = true;
            }
        }
        let live_owners = std::iter::once(None::<usize>)
            .chain(reachable.iter().copied().map(Some))
            .collect::<Vec<_>>();
        for owner in live_owners {
            let code = match owner {
                None => program.code.as_slice(),
                Some(index) => program.functions[index].code.as_slice(),
            };
            for (pc, instruction) in code.iter().enumerate() {
                if owner.is_none()
                    && declaration_at_pc[pc].is_some_and(|index| !active_declarations[index])
                {
                    continue;
                }
                match instruction {
                    Instruction::LoadGlobal(slot) => changed |= module_live.insert(*slot),
                    Instruction::LoadLocal(slot) | Instruction::LoadCell(slot) => match owner {
                        None => changed |= module_live.insert(*slot),
                        Some(index) => changed |= function_live[index].insert(*slot),
                    },
                    Instruction::DirectEval(site) => {
                        // The eval compiler can read or write every binding in its site's
                        // lexical environment. Preserve those cells, but do not let an eval in
                        // an unreachable function keep unrelated functions and module slots live.
                        for binding in &site.bindings {
                            let (binding_owner, slot) = match binding.access {
                                kiln_ir::EvalAccess::Cell(slot) => (owner, slot),
                                kiln_ir::EvalAccess::Upvalue(upvalue) => {
                                    let Some(index) = owner else {
                                        // A top-level eval cannot name an upvalue. Malformed or
                                        // hand-authored IR falls back to keeping the whole scope.
                                        return (
                                            (0..program.functions.len()).collect(),
                                            vec![HashSet::new(); program.functions.len() + 1],
                                            (0..program.local_count).collect(),
                                        );
                                    };
                                    let Some((binding_owner, slot)) = captured_slot_owner(
                                        program,
                                        &parents,
                                        index,
                                        upvalue as usize,
                                    ) else {
                                        return (
                                            (0..program.functions.len()).collect(),
                                            vec![HashSet::new(); program.functions.len() + 1],
                                            (0..program.local_count).collect(),
                                        );
                                    };
                                    (binding_owner, slot)
                                }
                            };
                            match binding_owner {
                                None => changed |= module_live.insert(slot),
                                Some(index) => changed |= function_live[index].insert(slot),
                            }
                        }
                    }
                    _ => {}
                }
            }
            for (pc, instruction) in code.iter().enumerate() {
                if owner.is_none()
                    && declaration_at_pc[pc].is_some_and(|index| !active_declarations[index])
                {
                    continue;
                }
                let Instruction::MakeFunction(index) = instruction else {
                    continue;
                };
                let target = *index as usize;
                let is_live = match stored_declaration(code, pc) {
                    Some((slot, true)) => module_live.contains(&slot),
                    Some((slot, false)) => match owner {
                        None => module_live.contains(&slot),
                        Some(index) => function_live[index].contains(&slot),
                    },
                    None => true,
                };
                if is_live {
                    changed |= reachable.insert(target);
                }
            }
        }

        let reachable_now = reachable.iter().copied().collect::<Vec<_>>();
        for index in reachable_now {
            let Some(function) = program.functions.get(index) else {
                continue;
            };
            for upvalue in 0..function.upvalues.len() {
                let Some((owner, slot)) =
                    captured_slot_owner(program, &parents, index, upvalue as usize)
                else {
                    return (
                        (0..program.functions.len()).collect(),
                        vec![HashSet::new(); program.functions.len() + 1],
                        (0..program.local_count).collect(),
                    );
                };
                match owner {
                    None => changed |= module_live.insert(slot),
                    Some(parent) => changed |= function_live[parent].insert(slot),
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut dead_pcs = vec![HashSet::new(); program.functions.len() + 1];
    for (index, declaration) in declarations.iter().enumerate() {
        if !active_declarations[index] {
            dead_pcs[0].extend(declaration.pc_start..declaration.pc_end);
        }
    }
    for (owner, code) in std::iter::once((None, program.code.as_slice())).chain(
        program
            .functions
            .iter()
            .enumerate()
            .filter(|(index, _)| reachable.contains(index))
            .map(|(index, function)| (Some(index), function.code.as_slice())),
    ) {
        for (pc, instruction) in code.iter().enumerate() {
            let Instruction::MakeFunction(index) = instruction else {
                continue;
            };
            let dead = match stored_declaration(code, pc) {
                Some((slot, true)) => !module_live.contains(&slot),
                Some((slot, false)) => match owner {
                    None => !module_live.contains(&slot),
                    Some(index) => !function_live[index].contains(&slot),
                },
                None => false,
            };
            if dead && !reachable.contains(&(*index as usize)) {
                let scope = owner.map_or(0, |index| index + 1);
                dead_pcs[scope].insert(pc as u32);
                dead_pcs[scope].insert((pc + 1) as u32);
            }
        }
    }
    (reachable, dead_pcs, module_live)
}

/// Module slots read by evaluation or by a function reachable from a demanded export.
///
/// This is the linker-facing half of the same conservative analysis used during emission.
pub fn closed_world_module_live_slots(
    program: &Program,
    demanded: &BTreeSet<u32>,
    direct_eval: bool,
) -> BTreeSet<u32> {
    closed_world_function_liveness(program, demanded, direct_eval, &[]).2
}

/// [`closed_world_module_live_slots`] with compiler-recorded pure declaration boundaries.
pub fn closed_world_module_live_slots_with_declarations(
    program: &Program,
    demanded: &BTreeSet<u32>,
    direct_eval: bool,
    declarations: &[RemovableDeclaration],
) -> BTreeSet<u32> {
    closed_world_function_liveness(program, demanded, direct_eval, declarations).2
}

/// Instruction and function counts after the same declaration liveness used by C emission.
/// Without a closed-world export demand, conservatively count the entire module. This is an
/// IR-only query: callers can choose specialization before constructing any SSA graphs.
pub fn retained_module_size(
    module: &ModuleProgram,
    demanded: Option<&BTreeSet<u32>>,
) -> (usize, usize) {
    let program = &module.program;
    let Some(demanded) = demanded else {
        return (
            program.code.len()
                + program
                    .functions
                    .iter()
                    .map(|f| f.code.len())
                    .sum::<usize>(),
            program.functions.len(),
        );
    };
    if module.link_facts.direct_eval {
        return (
            program.code.len()
                + program
                    .functions
                    .iter()
                    .map(|f| f.code.len())
                    .sum::<usize>(),
            program.functions.len(),
        );
    }
    let (reachable, dead_pcs, _) = closed_world_function_liveness(
        program,
        demanded,
        module.link_facts.direct_eval,
        &module.link_facts.removable_declarations,
    );
    let instructions = program.code.len() - dead_pcs[0].len()
        + reachable
            .iter()
            .map(|&index| program.functions[index].code.len() - dead_pcs[index + 1].len())
            .sum::<usize>();
    (instructions, reachable.len())
}

/// Instructions retained by the same declaration/function liveness used during C emission.
/// Consumers planning separately linked runtime engines must inspect this view rather than every
/// function prototype in the source module, or dead library code keeps unrelated engines alive.
pub fn retained_module_instructions<'a>(
    module: &'a ModuleProgram,
    demanded: Option<&BTreeSet<u32>>,
) -> Vec<&'a Instruction> {
    let program = &module.program;
    let Some(demanded) = demanded else {
        return program
            .code
            .iter()
            .chain(
                program
                    .functions
                    .iter()
                    .flat_map(|function| function.code.iter()),
            )
            .collect();
    };
    let (reachable, dead_pcs, _) = closed_world_function_liveness(
        program,
        demanded,
        module.link_facts.direct_eval,
        &module.link_facts.removable_declarations,
    );
    let mut retained = program
        .code
        .iter()
        .enumerate()
        .filter(|(pc, _)| !dead_pcs[0].contains(&(*pc as u32)))
        .map(|(_, instruction)| instruction)
        .collect::<Vec<_>>();
    for index in reachable {
        retained.extend(
            program.functions[index]
                .code
                .iter()
                .enumerate()
                .filter(|(pc, _)| !dead_pcs[index + 1].contains(&(*pc as u32)))
                .map(|(_, instruction)| instruction),
        );
    }
    retained
}

/// Statically named members read from one module namespace slot, or `None` when the namespace
/// escapes and therefore needs its complete export surface.
///
/// The value graph supplies the stack provenance that bytecode alone does not: arguments may be
/// evaluated between loading a namespace and calling its method. Aliases used only to carry the
/// same value across TDZ checks, locals, or control-flow joins remain inspectable. Any other use
/// is deliberately the conservative answer.
fn namespace_alias_reaches(
    value: ValueId,
    defs: &HashMap<ValueId, &GraphOp>,
    roots: &HashSet<ValueId>,
    local_aliases: &HashSet<u32>,
) -> bool {
    let mut pending = vec![value];
    let mut seen = HashSet::new();
    while let Some(value) = pending.pop() {
        if roots.contains(&value) {
            return true;
        }
        if !seen.insert(value) {
            continue;
        }
        match defs.get(&value) {
            Some(GraphOp::StoreLocal { value, .. } | GraphOp::CheckTdz { value, .. }) => {
                pending.push(*value);
            }
            Some(GraphOp::Phi { inputs }) => {
                pending.extend(inputs.iter().map(|(_, input)| *input));
            }
            Some(GraphOp::LoadLocal(slot)) if local_aliases.contains(slot) => return true,
            _ => {}
        }
    }
    false
}

/// Resolve namespace reads for several import slots using one verified graph construction.
/// Graphs live only for this batch; callers retain the compact answers, not program graphs.
pub fn closed_world_namespace_members_for_slots(
    program: &Program,
    slots: impl IntoIterator<Item = u32>,
    direct_eval: bool,
) -> BTreeMap<u32, Option<BTreeSet<String>>> {
    let slots = slots.into_iter().collect::<BTreeSet<_>>();
    if slots.is_empty() {
        return BTreeMap::new();
    }
    let graphs = namespace_graphs(program, direct_eval);
    slots
        .into_iter()
        .map(|slot| {
            let members = graphs
                .as_ref()
                .and_then(|graphs| namespace_members(program, slot, graphs));
            (slot, members)
        })
        .collect()
}

pub fn closed_world_namespace_members(
    program: &Program,
    slot: u32,
    direct_eval: bool,
) -> Option<BTreeSet<String>> {
    let graphs = namespace_graphs(program, direct_eval)?;
    namespace_members(program, slot, &graphs)
}

fn namespace_graphs(program: &Program, direct_eval: bool) -> Option<NamespaceGraphs> {
    if direct_eval {
        return None;
    }
    let parent_candidates = function_parent_candidates(program);
    // A compiler-produced function has one lexical creation site. Hand-authored IR can reuse a
    // function prototype under multiple parents; an upvalue in that shape has no single module
    // provenance, so a partial namespace answer would be unsound.
    if program
        .functions
        .iter()
        .enumerate()
        .any(|(index, function)| {
            parent_candidates[index].len() > 1 && !function.upvalues.is_empty()
        })
    {
        return None;
    }
    let module_upvalues = module_upvalue_slots_with_parents(program, &parent_candidates);
    let entry = graph_for(&entry_proto(program), program)?;
    let functions = program
        .functions
        .iter()
        .map(|function| graph_for(function, program).ok_or(()))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(NamespaceGraphs {
        entry,
        functions,
        module_upvalues,
    })
}

struct NamespaceGraphs {
    entry: FunctionGraph,
    functions: Vec<FunctionGraph>,
    module_upvalues: Vec<Vec<Option<u32>>>,
}

fn namespace_members(
    program: &Program,
    slot: u32,
    graphs: &NamespaceGraphs,
) -> Option<BTreeSet<String>> {
    let NamespaceGraphs {
        entry,
        functions,
        module_upvalues,
    } = graphs;
    let mut members = BTreeSet::new();
    for (graph, function_index) in std::iter::once((entry, None)).chain(
        functions
            .iter()
            .enumerate()
            .map(|(index, graph)| (graph, Some(index))),
    ) {
        let defs = graph
            .blocks
            .iter()
            .flat_map(|block| block.operations.iter().map(|(value, op)| (*value, op)))
            .collect::<std::collections::HashMap<_, _>>();
        let roots = graph
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter_map(|(value, op)| match op {
                GraphOp::LoadGlobal(found) if *found == slot => Some(*value),
                GraphOp::LoadLocal(found) | GraphOp::LoadCell(found)
                    if function_index.is_none() && *found == slot =>
                {
                    Some(*value)
                }
                GraphOp::LoadUpvalue(index)
                    if function_index.is_some_and(|function| {
                        module_upvalues[function]
                            .get(*index as usize)
                            .is_some_and(|found| *found == Some(slot))
                    }) =>
                {
                    Some(*value)
                }
                _ => None,
            })
            .collect::<HashSet<_>>();
        if roots.is_empty() {
            continue;
        }
        // A namespace value can cross a suspension through a local or a resume phi. Unlike
        // `resolve_root_alias`, this deliberately follows every phi input: a mixed value is still
        // a possible namespace, and including its statically named member only widens the result.
        let mut namespace_locals = HashSet::new();
        loop {
            let mut changed = false;
            for (_, op) in graph.blocks.iter().flat_map(|block| &block.operations) {
                let GraphOp::StoreLocal { slot, value } = op else {
                    continue;
                };
                if namespace_alias_reaches(*value, &defs, &roots, &namespace_locals) {
                    changed |= namespace_locals.insert(*slot);
                }
            }
            if !changed {
                break;
            }
        }
        let is_namespace = |value| namespace_alias_reaches(value, &defs, &roots, &namespace_locals);
        for block in &graph.blocks {
            for (_, op) in &block.operations {
                let namespace_operands = op
                    .operands()
                    .into_iter()
                    .filter(|value| is_namespace(*value))
                    .collect::<Vec<_>>();
                if namespace_operands.is_empty() {
                    continue;
                }
                match op {
                    GraphOp::GetProperty { object, name, .. }
                    | GraphOp::CallMethod { object, name, .. }
                        if is_namespace(*object) =>
                    {
                        let Some(Constant::String(name)) = program.constants.get(*name as usize)
                        else {
                            return None;
                        };
                        let Some(name) = name.to_scalar_string().ok() else {
                            return None;
                        };
                        members.insert(name);
                    }
                    GraphOp::CheckTdz { .. } | GraphOp::StoreLocal { .. } | GraphOp::Phi { .. } => {
                    }
                    _ => return None,
                }
            }
            let escapes = match &block.terminator {
                Terminator::Branch { condition, .. }
                | Terminator::BranchNotNullish { condition, .. } => is_namespace(*condition),
                Terminator::Return(Some(value)) | Terminator::Throw(value) => is_namespace(*value),
                Terminator::Suspend { value, spilled, .. } => {
                    is_namespace(*value) || spilled.iter().any(|value| is_namespace(*value))
                }
                Terminator::Jump(_) | Terminator::Return(None) | Terminator::Unreachable => false,
            };
            if escapes {
                return None;
            }
        }
    }
    Some(members)
}

#[cfg(test)]
mod namespace_batch_tests {
    use super::*;

    #[test]
    fn independent_namespace_slots_keep_named_reads_and_escapes_separate() {
        let program = Program {
            source: "".into(),
            version: kiln_ir::BYTECODE_VERSION,
            source_origin: None,
            static_source_identity: None,
            static_source_provenance: None,
            constants: vec![
                Constant::String("alpha".into()),
                Constant::String("beta".into()),
            ],
            code: vec![
                Instruction::LoadLocal(0),
                Instruction::GetProperty(0),
                Instruction::Pop,
                Instruction::LoadLocal(1),
                Instruction::GetProperty(1),
                Instruction::Pop,
                Instruction::LoadLocal(2),
                Instruction::Return,
            ],
            local_count: 3,
            functions: Vec::new(),
            template_sites: Vec::new(),
            debug_call_sites: Vec::new(),
        };
        let expected = [
            (0, Some(["alpha".to_owned()].into_iter().collect())),
            (1, Some(["beta".to_owned()].into_iter().collect())),
            (2, None),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            closed_world_namespace_members_for_slots(&program, [2, 0, 1, 0], false),
            expected
        );
        assert_eq!(
            closed_world_namespace_members_for_slots(&program, [0, 1, 2], true),
            [(0, None), (1, None), (2, None)].into_iter().collect()
        );
        assert!(closed_world_namespace_members_for_slots(&program, [], false).is_empty());
    }
}
