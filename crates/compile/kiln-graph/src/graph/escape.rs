use super::*;

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
            instruction: Instruction::DefineNamedProperty { .. },
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
pub(super) fn escape_analysis_with(
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
                delegate_wrapped,
                ..
            } => {
                live_values.insert(*value);
                if let Some((iter, _, _)) = delegate {
                    live_values.insert(*iter);
                }
                live_values.extend(delegate_wrapped.iter().copied());
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
                    | GraphOp::SetPropertyStrict { object, value, .. }
                    | GraphOp::DefineOwnProperty { object, value, .. } => {
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
                    }
                    | GraphOp::DefineOwnComputed {
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
                delegate_wrapped,
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
                if let Some(&root) = delegate_wrapped
                    .as_ref()
                    .and_then(|wrapped| alias_to_root.get(wrapped))
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
                }
                | GraphOp::DefineOwnProperty {
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
                }
                | GraphOp::DefineOwnComputed {
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
                    | GraphOp::DefineOwnProperty { object, name, .. }
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
                    | GraphOp::DefineOwnComputed { object, key, .. }
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
                    }
                    | GraphOp::DefineOwnProperty {
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
                    }
                    | GraphOp::DefineOwnComputed {
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
