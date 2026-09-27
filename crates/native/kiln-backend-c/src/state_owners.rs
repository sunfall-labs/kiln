//! Who owns each kind of mutable static the emitters write (ADR 0070, realms issue 04 step 5).
//!
//! The prelude's own statics are classified by `C99_MUTABLE_STATE_OWNERS` in `prelude.rs`. This is
//! the same manifest for what the *emitters* write into a unit or a launcher: every mutable static
//! must match a row here, and there is no row for realm state. Realm state is not a static: a
//! unit's module globals and template sites are its realm block (`kiln_c99_unit`), a launcher's
//! module linkage is its realm's graph context. A new kind of emitted static that holds a realm's
//! values therefore has nowhere to go but a new row, and a reviewer reading the row's owner.
//!
//! Names are matched after [`normalize_static_name`]: the unit's symbol becomes `{sym}` and every
//! run of digits becomes `N`, so one row covers `{sym}_fn_3_site` and `{sym}_fn_12_site` alike.

/// Why a mutable emitted static is allowed to be one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmittedStateOwner {
    /// An inline cache stated in realm-independent terms (ADR 0071): layouts, paths and code,
    /// never a realm's objects, so one process-wide copy serves every realm. Realms on several
    /// threads write it, so each answer is a slot published by one release store of its key word
    /// and never rewritten under a reader (`KILN_C99_SLOT_*` in the prelude, issue 04 step 4).
    Cache,
    /// Per thread (`_Thread_local`): a cache trusted only under the realm generation it was filled
    /// in, or scratch a thread consumes before it runs another realm.
    ThreadCache,
    /// Belongs to the process: a unit's identity (its block id), a registration that happened
    /// once before any realm, speculation and profiling counters that describe the code.
    Process,
    /// Built once from constants of the program and never changed again.
    Constant,
}

impl EmittedStateOwner {
    /// Whether the owner requires the declaration to be `_Thread_local` (or `__thread`).
    pub fn per_thread(self) -> bool {
        matches!(self, Self::ThreadCache)
    }
}

/// Every kind of mutable static the backend and the native launchers emit, by normalized name.
pub const EMITTED_MUTABLE_STATE_OWNERS: &[(&str, EmittedStateOwner)] = &[
    // -- a unit's realm block: its descriptor and this thread's cache of the block ------------
    ("{sym}_unit", EmittedStateOwner::Process),
    ("{sym}_unit_cached", EmittedStateOwner::ThreadCache),
    // -- constant keys (step 1b): registered once per process before any realm; a unit loaded
    //    after a realm exists holds one live realm's keys at a time, and `_keys_owner` says
    //    which (issue 04 step 4): a second live realm entering it is refused ------------------
    ("{sym}_key_N", EmittedStateOwner::Process),
    ("{sym}_keys", EmittedStateOwner::Process),
    ("{sym}_keys_owner", EmittedStateOwner::Process),
    ("{sym}_keys_shared", EmittedStateOwner::Process),
    // -- BigInt literal handles: provider handles made once from constant digits --------------
    ("{sym}_bigint_N", EmittedStateOwner::Constant),
    // -- the shared immutable region (realms issue 08): each site's closure, written once by the
    //    realm that builds the region before it is published sealed, read by every realm after -
    ("{sym}_region_sites", EmittedStateOwner::Process),
    // -- inline caches (ADR 0071, step 3) -------------------------------------------------------
    ("site", EmittedStateOwner::Cache),
    ("{sym}_sites", EmittedStateOwner::Cache),
    ("{sym}_fn_N_site", EmittedStateOwner::Cache),
    ("kiln_dnp_site", EmittedStateOwner::Cache),
    ("{sym}_fn_N_recv_shape", EmittedStateOwner::Cache),
    ("{sym}_fn_N_recv_refused", EmittedStateOwner::Cache),
    ("{sym}_fn_N_recv_slot", EmittedStateOwner::Cache),
    ("{sym}_fn_N_typed_recv_shape", EmittedStateOwner::Cache),
    ("{sym}_fn_N_typed_recv_refused", EmittedStateOwner::Cache),
    ("{sym}_fn_N_typed_recv_slot", EmittedStateOwner::Cache),
    // Step 3b's per-candidate method path: receiver shape, prototype shape, row, slot -- shared
    // shapes only, so one process-wide copy serves every realm; one slot keyed by the receiver.
    ("{sym}_fn_N_vN_t_N_path", EmittedStateOwner::Cache),
    ("{sym}_fn_entry_vN_t_N_path", EmittedStateOwner::Cache),
    ("lit_shape", EmittedStateOwner::Cache),
    ("kiln_sp_shape_N", EmittedStateOwner::Cache),
    ("kiln_sp_slot_N", EmittedStateOwner::Cache),
    // -- speculation guards and type profiles: counters about the code, not any realm's data,
    //    moved with atomic read-modify-writes so no realm's bits or counts are lost ------------
    ("kiln_loop_guard_N", EmittedStateOwner::Process),
    ("kiln_guard_entry_N", EmittedStateOwner::Process),
    ("{sym}_fn_N_param_tags", EmittedStateOwner::Process),
    ("{sym}_fn_N_return_tag", EmittedStateOwner::Process),
    ("{sym}_profile_dumped", EmittedStateOwner::Process),
    // -- the graph launcher (kiln-native) ------------------------------------------------------
    // Its module linkage is the realm's graph context; these are what is left.
    ("text", EmittedStateOwner::ThreadCache),
    ("string", EmittedStateOwner::ThreadCache),
    ("undefined", EmittedStateOwner::Constant),
    ("kiln_sup_stop_signal", EmittedStateOwner::Process),
    // The `KILN_WORKERS` supervisor's self-pipe: the supervising process's, before any realm.
    ("kiln_sup_pipe", EmittedStateOwner::Process),
    // `KILN_WORKERS` thread mode (realms issue 07): the worker index of the realm a thread runs,
    // set on that thread before its realm exists; the count and the stop signals the waiting
    // thread takes, both written once before any realm thread starts.
    ("kiln_cli_thread_worker", EmittedStateOwner::ThreadCache),
    ("kiln_cli_thread_workers", EmittedStateOwner::Process),
    ("kiln_threads_stop_signals", EmittedStateOwner::Process),
    // Library paths a dynamic `@opentui/core` import tries: string literals, never written.
    ("opentui_candidates", EmittedStateOwner::Constant),
    // The shared-library wrapper: this thread's pointers into the realm context of the body
    // that is running, set at the body's start and cleared at its end.
    ("kiln_shared_namespaces", EmittedStateOwner::ThreadCache),
    ("kiln_shared_commonjs", EmittedStateOwner::ThreadCache),
    (
        "kiln_shared_opentui_namespace",
        EmittedStateOwner::ThreadCache,
    ),
    (
        "kiln_shared_node_ffi_namespace",
        EmittedStateOwner::ThreadCache,
    ),
];

/// One mutable static found in emitted C: its name and the whole declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutableStatic {
    pub name: String,
    pub declaration: String,
}

/// Every mutable `static` declaration in `source`, at file or function scope, by statement.
///
/// A declaration is mutable unless it is `static const` of a non-pointer, or a `* const`
/// pointer. Function definitions and prototypes are skipped; so is anything whose name is a
/// `kiln_c99_` prelude name, which `C99_MUTABLE_STATE_OWNERS` classifies.
pub fn mutable_statics(source: &str) -> Vec<MutableStatic> {
    let mut statements: Vec<String> = Vec::new();
    let mut pending = String::new();
    for line in source.lines() {
        let line = line.trim();
        if pending.is_empty() {
            if !line.starts_with("static ") {
                continue;
            }
            pending.push_str(line);
        } else {
            pending.push(' ');
            pending.push_str(line);
        }
        let head = pending.split('=').next().unwrap_or(&pending);
        if head.contains('(') {
            // A function: emitted code declares no function-pointer statics.
            pending.clear();
        } else if pending.ends_with(';') {
            statements.push(std::mem::take(&mut pending));
        } else if pending.ends_with('{') && !pending.contains('=') {
            pending.clear();
        }
    }
    let mut found = Vec::new();
    for statement in statements {
        for piece in statement.split_inclusive(';') {
            let piece = piece.trim();
            if !piece.starts_with("static ") || !piece.ends_with(';') {
                continue;
            }
            let head = piece
                .split('=')
                .next()
                .unwrap_or(piece)
                .trim_end_matches(';')
                .trim();
            let words: Vec<&str> = head
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .filter(|w| !w.is_empty())
                .collect();
            let Some(name) = words
                .iter()
                .rev()
                .find(|w| !w.chars().next().is_some_and(|c| c.is_ascii_digit()))
            else {
                continue;
            };
            let before = &head[..head.rfind(name).unwrap_or(0)];
            let immutable = if before.contains('*') {
                before
                    .split_whitespace()
                    .collect::<String>()
                    .ends_with("*const")
            } else {
                before.split_whitespace().any(|w| w == "const")
            };
            if immutable || name.starts_with("kiln_c99_") {
                continue;
            }
            found.push(MutableStatic {
                name: (*name).to_owned(),
                declaration: piece.to_owned(),
            });
        }
    }
    found
}

/// `name` with each of `symbols` (longest first) replaced by `{sym}` and each digit run by `N`.
pub fn normalize_static_name(name: &str, symbols: &[&str]) -> String {
    let mut symbols = symbols.to_vec();
    symbols.sort_by_key(|symbol| std::cmp::Reverse(symbol.len()));
    let mut name = name.to_owned();
    for symbol in symbols {
        if !symbol.is_empty() && name.starts_with(symbol) {
            name = format!("{{sym}}{}", &name[symbol.len()..]);
            break;
        }
    }
    let mut out = String::with_capacity(name.len());
    let mut in_digits = false;
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('N');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

/// The owner of the static named `name` in a source emitted for `symbols`, if it has one.
pub fn emitted_state_owner(name: &str, symbols: &[&str]) -> Option<EmittedStateOwner> {
    let normalized = normalize_static_name(name, symbols);
    EMITTED_MUTABLE_STATE_OWNERS
        .iter()
        .find(|(pattern, _)| *pattern == normalized)
        .map(|(_, owner)| *owner)
}

/// Every mutable static in `source` that has no owner, or whose per-thread owner it does not
/// honour, as a line a failing test can print.
pub fn unowned_emitted_statics(source: &str, symbols: &[&str]) -> Vec<String> {
    let mut problems = Vec::new();
    for found in mutable_statics(source) {
        match emitted_state_owner(&found.name, symbols) {
            None => problems.push(format!(
                "no owner for `{}` ({}): `{}`",
                found.name,
                normalize_static_name(&found.name, symbols),
                found.declaration
            )),
            Some(owner)
                if owner.per_thread()
                    && !found.declaration.contains("_Thread_local")
                    && !found.declaration.contains("__thread") =>
            {
                problems.push(format!(
                    "`{}` is {owner:?}, which is per thread, but is declared `{}`",
                    found.name, found.declaration
                ));
            }
            Some(_) => {}
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realm_state_in_a_static_has_no_owner() {
        let symbols = ["kiln_m_1"];
        for realm_state in [
            "static kiln_val kiln_m_1_globals[2];",
            "static kiln_val kiln_m_1_template_sites[1];",
            "static kiln_value kiln_namespaces[3];",
            "static kiln_value kiln_exports_0[2];",
            "static kiln_commonjs_context kiln_commonjs_4;",
        ] {
            assert_eq!(
                unowned_emitted_statics(realm_state, &symbols).len(),
                1,
                "`{realm_state}` must have no owner"
            );
        }
        let owned = "static kiln_c99_unit kiln_m_1_unit = { 0, 3 };\n\
                     static _Thread_local kiln_c99_unit_cache kiln_m_1_unit_cached;\n\
                     static kiln_c99_method_path kiln_m_1_fn_3_v7_t_0_path = KILN_C99_UNLEARNED_METHOD_PATH; static uint64_t kiln_m_1_keys_owner;\n\
                         static uint32_t kiln_loop_guard_12 = 0;\n\
                     static const char kiln_m_1_name[] = \"x\";\n\
                     static int kiln_m_1_body(kiln_host *host, kiln_value *result) {";
        assert_eq!(
            unowned_emitted_statics(owned, &symbols),
            Vec::<String>::new()
        );
        assert_eq!(mutable_statics(owned).len(), 5);
        // A per-thread owner is honoured only by a per-thread declaration.
        assert_eq!(
            unowned_emitted_statics("static kiln_c99_unit_cache kiln_m_1_unit_cached;", &symbols)
                .len(),
            1
        );
    }
}
