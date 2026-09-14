use super::EntryKind;
use crate::direct::{CallPlan, direct_callable};
use crate::graph::{FunctionGraph, GraphOp, ValueId};
use kiln_ir::{Constant, Instruction, Program};
use std::collections::HashSet;

/// The protos this unit defines as a method of `key` -- `class C { key() {} }` and
/// `{ key() {} }` -- read off the bytecode as `Const(key) MakeFunction(i) DefineMethod`. A hint
/// only: a method-call site whose cached callee's body is one of these calls it by name, which
/// lets the C compiler inline it; a callee that is none of them takes the indirect call. Nothing
/// is assumed about which closure a site sees.
pub(crate) fn method_candidates(
    program: &Program,
    key: &str,
    publishes_frame: &[bool],
) -> Vec<u32> {
    let mut found = Vec::new();
    let mut scan = |code: &[Instruction]| {
        for window in code.windows(3) {
            if let [
                Instruction::Const(k),
                Instruction::MakeFunction(i),
                Instruction::DefineMethod,
            ] = window
                && matches!(program.constants.get(*k as usize), Some(Constant::String(s)) if s == key)
                && direct_callable(
                    &program.functions[*i as usize],
                    publishes_frame[*i as usize],
                )
                && !found.contains(i)
            {
                found.push(*i);
            }
        }
    };
    scan(&program.code);
    for proto in &program.functions {
        scan(&proto.code);
    }
    found
}

/// A function's inline-cache sites, shared by every clone of it. Each operation declares its
/// site inline as `static kiln_c99_site site`; a clone that declared its own would start
/// empty and, having no slow path to learn with, stay empty. So the sites are hoisted to file
/// scope, named per function and numbered in emission order -- the same order in every clone
/// -- and only the boxed clone, emitted first, declares them. Returns the body with the sites
/// renamed and how many there were.
pub(super) fn share_sites(body: &str, prefix: &str) -> (String, usize) {
    const DECLARATIONS: &[&str] = &[
        "static kiln_c99_site site = {0}; ",
        "static kiln_c99_site site = {KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}; ",
    ];
    let mut out = String::with_capacity(body.len());
    let mut count = 0usize;
    for line in body.split_inclusive('\n') {
        if let Some((at, declaration)) = DECLARATIONS
            .iter()
            .find_map(|declaration| line.find(declaration).map(|at| (at, *declaration)))
        {
            let name = format!("{prefix}[{count}]");
            count += 1;
            let stripped = format!("{}{}", &line[..at], &line[at + declaration.len()..]);
            // `site` as a token of its own: `site.`, `&site`, `(site` -- never inside an
            // identifier such as `kiln_c99_site` or `diagnostic_site`.
            let mut renamed = String::with_capacity(stripped.len() + 64);
            let bytes = stripped.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                if stripped[i..].starts_with("site")
                    && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
                    && !bytes
                        .get(i + 4)
                        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                {
                    renamed.push_str(&name);
                    i += 4;
                } else {
                    let ch = stripped[i..].chars().next().unwrap();
                    renamed.push(ch);
                    i += ch.len_utf8();
                }
            }
            out.push_str(&renamed);
        } else {
            out.push_str(line);
        }
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::share_sites;

    #[test]
    fn zero_backed_sites_are_shared_without_renaming_other_identifiers() {
        let body = "{ static kiln_c99_site site = {0}; use(&site, site.pic_count, diagnostic_site); }\n\
                    { static kiln_c99_site site = {KILN_C99_SITE_NONE, KILN_C99_SITE_NONE, 0}; use(&site); }\n";
        let (shared, count) = share_sites(body, "fn_0_site");
        assert_eq!(count, 2);
        assert!(!shared.contains("static kiln_c99_site"));
        assert!(shared.contains("use(&fn_0_site[0], fn_0_site[0].pic_count, diagnostic_site)"));
        assert!(shared.contains("use(&fn_0_site[1])"));
    }
}

/// The reads of `this` in a body, each with its column in the function's receiver-shape
/// cache: one column per property name, in first-read order -- the same order in every
/// clone. Two reads of one name share a column because for one receiver of one shape they
/// are one slot, and the C compiler then reads it once.
pub(crate) fn this_columns(
    graph: &FunctionGraph,
) -> (std::collections::HashMap<ValueId, usize>, Vec<u32>) {
    let mut names: Vec<u32> = Vec::new();
    let columns = graph
        .blocks
        .iter()
        .flat_map(|b| b.operations.iter())
        .filter_map(|(vid, op)| match op {
            GraphOp::GetProperty { object, name, .. }
                if graph
                    .blocks
                    .iter()
                    .flat_map(|b| b.operations.iter())
                    .any(|(v, def)| v == object && matches!(def, GraphOp::LoadThis)) =>
            {
                let column = names.iter().position(|n| n == name).unwrap_or_else(|| {
                    names.push(*name);
                    names.len() - 1
                });
                Some((*vid, column))
            }
            _ => None,
        })
        .collect();
    (columns, names)
}

/// How many columns the receiver-shape cache of a body has.
pub(crate) fn this_site_count(graph: &FunctionGraph) -> usize {
    this_columns(graph).1.len()
}

/// The receiver-shape cache's row for the receiver, found or claimed at entry, before any
/// dispatch that could jump past it: a resumed generator lands at its landing with this
/// already run. NULL when the receiver is not an object, or its shape has no row.
pub(super) fn receiver_prologue(
    symbol: &str,
    index: usize,
    columns: &[u32],
    kind: EntryKind,
    double_mask: u32,
) -> String {
    let keys: Vec<String> = columns
        .iter()
        .map(|name| format!("{symbol}_key_{name}"))
        .collect();
    let count = columns.len();
    // A typed entry takes the row from a caller that has it, and looks one up only when the
    // caller had none: the site remembers the callee's row for the receiver's shape.
    let (init, lookup, shapes_var, refused_var, slot_var, check_guard) =
        if matches!(kind, EntryKind::Typed { .. }) {
            (
                "recv_row_in",
                "if (recv_row == NULL) ",
                format!("{symbol}_fn_{index}_typed_recv_shape"),
                format!("{symbol}_fn_{index}_typed_recv_refused"),
                format!("{symbol}_fn_{index}_typed_recv_slot"),
                "    if (KILN_UNLIKELY(recv_row == NULL || recv_obj == NULL)) goto kiln_refusal;\n",
            )
        } else {
            (
                "NULL",
                "",
                format!("{symbol}_fn_{index}_recv_shape"),
                format!("{symbol}_fn_{index}_recv_refused"),
                format!("{symbol}_fn_{index}_recv_slot"),
                "",
            )
        };
    format!(
        "    const uint32_t *recv_row = {init}; const kiln_object *recv_obj = NULL; if (KILN_VAL_IS_OBJECT(receiver)) {{ recv_obj = (const kiln_object *)KILN_VAL_PTR(receiver); if (recv_obj == NULL) __builtin_unreachable(); {lookup}{{ recv_row = kiln_c99_recv_row({shapes_var}, &{slot_var}[0][0], {count}, recv_obj->shape); if (recv_row == NULL && !kiln_c99_recv_refused({refused_var}, recv_obj->shape) && kiln_c99_gc_kind_of_ptr(recv_obj) == KILN_GC_OBJECT) {{ const char *const recv_keys[{count}] = {{ {} }}; recv_row = kiln_c99_recv_claim({shapes_var}, {refused_var}, &{slot_var}[0][0], {count}, recv_keys, recv_obj, {double_mask}u); }} }} }} (void)recv_row; (void)recv_obj;\n{check_guard}",
        keys.join(", ")
    )
}

pub(crate) fn collect_typed_call_sites(
    graph: &FunctionGraph,
    program: &Program,
    plan: &CallPlan,
    numbers: &HashSet<ValueId>,
) -> Vec<(ValueId, Vec<u32>)> {
    let mut sites = Vec::new();
    for block in &graph.blocks {
        for (vid, op) in &block.operations {
            if let GraphOp::CallMethod { name, args, .. } = op {
                let key_str = match program.constants.get(*name as usize) {
                    Some(Constant::String(s)) => s.as_str(),
                    _ => "",
                };
                let argc = args.len();
                let candidates = method_candidates(program, key_str, &plan.publishes_frame)
                    .into_iter()
                    .filter(|&index| plan.available.get(index as usize).copied().unwrap_or(true))
                    .collect::<Vec<_>>();
                let typed_candidates: Vec<u32> = candidates
                    .iter()
                    .copied()
                    .filter(|&i| {
                        let callee = &program.functions[i as usize];
                        plan.typed_entries.get(i as usize).copied().unwrap_or(false)
                            && !callee.arrow
                            && callee.parameter_count as usize == argc
                            && args.iter().all(|a| numbers.contains(a))
                    })
                    .collect();
                if !typed_candidates.is_empty() {
                    sites.push((*vid, typed_candidates));
                }
            }
        }
    }
    sites
}
