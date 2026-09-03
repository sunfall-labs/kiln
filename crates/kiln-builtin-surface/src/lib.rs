//! Which members each built-in global has, declared once for both engines.
//!
//! Kiln has two engines -- `kiln-interpreter` and `kiln-backend-c`, which emits C -- and this is
//! the one place either of them says what `Object` or `Number` *has*. Each engine still decides
//! for itself what a member *does*; only the surface is shared, because only the surface was ever
//! written down twice.
//!
//! ONE TABLE, TWO EMITTERS (ADR 0043). Before this crate, `Number` had fifteen statics in the
//! interpreter and five in the emitted C, `Symbol` had thirteen well-known symbols and five, and
//! nothing compared the two. The native backend refused at runtime when a program read a member it
//! lacked -- so the gaps arrived one npm bundle at a time, in whatever order real code happened to
//! touch them. `Number.MAX_VALUE` is the first property a bundled `zod` reads.
//!
//! # What the compiler enforces, and what it cannot
//!
//! Adding a member here is a compile error in `kiln-backend-c` until that backend either
//! implements it or records it in `BUILTIN_SURFACE_BACKLOG` with a reason. That check is a
//! `const` evaluation of both tables, so it fails the build rather than a test run. The
//! interpreter reads its own member lists straight out of this table through [`members`], which is
//! a `const fn`: a global that is not here does not compile there either.
//!
//! What no amount of const evaluation can give is a *good message* -- a `const` panic carries a
//! literal and nothing else. So the build says only that the two disagree, and
//! `builtin_surface_drift` in `kiln-native` says which members and why. The compiler is the gate;
//! the test is the explanation.
//!
//! # Spelling
//!
//! A `@@`-prefixed member is symbol-keyed: `@@species` is `Symbol.species`, and both engines
//! already spell a reserved store key that way. `length`, `name` and `prototype` are not listed --
//! they belong to every callable rather than to any particular one, and each engine supplies them
//! from its own knowledge of what a function is.
//!
//! # Order is observable
//!
//! A string-keyed member enumerates in the order it is defined, so this list is the order
//! `Object.getOwnPropertyNames` and `Reflect.ownKeys` report -- in both engines, since both read
//! it from here. Match the reference implementation rather than grouping by what reads well:
//! `Array` carried `fromAsync` before `of` and a differential probe caught it, because V8 defines
//! `of` first and a proxy's `ownKeys` prints the difference. Appending a new member to the end of
//! a list is therefore a claim about where the reference implementation defines it.

#![forbid(unsafe_code)]

/// A built-in global and the members it owns.
pub struct Builtin {
    /// The global as a program spells it. Two entries are not globals a program can name:
    /// `%TypedArray%` is the intrinsic the concrete views inherit from, and
    /// `%TypedArrayConstructor%` is the shared shape of every view constructor except `Uint8Array`,
    /// which has two statics of its own. Both are spelled the way the specification spells an
    /// intrinsic, so they cannot collide with a global.
    pub global: &'static str,
    /// Its own members, in the order reflection reports them.
    pub members: &'static [&'static str],
}

/// Every built-in whose surface both engines are expected to agree on.
pub const SURFACE: &[Builtin] = &[
    Builtin {
        global: "Object",
        members: &[
            "keys",
            "getOwnPropertyNames",
            "assign",
            "entries",
            "values",
            "freeze",
            "isFrozen",
            "create",
            "defineProperty",
            "fromEntries",
            "hasOwn",
            "groupBy",
            "is",
            "getOwnPropertySymbols",
            "defineProperties",
            "getOwnPropertyDescriptor",
            "getOwnPropertyDescriptors",
            "setPrototypeOf",
            "getPrototypeOf",
            "preventExtensions",
            "isExtensible",
            "seal",
            "isSealed",
        ],
    },
    Builtin {
        global: "Array",
        members: &["isArray", "from", "of", "fromAsync", "@@species"],
    },
    Builtin {
        global: "String",
        members: &["raw", "fromCharCode", "fromCodePoint"],
    },
    Builtin {
        global: "Number",
        members: &[
            "isInteger",
            "isSafeInteger",
            "isNaN",
            "isFinite",
            "parseInt",
            "parseFloat",
            "MAX_SAFE_INTEGER",
            "MIN_SAFE_INTEGER",
            "MAX_VALUE",
            "MIN_VALUE",
            "EPSILON",
            "POSITIVE_INFINITY",
            "NEGATIVE_INFINITY",
            "NaN",
        ],
    },
    Builtin {
        global: "Date",
        members: &["now", "UTC", "parse"],
    },
    Builtin {
        global: "Promise",
        members: &[
            "resolve",
            "reject",
            "all",
            "allSettled",
            "race",
            "any",
            "withResolvers",
            "try",
        ],
    },
    Builtin {
        global: "JSON",
        members: &[
            "parse",
            "rawJSON",
            "isRawJSON",
            "stringify",
            "@@toStringTag",
        ],
    },
    Builtin {
        global: "Reflect",
        members: &[
            "defineProperty",
            "deleteProperty",
            "apply",
            "construct",
            "get",
            "getOwnPropertyDescriptor",
            "getPrototypeOf",
            "has",
            "isExtensible",
            "ownKeys",
            "preventExtensions",
            "set",
            "setPrototypeOf",
            "@@toStringTag",
        ],
    },
    Builtin {
        global: "Proxy",
        members: &["revocable"],
    },
    Builtin {
        global: "ArrayBuffer",
        members: &["isView"],
    },
    Builtin {
        global: "RegExp",
        members: &["escape"],
    },
    Builtin {
        global: "Error",
        members: &["isError", "captureStackTrace"],
    },
    Builtin {
        global: "BigInt",
        members: &["asIntN", "asUintN"],
    },
    Builtin {
        global: "Map",
        members: &["groupBy"],
    },
    Builtin {
        global: "Symbol",
        members: &[
            "for",
            "keyFor",
            "asyncIterator",
            "hasInstance",
            "isConcatSpreadable",
            "iterator",
            "match",
            "matchAll",
            "replace",
            "search",
            "species",
            "split",
            "toPrimitive",
            "toStringTag",
            "unscopables",
        ],
    },
    Builtin {
        global: "Iterator",
        members: &["from", "concat", "zip", "zipKeyed"],
    },
    Builtin {
        global: "Buffer",
        members: &[
            "from",
            "alloc",
            "allocUnsafe",
            "allocUnsafeSlow",
            "isBuffer",
            "byteLength",
            "concat",
            "constants",
        ],
    },
    Builtin {
        global: "Atomics",
        members: &[
            "add",
            "and",
            "compareExchange",
            "exchange",
            "isLockFree",
            "load",
            "notify",
            "or",
            "pause",
            "store",
            "sub",
            "wait",
            "waitAsync",
            "xor",
            "@@toStringTag",
        ],
    },
    Builtin {
        global: "Math",
        members: &[
            "abs",
            "floor",
            "ceil",
            "round",
            "trunc",
            "sign",
            "sqrt",
            "cbrt",
            "random",
            "sinh",
            "cosh",
            "tanh",
            "asinh",
            "acosh",
            "atanh",
            "log1p",
            "expm1",
            "fround",
            "f16round",
            "clz32",
            "imul",
            "hypot",
            "atan2",
            "sin",
            "cos",
            "tan",
            "asin",
            "acos",
            "atan",
            "pow",
            "min",
            "max",
            "log",
            "log2",
            "log10",
            "exp",
            "PI",
            "E",
            "LN2",
            "LN10",
            "SQRT2",
            "SQRT1_2",
            "LOG2E",
            "LOG10E",
            "@@toStringTag",
        ],
    },
    Builtin {
        global: "%TypedArray%",
        members: &["@@species"],
    },
    Builtin {
        global: "%TypedArrayConstructor%",
        members: &["of", "from", "BYTES_PER_ELEMENT"],
    },
    Builtin {
        // The two base64 helpers are `Uint8Array`'s alone: they read and write *bytes*, and no
        // other view's element is a byte.
        global: "Uint8Array",
        members: &["of", "from", "BYTES_PER_ELEMENT", "fromBase64", "fromHex"],
    },
    Builtin {
        // `now` is the only member either engine answers. The rest of the Performance interface
        // wants an entry buffer and a time origin that a realm does not carry yet, so declaring
        // just this one keeps the table a description of what is really shared.
        global: "performance",
        members: &["now"],
    },
];

const fn same(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// The members `global` owns.
///
/// A `const fn`, so a caller that binds the result to a `const` turns a misspelled global into a
/// compile error rather than an empty list. An empty list is the failure mode worth engineering
/// against here: it is indistinguishable from "this built-in has no statics", which is why a
/// missing Node namespace once made `Object.keys(require("path"))` answer nothing at all.
pub const fn members(global: &str) -> &'static [&'static str] {
    let mut index = 0;
    while index < SURFACE.len() {
        if same(SURFACE[index].global, global) {
            return SURFACE[index].members;
        }
        index += 1;
    }
    panic!("no such built-in in kiln_builtin_surface::SURFACE");
}

/// The members `global` owns, or `None` when the table does not speak for it.
///
/// The runtime companion to [`members`], for a caller holding a name it did not write down.
pub fn find(global: &str) -> Option<&'static [&'static str]> {
    SURFACE
        .iter()
        .find(|builtin| builtin.global == global)
        .map(|builtin| builtin.members)
}

/// Whether `global` owns `member`.
pub const fn has(global: &str, member: &str) -> bool {
    let members = members(global);
    let mut index = 0;
    while index < members.len() {
        if same(members[index], member) {
            return true;
        }
        index += 1;
    }
    false
}

/// Whether the table speaks for `global` at all, answerable in a `const`.
pub const fn is_declared(global: &str) -> bool {
    let mut index = 0;
    while index < SURFACE.len() {
        if same(SURFACE[index].global, global) {
            return true;
        }
        index += 1;
    }
    false
}

/// A global may be declared once, and a member may be listed once within it.
///
/// A duplicate is not harmless: both engines answer the *first* match, so a second row with a
/// different meaning would be dead in a way no reader could see.
const fn validate(surface: &[Builtin]) {
    let mut index = 0;
    while index < surface.len() {
        let mut other = index + 1;
        while other < surface.len() {
            assert!(
                !same(surface[index].global, surface[other].global),
                "a built-in is declared twice in SURFACE"
            );
            other += 1;
        }
        let members = surface[index].members;
        assert!(
            !members.is_empty(),
            "a built-in with no members is a row that says nothing"
        );
        let mut member = 0;
        while member < members.len() {
            assert!(!members[member].is_empty(), "a member with no name");
            let mut rest = member + 1;
            while rest < members.len() {
                assert!(
                    !same(members[member], members[rest]),
                    "a member is listed twice for one built-in"
                );
                rest += 1;
            }
            member += 1;
        }
        index += 1;
    }
}

const _: () = validate(SURFACE);
