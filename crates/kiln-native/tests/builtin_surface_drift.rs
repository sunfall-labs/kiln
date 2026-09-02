//! The two engines' built-in surfaces, compared without running either of them.
//!
//! Kiln has two engines and they used to write the built-in surface down twice: once in
//! `kiln-interpreter`'s Rust, once as hand-written `kiln_c99_same_name` chains inside
//! `kiln-backend-c`'s emitted C prelude. Nothing compared the two. When they disagreed the native
//! backend refused *at runtime*, and only when a program happened to read that member -- so gaps
//! surfaced one npm bundle at a time, in whatever order real code touched them. `Number` had
//! fifteen statics on one side and five on the other; `Number.MAX_VALUE` is the first property a
//! bundled `zod` reads, so its absence refused the whole module.
//!
//! ADR 0043 makes each engine declare its surface once and puts the comparison here. Both halves
//! are Rust tables, so this test consults them directly: it compiles no C and runs no JavaScript,
//! which is what lets it name *every* difference at once. That matters more than it sounds.
//! Walking a real program until it refuses reports the first gap and hides everything behind it:
//! `Object` answers ten members and then refuses on `getOwnPropertySymbols`, so `getPrototypeOf`,
//! `groupBy`, `hasOwn`, `is` and every name alphabetically later were never reached at all.
//!
//! It also catches the failure mode a refusal-based probe cannot see. `Array[Symbol.species]` read
//! `undefined` natively where the interpreter answers a function: no refusal, no diagnostic, the
//! program simply took the other branch. `typeof X === "undefined"` is how real code
//! feature-detects, so that is the shape which reaches production silently. Symbol-keyed members
//! are spelled `@@name` on both sides and are compared like any other.

use std::collections::BTreeSet;

/// A member the interpreter has and the native backend does not, with the reason it is still open.
///
/// This is the backlog, and it is a list rather than a count on purpose: a test that says "forty
/// gaps" teaches people to ignore it, and one that names them is a work queue. Closing a gap means
/// deleting its row here -- the test fails on a row that is no longer true, so the list cannot rot
/// into a description of a past that has been fixed.
struct KnownGap {
    global: &'static str,
    member: &'static str,
    why: &'static str,
}

const KNOWN_GAPS: &[KnownGap] = &[
    KnownGap {
        global: "Object",
        member: "groupBy",
        why: "not implemented natively; needs an iterator walk keyed by a callback's result",
    },
    KnownGap {
        global: "Object",
        member: "preventExtensions",
        why: "extensibility is not modelled natively; only freezing is",
    },
    KnownGap {
        global: "Object",
        member: "isExtensible",
        why: "extensibility is not modelled natively; only freezing is",
    },
    KnownGap {
        global: "Object",
        member: "seal",
        why: "sealing is weaker than freezing; answering freeze would be a silent wrong answer",
    },
    KnownGap {
        global: "Object",
        member: "isSealed",
        why: "sealing is weaker than freezing; answering isFrozen would be a silent wrong answer",
    },
    KnownGap {
        global: "Array",
        member: "fromAsync",
        why: "not implemented natively; needs the async-iterator protocol",
    },
    KnownGap {
        global: "String",
        member: "fromCharCode",
        why: "not implemented natively; builds a string from code units",
    },
    KnownGap {
        global: "String",
        member: "fromCodePoint",
        why: "not implemented natively; builds a string from scalar values",
    },
    KnownGap {
        global: "Promise",
        member: "try",
        why: "not implemented natively",
    },
    KnownGap {
        global: "JSON",
        member: "rawJSON",
        why: "not implemented natively; the raw-JSON marker has no native representation",
    },
    KnownGap {
        global: "JSON",
        member: "isRawJSON",
        why: "not implemented natively; the raw-JSON marker has no native representation",
    },
    KnownGap {
        global: "Reflect",
        member: "defineProperty",
        why: "answers a boolean where Object.defineProperty answers the target; not the same code",
    },
    KnownGap {
        global: "Reflect",
        member: "getPrototypeOf",
        why: "not implemented natively",
    },
    KnownGap {
        global: "Reflect",
        member: "setPrototypeOf",
        why: "answers a boolean where Object.setPrototypeOf answers the target; not the same code",
    },
    KnownGap {
        global: "Reflect",
        member: "isExtensible",
        why: "extensibility is not modelled natively",
    },
    KnownGap {
        global: "Reflect",
        member: "preventExtensions",
        why: "extensibility is not modelled natively",
    },
    KnownGap {
        global: "Error",
        member: "isError",
        why: "not implemented natively",
    },
    KnownGap {
        global: "RegExp",
        member: "escape",
        why: "not implemented natively",
    },
    KnownGap {
        global: "BigInt",
        member: "asIntN",
        why: "not implemented natively; needs a width-truncating BigInt provider call",
    },
    KnownGap {
        global: "BigInt",
        member: "asUintN",
        why: "not implemented natively; needs a width-truncating BigInt provider call",
    },
    KnownGap {
        global: "Map",
        member: "groupBy",
        why: "not implemented natively; needs an iterator walk keyed by a callback's result",
    },
    KnownGap {
        global: "Math",
        member: "f16round",
        why: "not implemented natively; needs a half-precision round the provider does not export",
    },
    KnownGap {
        global: "Buffer",
        member: "constants",
        why: "not implemented natively; the namespace object has no native representation",
    },
];

/// Globals the interpreter tabulates that the native backend does not put through the one table at
/// all, and the reason each is still on its own path.
///
/// Named rather than skipped. A global that is absent from both lists is unchecked, and unchecked
/// is exactly the state this whole seam exists to abolish; so a global here is a declaration that
/// somebody looked, not that nobody did.
const UNCONVERTED: &[(&str, &str)] = &[
    (
        "Iterator",
        "refused by name: KILN_C99_BUILTIN_ITERATOR is in the unavailable-globals range, which \
         publishes the right `typeof` and refuses every member",
    ),
    (
        "Atomics",
        "refused by name: shared memory has no native representation, and KILN_C99_METHOD_\
         ATOMICS_UNAVAILABLE refuses every member rather than answering one",
    ),
];

/// Globals the native table speaks for that the interpreter does not tabulate.
///
/// Not drift in either direction -- the interpreter answers these members, it just does not
/// publish them through `intrinsic_tabulated_property_names`, so there is nothing to diff against.
const UNTABULATED_BY_INTERPRETER: &[&str] = &["AbortSignal"];

fn interpreter_members(global: &str) -> BTreeSet<&'static str> {
    kiln_interpreter::intrinsic_static_members(global)
        .unwrap_or_else(|| panic!("`{global}` is listed in INTRINSIC_SURFACE_GLOBALS"))
        .iter()
        .copied()
        .collect()
}

fn native_members(global: &str) -> BTreeSet<&'static str> {
    kiln_backend_c::builtin_surface_members(global)
        .into_iter()
        .collect()
}

/// Every difference between the two surfaces, named, with the backlog held to exactly its rows.
#[test]
fn the_two_engines_declare_the_same_builtin_surface() {
    let expected: BTreeSet<(&str, &str)> = KNOWN_GAPS
        .iter()
        .map(|gap| (gap.global, gap.member))
        .collect();
    let unconverted: BTreeSet<&str> = UNCONVERTED.iter().map(|(global, _)| *global).collect();

    let mut regressed: Vec<String> = Vec::new();
    let mut invented: Vec<String> = Vec::new();
    let mut closed: Vec<String> = Vec::new();

    for global in kiln_interpreter::INTRINSIC_SURFACE_GLOBALS {
        if unconverted.contains(global) {
            assert!(
                native_members(global).is_empty(),
                "`{global}` is listed as unconverted but the native table has rows for it; either \
                 finish the conversion and drop the UNCONVERTED row, or drop the table rows"
            );
            continue;
        }
        // A global with no native rows at all is not skipped here. Every one of its members lands
        // in the difference below, so it either has a KNOWN_GAPS row apiece or this test fails
        // naming them. `RegExp`, `BigInt` and `Map` are that shape today: their whole tabulated
        // surface is one or two statics nobody has implemented natively.
        let interpreted = interpreter_members(global);
        let native = native_members(global);
        for member in interpreted.difference(&native) {
            if !expected.contains(&(global, member)) {
                regressed.push(format!("{global}.{member}"));
            }
        }
        for member in native.difference(&interpreted) {
            invented.push(format!("{global}.{member}"));
        }
    }

    for gap in KNOWN_GAPS {
        if unconverted.contains(gap.global) {
            closed.push(format!(
                "{}.{} -- its global is listed as unconverted, so the gap is redundant",
                gap.global, gap.member
            ));
            continue;
        }
        if native_members(gap.global).contains(gap.member) {
            closed.push(format!(
                "{}.{} -- the native table answers it now; delete the KNOWN_GAPS row",
                gap.global, gap.member
            ));
        } else if !interpreter_members(gap.global).contains(gap.member) {
            closed.push(format!(
                "{}.{} -- the interpreter no longer has it; delete the KNOWN_GAPS row",
                gap.global, gap.member
            ));
        }
    }

    let mut report = String::new();
    if !regressed.is_empty() {
        report.push_str(&format!(
            "\nthe interpreter has {} member(s) the native backend does not, and none of them is \
             in KNOWN_GAPS.\nAdd the member to `BUILTIN_SURFACE` in kiln-backend-c, or add a \
             KNOWN_GAPS row saying why it is still open:\n  {}\n",
            regressed.len(),
            regressed.join("\n  ")
        ));
    }
    if !invented.is_empty() {
        report.push_str(&format!(
            "\nthe native backend answers {} member(s) the interpreter does not declare. A member \
             only one engine has is drift whichever engine has it:\n  {}\n",
            invented.len(),
            invented.join("\n  ")
        ));
    }
    if !closed.is_empty() {
        report.push_str(&format!(
            "\n{} KNOWN_GAPS row(s) are no longer true. A stale backlog is worse than none:\n  \
             {}\n",
            closed.len(),
            closed.join("\n  ")
        ));
    }
    assert!(report.is_empty(), "{report}");
}

/// The native table's own globals are accounted for, so nothing is compared by accident.
#[test]
fn every_natively_tabled_global_is_either_diffed_or_named() {
    let tabulated: BTreeSet<&str> = kiln_interpreter::INTRINSIC_SURFACE_GLOBALS
        .iter()
        .copied()
        .collect();
    let untabulated: BTreeSet<&str> = UNTABULATED_BY_INTERPRETER.iter().copied().collect();
    let mut unaccounted: Vec<&str> = Vec::new();
    for global in kiln_backend_c::builtin_surface_owners() {
        if !tabulated.contains(global) && !untabulated.contains(global) {
            unaccounted.push(global);
        }
    }
    assert!(
        unaccounted.is_empty(),
        "the native table speaks for {unaccounted:?}, which the interpreter neither tabulates nor \
         is recorded as not tabulating. Either add it to INTRINSIC_SURFACE_GLOBALS so the two are \
         compared, or to UNTABULATED_BY_INTERPRETER to record that there is nothing to compare."
    );
    for global in &untabulated {
        assert!(
            kiln_interpreter::intrinsic_static_members(global).is_none(),
            "`{global}` is recorded as untabulated by the interpreter, but the interpreter now \
             tabulates it. Move it into INTRINSIC_SURFACE_GLOBALS so it is diffed."
        );
    }
}

/// A backlog row must name a member of a global one of the two engines actually has.
#[test]
fn the_backlog_names_real_members() {
    for gap in KNOWN_GAPS {
        assert!(
            !gap.why.is_empty(),
            "`{}.{}` has no reason; a gap without one is indistinguishable from an oversight",
            gap.global,
            gap.member
        );
        assert!(
            kiln_interpreter::INTRINSIC_SURFACE_GLOBALS.contains(&gap.global),
            "`{}` is not a global the interpreter tabulates, so `{}.{}` cannot be a gap between \
             the two engines",
            gap.global,
            gap.global,
            gap.member
        );
    }
}
