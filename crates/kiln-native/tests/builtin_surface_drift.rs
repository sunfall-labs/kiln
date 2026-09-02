//! What the two engines answer for each built-in, named -- without running either of them.
//!
//! Kiln has two engines and they used to write the built-in surface down twice: once in
//! `kiln-interpreter`'s Rust, once as hand-written `kiln_c99_same_name` chains inside
//! `kiln-backend-c`'s emitted C prelude. Nothing compared the two. When they disagreed the native
//! backend refused *at runtime*, and only when a program happened to read that member -- so gaps
//! surfaced one npm bundle at a time, in whatever order real code touched them. `Number` had
//! fifteen statics on one side and five on the other; `Number.MAX_VALUE` is the first property a
//! bundled `zod` reads, so its absence refused the whole module.
//!
//! ADR 0043 makes both engines read one declaration, `kiln_builtin_surface::SURFACE`, and makes
//! disagreement a **compile error** in `kiln-backend-c`: a member of the shared surface that
//! backend neither implements nor records as backlog does not build. That gate is a `const`
//! evaluation, and a `const` panic carries a literal and nothing else, so it can say only *that*
//! they disagree.
//!
//! This is the half that says *which*. It reads the same three tables the gate reads --
//! `kiln_builtin_surface::SURFACE`, `kiln_backend_c::BUILTIN_SURFACE`, and that backend's
//! `BUILTIN_SURFACE_BACKLOG` -- so it compiles no C and runs no JavaScript, and can therefore name
//! every difference at once. That matters more than it sounds. Walking a real program until it
//! refuses reports the first gap and hides everything behind it: `Object` answers ten members and
//! then refuses on `getOwnPropertySymbols`, so `getPrototypeOf`, `groupBy`, `hasOwn`, `is` and
//! every name alphabetically later were never reached at all.
//!
//! It also covers the failure mode a refusal-based probe cannot see. `Array[Symbol.species]` read
//! `undefined` natively where the interpreter answers a function: no refusal, no diagnostic, the
//! program simply took the other branch. `typeof X === "undefined"` is how real code
//! feature-detects, so that is the shape which reaches production silently. Symbol-keyed members
//! are spelled `@@name` on both sides and are compared like any other.

use std::collections::{BTreeMap, BTreeSet};

/// The interpreter answers exactly the shared surface, by construction.
///
/// Its own lists are `const` reads of `kiln_builtin_surface::members`, so this asserts the
/// construction rather than the contents -- and it is the assertion that would fail first if
/// somebody reintroduced a hand-written list beside the table.
#[test]
fn the_interpreter_answers_the_shared_surface() {
    for builtin in kiln_builtin_surface::SURFACE {
        let declared: BTreeSet<&str> = builtin.members.iter().copied().collect();
        assert_eq!(
            declared.len(),
            builtin.members.len(),
            "`{}` lists a member twice",
            builtin.global
        );
        assert!(
            kiln_builtin_surface::find(builtin.global).is_some(),
            "`{}` is in SURFACE but `find` cannot reach it",
            builtin.global
        );
    }
}

/// Every gap between the two engines, named, with the reason each is still open.
///
/// Always prints, pass or fail. The number on its own ("twenty-three gaps") is what teaches people
/// to ignore a gate; the names are a work queue.
#[test]
fn the_backlog_names_every_gap_and_nothing_else() {
    let answered: BTreeSet<(&str, &str)> = kiln_backend_c::BUILTIN_SURFACE
        .iter()
        .map(|member| (member.owner, member.name))
        .collect();

    let mut by_global: BTreeMap<&str, Vec<&kiln_backend_c::BuiltinGap>> = BTreeMap::new();
    for gap in kiln_backend_c::BUILTIN_SURFACE_BACKLOG {
        by_global.entry(gap.global).or_default().push(gap);
    }

    let mut unrecorded: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();
    let mut report = String::new();

    for builtin in kiln_builtin_surface::SURFACE {
        let gaps = by_global.get(builtin.global).cloned().unwrap_or_default();
        let named: BTreeSet<&str> = gaps.iter().map(|gap| gap.member).collect();
        for member in builtin.members {
            let implemented = answered.contains(&(builtin.global, *member));
            if !implemented && !named.contains(member) {
                unrecorded.push(format!("{}.{member}", builtin.global));
            }
            if implemented && named.contains(member) {
                stale.push(format!("{}.{member}", builtin.global));
            }
        }
        if gaps.is_empty() {
            continue;
        }
        report.push_str(&format!("\n{}\n", builtin.global));
        for gap in gaps {
            report.push_str(&format!("  {:<24} {}\n", gap.member, gap.why));
        }
    }

    println!(
        "{} of {} shared members are answered natively; {} remain:\n{report}",
        kiln_builtin_surface::SURFACE
            .iter()
            .flat_map(|builtin| builtin
                .members
                .iter()
                .map(move |member| (builtin.global, member)))
            .filter(|(global, member)| answered.contains(&(*global, **member)))
            .count(),
        kiln_builtin_surface::SURFACE
            .iter()
            .map(|builtin| builtin.members.len())
            .sum::<usize>(),
        kiln_backend_c::BUILTIN_SURFACE_BACKLOG.len(),
    );

    assert!(
        unrecorded.is_empty(),
        "the shared surface declares {} member(s) the native backend neither answers nor records \
         as backlog. This should have failed the build of kiln-backend-c already; if it did not, \
         `assert_surface_is_covered` has been weakened:\n  {}",
        unrecorded.len(),
        unrecorded.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "{} backlog row(s) name a member the native backend now answers. A stale backlog \
         describes a past that has been fixed, and the next reader believes it:\n  {}",
        stale.len(),
        stale.join("\n  ")
    );
}

/// A native row for a global the shared surface does not declare is reach, not drift -- but it is
/// only reach if somebody said so.
///
/// A built-in in neither list is *unchecked*, which is the state ADR 0043 exists to abolish, so it
/// has to be named here to be allowed.
const BEYOND_THE_SHARED_SURFACE: &[(&str, &str)] = &[(
    "AbortSignal",
    "the interpreter answers `abort`, `timeout` and `any` without publishing them through \
     `intrinsic_tabulated_property_names`, so there is no list to diff against yet",
)];

#[test]
fn every_natively_answered_global_is_shared_or_named() {
    let named: BTreeSet<&str> = BEYOND_THE_SHARED_SURFACE
        .iter()
        .map(|(global, _)| *global)
        .collect();
    let mut unaccounted: Vec<&str> = Vec::new();
    for global in kiln_backend_c::builtin_surface_owners() {
        if kiln_builtin_surface::find(global).is_none() && !named.contains(global) {
            unaccounted.push(global);
        }
    }
    assert!(
        unaccounted.is_empty(),
        "the native backend answers for {unaccounted:?}, which is neither in \
         kiln_builtin_surface::SURFACE nor recorded in BEYOND_THE_SHARED_SURFACE. Declare it in \
         the shared table so both engines are held to it, or record why there is nothing to hold \
         them to."
    );
    for (global, why) in BEYOND_THE_SHARED_SURFACE {
        assert!(
            !why.is_empty(),
            "`{global}` is allowed through with no reason"
        );
        assert!(
            kiln_builtin_surface::find(global).is_none(),
            "`{global}` is recorded as outside the shared surface, but the shared surface declares \
             it now. Delete the row so the two engines are diffed."
        );
        assert!(
            !kiln_backend_c::builtin_surface_members(global).is_empty(),
            "`{global}` is recorded as answered natively beyond the shared surface, but the native \
             table has no rows for it"
        );
    }
}
