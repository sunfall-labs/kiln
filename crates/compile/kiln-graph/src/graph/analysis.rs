use super::*;

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
    // A remainder takes the sign of its dividend, so an integer remainder of a negative dividend
    // may be `-0` (`-4 % 2`), which an `int64_t` twin cannot hold: every consumer reading the
    // twin as a double would see `+0` (issue 20). The remainder is an `Int` only when the
    // dividend is proven non-negative; otherwise it is a `Number`, whose box and double carry
    // the sign.
    let defs: HashMap<ValueId, &GraphOp> = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter().map(|(v, op)| (*v, op)))
        .collect();
    let is_non_negative = |v: &ValueId| non_negative_int(*v, &defs, &ivs, 8);
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
                        Constant::BigInt(_) | Constant::WellKnownSymbol(_) => {
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
                            if int(&state, left)
                                && int(&state, right)
                                && is_non_zero(right)
                                && is_non_negative(left)
                            {
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
                        UnaryOp::TypeOf | UnaryOp::ToString => Some(Proven::String),
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

/// Whether `value`, where it is an integer, is certainly `>= 0` (and so never `-0`): a
/// non-negative constant, a non-negative induction variable, `x >>> n`, `x & c` with a constant
/// `c` in `[0, 2^31)`, the sum of two such values, a remainder of one, or a phi of them.
/// `depth` bounds the walk through phis; running out answers "not proven".
fn non_negative_int(
    value: ValueId,
    defs: &HashMap<ValueId, &GraphOp>,
    ivs: &HashMap<ValueId, InductionVariable>,
    depth: u32,
) -> bool {
    if ivs
        .get(&value)
        .is_some_and(|iv| iv.non_negative && iv.min >= 0)
    {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let recurse = |v: &ValueId| non_negative_int(*v, defs, ivs, depth - 1);
    let small_non_negative_const = |v: &ValueId| {
        matches!(defs.get(v), Some(GraphOp::Const { constant, .. })
            if as_safe_integer(constant).is_some_and(|n| (0..=i64::from(i32::MAX)).contains(&n)))
    };
    match defs.get(&value) {
        Some(GraphOp::Const { constant, .. }) => as_safe_integer(constant).is_some_and(|n| n >= 0),
        Some(GraphOp::Binary { op, left, right }) => match op {
            BinaryOp::UnsignedShiftRight => true,
            BinaryOp::BitwiseAnd => {
                small_non_negative_const(left) || small_non_negative_const(right)
            }
            BinaryOp::Add => recurse(left) && recurse(right),
            BinaryOp::Remainder => recurse(left),
            _ => false,
        },
        Some(GraphOp::Phi { inputs }) => {
            !inputs.is_empty() && inputs.iter().all(|(_, input)| recurse(input))
        }
        Some(GraphOp::StoreLocal { value, .. } | GraphOp::CheckTdz { value, .. }) => recurse(value),
        _ => false,
    }
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
        // A direct-eval call carries the same callee/argument stack shape as a normal call.
        // Spread calls materialize their arguments as one array before this instruction.
        Instruction::DirectEval(site) => (usize::from(site.argc.unwrap_or(1)) + 1, 1),
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
        | Instruction::DefineNamedProperty { .. }
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
