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
//! # The key is the name
//!
//! `name` is not listed as a *member*, but this table still decides it: a static's `.name` is its
//! member key. `Object.keys.name` is `"keys"`, and node makes that true for every static on every
//! built-in without exception -- it was measured across all seventeen rows where the two engines
//! disagreed, and node's answer was the key every time.
//!
//! That was not always how either engine answered. Each derived the name separately -- the native
//! backend from a hand-written `case` per method code, the interpreter from the Rust variant's
//! spelling -- so the name was written down three times and agreed twice at best. Fifteen native
//! rows had a key and no `case` and reported `""`; thirteen interpreter rows reported the variant,
//! so `Buffer.concat.name` was `"nodeBufferConcat"`. Nothing caught it, because a wrong name
//! refuses nothing: it is a string a program reads, and reading it succeeds.
//!
//! So the native backend now reads the name out of the row, and the interpreter is held to the row
//! by `every_surface_member_is_named_for_its_key`. [`NAME_IS_SHARED`] is the only way out, and it
//! has two entries.
//! # Order is observable
//!
//! A string-keyed member enumerates in the order it is defined, so this list is the order
//! `Object.getOwnPropertyNames` and `Reflect.ownKeys` report -- in both engines, since both read
//! it from here. Match the reference implementation rather than grouping by what reads well:
//! `Array` carries `fromAsync` before `of` per ECMAScript §23.1.2 and Node v26, and a differential
//! probe checks it. Appending a new member to the end of
//! a list is therefore a claim about where the reference implementation defines it.

#![forbid(unsafe_code)]

/// The members this implementation serves with *another* member's function.
///
/// A static's `.name` is its member key -- `Object.keys.name` is `"keys"` -- and node makes that
/// true for every static on every built-in, without exception. Both engines therefore take the
/// name straight from the key in [`SURFACE`], which is what stops the name being written down
/// twice and drifting: seventeen of them had, and nobody could tell, because a name is a string a
/// program reads rather than an operation that can refuse.
///
/// These are the rows where that cannot hold. `Buffer.allocUnsafe` is not a distinct function
/// here -- it is `Buffer.alloc`, which differs from node only in that node zero-fills one and not
/// the other -- so it reports `"alloc"`, the name of the function it actually is. Listing it is
/// how the guards in both engines tell a known alias from a name that has drifted, and the list
/// is short on purpose: the entry is a debt, not a licence.
pub const NAME_IS_SHARED: &[(&str, &str)] = &[
    ("Buffer", "allocUnsafe"),
    ("Buffer", "allocUnsafeSlow"),
    ("Array.prototype", "@@iterator"),
];

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
        members: &["isArray", "from", "fromAsync", "of", "@@species"],
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
            "allKeyed",
            "allSettledKeyed",
            "@@species",
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
        members: &["isView", "@@species"],
    },
    Builtin {
        global: "RegExp",
        members: &["escape", "@@species"],
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
        members: &["groupBy", "@@species"],
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
            "dispose",
            "asyncDispose",
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

/// The 16 ECMAScript well-known symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WellKnownSymbol {
    Iterator,
    AsyncIterator,
    HasInstance,
    ToStringTag,
    ToPrimitive,
    Match,
    MatchAll,
    Replace,
    Search,
    Split,
    Species,
    Unscopables,
    IsConcatSpreadable,
    Dispose,
    AsyncDispose,
    NodeInspectCustom,
}

impl WellKnownSymbol {
    pub const ALL: [Self; 16] = [
        Self::Iterator,
        Self::AsyncIterator,
        Self::HasInstance,
        Self::ToStringTag,
        Self::ToPrimitive,
        Self::Match,
        Self::MatchAll,
        Self::Replace,
        Self::Search,
        Self::Split,
        Self::Species,
        Self::Unscopables,
        Self::IsConcatSpreadable,
        Self::Dispose,
        Self::AsyncDispose,
        Self::NodeInspectCustom,
    ];

    pub const fn property_key(self) -> &'static str {
        match self {
            Self::Iterator => "@@iterator",
            Self::AsyncIterator => "@@asyncIterator",
            Self::HasInstance => "@@hasInstance",
            Self::ToStringTag => "@@toStringTag",
            Self::ToPrimitive => "@@toPrimitive",
            Self::Match => "@@match",
            Self::MatchAll => "@@matchAll",
            Self::Replace => "@@replace",
            Self::Search => "@@search",
            Self::Split => "@@split",
            Self::Species => "@@species",
            Self::Unscopables => "@@unscopables",
            Self::IsConcatSpreadable => "@@isConcatSpreadable",
            Self::Dispose => "@@dispose",
            Self::AsyncDispose => "@@asyncDispose",
            Self::NodeInspectCustom => "@@nodeInspectCustom",
        }
    }

    pub const fn js_name(self) -> &'static str {
        match self {
            Self::Iterator => "iterator",
            Self::AsyncIterator => "asyncIterator",
            Self::HasInstance => "hasInstance",
            Self::ToStringTag => "toStringTag",
            Self::ToPrimitive => "toPrimitive",
            Self::Match => "match",
            Self::MatchAll => "matchAll",
            Self::Replace => "replace",
            Self::Search => "search",
            Self::Split => "split",
            Self::Species => "species",
            Self::Unscopables => "unscopables",
            Self::IsConcatSpreadable => "isConcatSpreadable",
            Self::Dispose => "dispose",
            Self::AsyncDispose => "asyncDispose",
            Self::NodeInspectCustom => "nodeInspectCustom",
        }
    }

    pub const fn from_property_key(key: &str) -> Option<Self> {
        let mut i = 0;
        while i < Self::ALL.len() {
            if same(Self::ALL[i].property_key(), key) {
                return Some(Self::ALL[i]);
            }
            i += 1;
        }
        None
    }

    pub const fn ordinal(self) -> usize {
        match self {
            Self::Iterator => 0,
            Self::AsyncIterator => 1,
            Self::HasInstance => 2,
            Self::ToStringTag => 3,
            Self::ToPrimitive => 4,
            Self::Match => 5,
            Self::MatchAll => 6,
            Self::Replace => 7,
            Self::Search => 8,
            Self::Split => 9,
            Self::Species => 10,
            Self::Unscopables => 11,
            Self::IsConcatSpreadable => 12,
            Self::Dispose => 13,
            Self::AsyncDispose => 14,
            Self::NodeInspectCustom => 15,
        }
    }

    pub const fn published_by_symbol(self) -> bool {
        !matches!(self, Self::NodeInspectCustom)
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Iterator => "Symbol.iterator",
            Self::AsyncIterator => "Symbol.asyncIterator",
            Self::HasInstance => "Symbol.hasInstance",
            Self::ToStringTag => "Symbol.toStringTag",
            Self::ToPrimitive => "Symbol.toPrimitive",
            Self::Match => "Symbol.match",
            Self::MatchAll => "Symbol.matchAll",
            Self::Replace => "Symbol.replace",
            Self::Search => "Symbol.search",
            Self::Split => "Symbol.split",
            Self::Species => "Symbol.species",
            Self::Unscopables => "Symbol.unscopables",
            Self::IsConcatSpreadable => "Symbol.isConcatSpreadable",
            Self::Dispose => "Symbol.dispose",
            Self::AsyncDispose => "Symbol.asyncDispose",
            Self::NodeInspectCustom => "Symbol(nodejs.util.inspect.custom)",
        }
    }
}

/// The key a built-in member is published under.
///
/// Separates string-keyed members from symbol-keyed members by type, preventing internal
/// `@@`-prefixed spellings from being exposed as string names (ADR 0054, ADR 0059).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberKey {
    Named(&'static str),
    Symbol(WellKnownSymbol),
}

impl MemberKey {
    pub const fn property_key(self) -> &'static str {
        match self {
            Self::Named(name) => name,
            Self::Symbol(symbol) => symbol.property_key(),
        }
    }

    pub const fn is_symbol(self) -> bool {
        matches!(self, Self::Symbol(_))
    }

    pub const fn same(self, other: MemberKey) -> bool {
        match (self, other) {
            (MemberKey::Named(a), MemberKey::Named(b)) => same(a, b),
            (MemberKey::Symbol(a), MemberKey::Symbol(b)) => a as usize == b as usize,
            _ => false,
        }
    }
}

pub const fn is_reserved_spelling(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'@' && bytes[1] == b'@'
}

/// The kind of built-in member and its callable metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// A callable method with a defined ECMAScript arity/length.
    Method { length: usize },
    /// An accessor property.
    Accessor,
    /// A plain data property.
    Data,
}

/// ECMAScript property descriptor attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorAttributes {
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

impl DescriptorAttributes {
    /// Standard built-in prototype method: { writable: true, enumerable: false, configurable: true }
    pub const METHOD: Self = Self {
        writable: true,
        enumerable: false,
        configurable: true,
    };

    /// Standard built-in prototype accessor: { writable: false, enumerable: false, configurable: true }
    pub const ACCESSOR: Self = Self {
        writable: false,
        enumerable: false,
        configurable: true,
    };

    /// Read-only configurable data property: { writable: false, enumerable: false, configurable: true }
    pub const READONLY: Self = Self {
        writable: false,
        enumerable: false,
        configurable: true,
    };

    /// Locked property: { writable: false, enumerable: false, configurable: false }
    pub const LOCKED: Self = Self {
        writable: false,
        enumerable: false,
        configurable: false,
    };

    /// Array.prototype.length: { writable: true, enumerable: false, configurable: false }
    pub const ARRAY_LENGTH: Self = Self {
        writable: true,
        enumerable: false,
        configurable: false,
    };
}

/// Which built-in prototype owns this member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrototypeId {
    Symbol,
    Array,
    Number,
    Boolean,
    BigInt,
    Promise,
    Object,
    Function,
    Map,
    Set,
    WeakMap,
    WeakSet,
}

impl PrototypeId {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Symbol => "Symbol.prototype",
            Self::Array => "Array.prototype",
            Self::Number => "Number.prototype",
            Self::Boolean => "Boolean.prototype",
            Self::BigInt => "BigInt.prototype",
            Self::Promise => "Promise.prototype",
            Self::Object => "Object.prototype",
            Self::Function => "Function.prototype",
            Self::Map => "Map.prototype",
            Self::Set => "Set.prototype",
            Self::WeakMap => "WeakMap.prototype",
            Self::WeakSet => "WeakSet.prototype",
        }
    }

    pub const fn from_name(name: &str) -> Option<Self> {
        if same(name, "Symbol.prototype") {
            Some(Self::Symbol)
        } else if same(name, "Array.prototype") {
            Some(Self::Array)
        } else if same(name, "Number.prototype") {
            Some(Self::Number)
        } else if same(name, "Boolean.prototype") {
            Some(Self::Boolean)
        } else if same(name, "BigInt.prototype") {
            Some(Self::BigInt)
        } else if same(name, "Promise.prototype") {
            Some(Self::Promise)
        } else if same(name, "Object.prototype") {
            Some(Self::Object)
        } else if same(name, "Function.prototype") {
            Some(Self::Function)
        } else if same(name, "Map.prototype") {
            Some(Self::Map)
        } else if same(name, "Set.prototype") {
            Some(Self::Set)
        } else if same(name, "WeakMap.prototype") {
            Some(Self::WeakMap)
        } else if same(name, "WeakSet.prototype") {
            Some(Self::WeakSet)
        } else {
            None
        }
    }
}

/// Built-in methods of `Array.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArrayMethod {
    At,
    Concat,
    CopyWithin,
    Entries,
    Every,
    Fill,
    Filter,
    Find,
    FindIndex,
    FindLast,
    FindLastIndex,
    Flat,
    FlatMap,
    ForEach,
    Includes,
    IndexOf,
    Join,
    Keys,
    LastIndexOf,
    Map,
    ParallelMap,
    Pop,
    Push,
    Reduce,
    ReduceRight,
    Reverse,
    Shift,
    Slice,
    Some,
    Sort,
    Splice,
    ToLocaleString,
    ToReversed,
    ToSorted,
    ToSpliced,
    ToString,
    Unshift,
    Values,
    With,
}

/// Built-in methods of `Number.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NumberMethod {
    ToExponential,
    ToFixed,
    ToLocaleString,
    ToPrecision,
    ToString,
    ValueOf,
}

/// Built-in methods of `Boolean.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BooleanMethod {
    ToString,
    ValueOf,
}

/// Built-in methods of `BigInt.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BigIntMethod {
    ToLocaleString,
    ToString,
    ValueOf,
}

/// Built-in methods of `Promise.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PromiseMethod {
    Then,
    Catch,
    Finally,
}

/// Built-in methods of `Object.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectMethod {
    HasOwnProperty,
    IsPrototypeOf,
    PropertyIsEnumerable,
    ToString,
    ToLocaleString,
    ValueOf,
    DefineGetter,
    DefineSetter,
    LookupGetter,
    LookupSetter,
}

/// Built-in methods of `Function.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FunctionMethod {
    Apply,
    Bind,
    Call,
    ToString,
}

/// Built-in methods of `Map.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MapMethod {
    Clear,
    Delete,
    Entries,
    ForEach,
    Get,
    GetOrInsert,
    GetOrInsertComputed,
    Has,
    Keys,
    Set,
    Values,
}

/// Built-in methods of `Set.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SetMethod {
    Add,
    Clear,
    Delete,
    Entries,
    ForEach,
    Has,
    Keys,
    Values,
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
    IsSubsetOf,
    IsSupersetOf,
    IsDisjointFrom,
}

/// Built-in methods of `WeakMap.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WeakMapMethod {
    Delete,
    Get,
    GetOrInsert,
    GetOrInsertComputed,
    Has,
    Set,
}

/// Built-in methods of `WeakSet.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WeakSetMethod {
    Add,
    Delete,
    Has,
}

/// Authoritative semantic operation binding across both engines (ADR 0059, ADR 0060).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticBinding {
    PrimitiveToString,
    PrimitiveValueOf,
    SymbolDescription,
    SymbolToPrimitive,
    StringTag(&'static str),
    Constructor,
    ArrayLength,
    ArrayUnscopables,
    ArrayMethod(ArrayMethod),
    NumberMethod(NumberMethod),
    BooleanMethod(BooleanMethod),
    BigIntMethod(BigIntMethod),
    PromiseMethod(PromiseMethod),
    ObjectMethod(ObjectMethod),
    ObjectProto,
    FunctionMethod(FunctionMethod),
    FunctionHasInstance,
    FunctionCaller,
    FunctionArguments,
    MapMethod(MapMethod),
    SetMethod(SetMethod),
    WeakMapMethod(WeakMapMethod),
    WeakSetMethod(WeakSetMethod),
    CollectionSize(bool),     // true = Map, false = Set
    CollectionIterator(bool), // true = Map, false = Set
}

/// One declarative row describing a prototype member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrototypeMember {
    pub prototype: PrototypeId,
    pub key: MemberKey,
    pub kind: MemberKind,
    pub attributes: DescriptorAttributes,
    pub binding: SemanticBinding,
}

/// Every prototype member declared for unified materialization across engines.
pub const PROTOTYPE_SURFACE: &[PrototypeMember] = &[
    PrototypeMember {
        prototype: PrototypeId::Symbol,
        key: MemberKey::Named("toString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::PrimitiveToString,
    },
    PrototypeMember {
        prototype: PrototypeId::Symbol,
        key: MemberKey::Named("valueOf"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::PrimitiveValueOf,
    },
    PrototypeMember {
        prototype: PrototypeId::Symbol,
        key: MemberKey::Named("description"),
        kind: MemberKind::Accessor,
        attributes: DescriptorAttributes::ACCESSOR,
        binding: SemanticBinding::SymbolDescription,
    },
    PrototypeMember {
        prototype: PrototypeId::Symbol,
        key: MemberKey::Symbol(WellKnownSymbol::ToStringTag),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::StringTag("Symbol"),
    },
    PrototypeMember {
        prototype: PrototypeId::Symbol,
        key: MemberKey::Symbol(WellKnownSymbol::ToPrimitive),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::SymbolToPrimitive,
    },
    PrototypeMember {
        prototype: PrototypeId::Symbol,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    // Array.prototype
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("length"),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::ARRAY_LENGTH,
        binding: SemanticBinding::ArrayLength,
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("map"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Map),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("parallelMap"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ParallelMap),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("forEach"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ForEach),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("filter"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Filter),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("reduce"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Reduce),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("sort"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Sort),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("findIndex"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::FindIndex),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("find"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Find),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("some"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Some),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("every"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Every),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("flatMap"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::FlatMap),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("findLast"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::FindLast),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("findLastIndex"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::FindLastIndex),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("reduceRight"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ReduceRight),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("join"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Join),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("push"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Push),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("pop"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Pop),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("indexOf"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::IndexOf),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("includes"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Includes),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("slice"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Slice),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("at"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::At),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("flat"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Flat),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("entries"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Entries),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("keys"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Keys),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("concat"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Concat),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("reverse"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Reverse),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("shift"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Shift),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("unshift"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Unshift),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("lastIndexOf"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::LastIndexOf),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("fill"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Fill),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("splice"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Splice),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("copyWithin"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::CopyWithin),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("toString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ToString),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("toLocaleString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ToLocaleString),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("values"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Values),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("with"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::With),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("toReversed"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ToReversed),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("toSorted"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ToSorted),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Named("toSpliced"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::ToSpliced),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Symbol(WellKnownSymbol::Iterator),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ArrayMethod(ArrayMethod::Values),
    },
    PrototypeMember {
        prototype: PrototypeId::Array,
        key: MemberKey::Symbol(WellKnownSymbol::Unscopables),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::ArrayUnscopables,
    },
    // Number.prototype (ECMAScript §21.1.3)
    PrototypeMember {
        prototype: PrototypeId::Number,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Number,
        key: MemberKey::Named("toExponential"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::NumberMethod(NumberMethod::ToExponential),
    },
    PrototypeMember {
        prototype: PrototypeId::Number,
        key: MemberKey::Named("toFixed"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::NumberMethod(NumberMethod::ToFixed),
    },
    PrototypeMember {
        prototype: PrototypeId::Number,
        key: MemberKey::Named("toPrecision"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::NumberMethod(NumberMethod::ToPrecision),
    },
    PrototypeMember {
        prototype: PrototypeId::Number,
        key: MemberKey::Named("toString"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::NumberMethod(NumberMethod::ToString),
    },
    PrototypeMember {
        prototype: PrototypeId::Number,
        key: MemberKey::Named("valueOf"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::NumberMethod(NumberMethod::ValueOf),
    },
    PrototypeMember {
        prototype: PrototypeId::Number,
        key: MemberKey::Named("toLocaleString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::NumberMethod(NumberMethod::ToLocaleString),
    },
    // Boolean.prototype (ECMAScript §20.3.3)
    PrototypeMember {
        prototype: PrototypeId::Boolean,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Boolean,
        key: MemberKey::Named("toString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::BooleanMethod(BooleanMethod::ToString),
    },
    PrototypeMember {
        prototype: PrototypeId::Boolean,
        key: MemberKey::Named("valueOf"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::BooleanMethod(BooleanMethod::ValueOf),
    },
    // BigInt.prototype (ECMAScript §21.2.3)
    PrototypeMember {
        prototype: PrototypeId::BigInt,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::BigInt,
        key: MemberKey::Named("toLocaleString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::BigIntMethod(BigIntMethod::ToLocaleString),
    },
    PrototypeMember {
        prototype: PrototypeId::BigInt,
        key: MemberKey::Named("toString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::BigIntMethod(BigIntMethod::ToString),
    },
    PrototypeMember {
        prototype: PrototypeId::BigInt,
        key: MemberKey::Named("valueOf"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::BigIntMethod(BigIntMethod::ValueOf),
    },
    PrototypeMember {
        prototype: PrototypeId::BigInt,
        key: MemberKey::Symbol(WellKnownSymbol::ToStringTag),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::StringTag("BigInt"),
    },
    // Promise.prototype (ECMAScript §27.2.5)
    PrototypeMember {
        prototype: PrototypeId::Promise,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Promise,
        key: MemberKey::Named("then"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::PromiseMethod(PromiseMethod::Then),
    },
    PrototypeMember {
        prototype: PrototypeId::Promise,
        key: MemberKey::Named("catch"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::PromiseMethod(PromiseMethod::Catch),
    },
    PrototypeMember {
        prototype: PrototypeId::Promise,
        key: MemberKey::Named("finally"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::PromiseMethod(PromiseMethod::Finally),
    },
    PrototypeMember {
        prototype: PrototypeId::Promise,
        key: MemberKey::Symbol(WellKnownSymbol::ToStringTag),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::StringTag("Promise"),
    },
    // Object.prototype (ECMAScript §20.1.3 and Annex B.2.2)
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("hasOwnProperty"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::HasOwnProperty),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("isPrototypeOf"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::IsPrototypeOf),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("propertyIsEnumerable"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::PropertyIsEnumerable),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("toLocaleString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::ToLocaleString),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("toString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::ToString),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("valueOf"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::ValueOf),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("__defineGetter__"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::DefineGetter),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("__defineSetter__"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::DefineSetter),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("__lookupGetter__"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::LookupGetter),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("__lookupSetter__"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::ObjectMethod(ObjectMethod::LookupSetter),
    },
    PrototypeMember {
        prototype: PrototypeId::Object,
        key: MemberKey::Named("__proto__"),
        kind: MemberKind::Accessor,
        attributes: DescriptorAttributes::ACCESSOR,
        binding: SemanticBinding::ObjectProto,
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Named("apply"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::FunctionMethod(FunctionMethod::Apply),
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Named("bind"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::FunctionMethod(FunctionMethod::Bind),
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Named("call"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::FunctionMethod(FunctionMethod::Call),
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Named("toString"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::FunctionMethod(FunctionMethod::ToString),
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Symbol(WellKnownSymbol::HasInstance),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::LOCKED,
        binding: SemanticBinding::FunctionHasInstance,
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Named("caller"),
        kind: MemberKind::Accessor,
        attributes: DescriptorAttributes::ACCESSOR,
        binding: SemanticBinding::FunctionCaller,
    },
    PrototypeMember {
        prototype: PrototypeId::Function,
        key: MemberKey::Named("arguments"),
        kind: MemberKind::Accessor,
        attributes: DescriptorAttributes::ACCESSOR,
        binding: SemanticBinding::FunctionArguments,
    },
    // Map.prototype (ECMAScript §24.1.3)
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("clear"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Clear),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("delete"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Delete),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("entries"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Entries),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("forEach"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::ForEach),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("get"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Get),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("getOrInsert"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::GetOrInsert),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("getOrInsertComputed"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::GetOrInsertComputed),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("has"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Has),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("keys"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Keys),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("set"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Set),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("size"),
        kind: MemberKind::Accessor,
        attributes: DescriptorAttributes::ACCESSOR,
        binding: SemanticBinding::CollectionSize(true),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Named("values"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::MapMethod(MapMethod::Values),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Symbol(WellKnownSymbol::Iterator),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::CollectionIterator(true),
    },
    PrototypeMember {
        prototype: PrototypeId::Map,
        key: MemberKey::Symbol(WellKnownSymbol::ToStringTag),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::StringTag("Map"),
    },
    // Set.prototype (ECMAScript §24.2.3)
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("add"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Add),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("clear"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Clear),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("delete"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Delete),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("difference"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Difference),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("entries"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Entries),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("forEach"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::ForEach),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("has"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Has),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("intersection"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Intersection),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("isDisjointFrom"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::IsDisjointFrom),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("isSubsetOf"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::IsSubsetOf),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("isSupersetOf"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::IsSupersetOf),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("keys"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Keys),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("size"),
        kind: MemberKind::Accessor,
        attributes: DescriptorAttributes::ACCESSOR,
        binding: SemanticBinding::CollectionSize(false),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("symmetricDifference"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::SymmetricDifference),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("union"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Union),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Named("values"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::SetMethod(SetMethod::Values),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Symbol(WellKnownSymbol::Iterator),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::CollectionIterator(false),
    },
    PrototypeMember {
        prototype: PrototypeId::Set,
        key: MemberKey::Symbol(WellKnownSymbol::ToStringTag),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::StringTag("Set"),
    },
    // WeakMap.prototype (ECMAScript §24.3.3)
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Named("delete"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakMapMethod(WeakMapMethod::Delete),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Named("get"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakMapMethod(WeakMapMethod::Get),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Named("getOrInsert"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakMapMethod(WeakMapMethod::GetOrInsert),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Named("getOrInsertComputed"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakMapMethod(WeakMapMethod::GetOrInsertComputed),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Named("has"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakMapMethod(WeakMapMethod::Has),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Named("set"),
        kind: MemberKind::Method { length: 2 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakMapMethod(WeakMapMethod::Set),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakMap,
        key: MemberKey::Symbol(WellKnownSymbol::ToStringTag),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::StringTag("WeakMap"),
    },
    // WeakSet.prototype (ECMAScript §24.4.3)
    PrototypeMember {
        prototype: PrototypeId::WeakSet,
        key: MemberKey::Named("constructor"),
        kind: MemberKind::Method { length: 0 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::Constructor,
    },
    PrototypeMember {
        prototype: PrototypeId::WeakSet,
        key: MemberKey::Named("add"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakSetMethod(WeakSetMethod::Add),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakSet,
        key: MemberKey::Named("delete"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakSetMethod(WeakSetMethod::Delete),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakSet,
        key: MemberKey::Named("has"),
        kind: MemberKind::Method { length: 1 },
        attributes: DescriptorAttributes::METHOD,
        binding: SemanticBinding::WeakSetMethod(WeakSetMethod::Has),
    },
    PrototypeMember {
        prototype: PrototypeId::WeakSet,
        key: MemberKey::Symbol(WellKnownSymbol::ToStringTag),
        kind: MemberKind::Data,
        attributes: DescriptorAttributes::READONLY,
        binding: SemanticBinding::StringTag("WeakSet"),
    },
];

const fn validate_prototype_surface(surface: &[PrototypeMember]) {
    let mut index = 0;
    while index < surface.len() {
        let row = &surface[index];
        if let MemberKey::Named(name) = row.key {
            assert!(
                !is_reserved_spelling(name),
                "a symbol-keyed member cannot be published as a named key (use MemberKey::Symbol)"
            );
        }
        let mut other = index + 1;
        while other < surface.len() {
            if row.prototype as usize == surface[other].prototype as usize {
                assert!(
                    !row.key.same(surface[other].key),
                    "duplicate member key on the same prototype in PROTOTYPE_SURFACE"
                );
            }
            other += 1;
        }
        index += 1;
    }
}

const _: () = validate_prototype_surface(PROTOTYPE_SURFACE);

pub fn prototype_members(prototype: PrototypeId) -> impl Iterator<Item = &'static PrototypeMember> {
    PROTOTYPE_SURFACE
        .iter()
        .filter(move |m| m.prototype == prototype)
}

pub fn find_prototype_member(
    prototype: PrototypeId,
    key: &str,
) -> Option<&'static PrototypeMember> {
    PROTOTYPE_SURFACE
        .iter()
        .find(|m| m.prototype == prototype && m.key.property_key() == key)
}

pub const fn prototype_has(prototype: PrototypeId, key: &str) -> bool {
    let mut index = 0;
    while index < PROTOTYPE_SURFACE.len() {
        if PROTOTYPE_SURFACE[index].prototype as usize == prototype as usize
            && same(PROTOTYPE_SURFACE[index].key.property_key(), key)
        {
            return true;
        }
        index += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_symbol_round_trip() {
        for sym in WellKnownSymbol::ALL {
            assert_eq!(
                WellKnownSymbol::from_property_key(sym.property_key()),
                Some(sym)
            );
        }
    }

    #[test]
    fn symbol_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Symbol).collect();
        assert_eq!(members.len(), 6);
        assert!(prototype_has(PrototypeId::Symbol, "toString"));
        assert!(prototype_has(PrototypeId::Symbol, "valueOf"));
        assert!(prototype_has(PrototypeId::Symbol, "description"));
        assert!(prototype_has(PrototypeId::Symbol, "constructor"));
        assert!(prototype_has(PrototypeId::Symbol, "@@toStringTag"));
        assert!(prototype_has(PrototypeId::Symbol, "@@toPrimitive"));
        assert!(!prototype_has(PrototypeId::Symbol, "nonexistent"));
    }

    #[test]
    fn array_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Array).collect();
        assert_eq!(members.len(), 43);
        assert!(prototype_has(PrototypeId::Array, "length"));
        assert!(prototype_has(PrototypeId::Array, "constructor"));
        assert!(prototype_has(PrototypeId::Array, "map"));
        assert!(prototype_has(PrototypeId::Array, "parallelMap"));
        assert!(prototype_has(PrototypeId::Array, "push"));
        assert!(prototype_has(PrototypeId::Array, "pop"));
        assert!(prototype_has(PrototypeId::Array, "slice"));
        assert!(prototype_has(PrototypeId::Array, "@@iterator"));
        assert!(prototype_has(PrototypeId::Array, "@@unscopables"));
        assert!(!prototype_has(PrototypeId::Array, "nonexistent"));
    }

    #[test]
    fn number_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Number).collect();
        assert_eq!(members.len(), 7);
        assert!(prototype_has(PrototypeId::Number, "constructor"));
        assert!(prototype_has(PrototypeId::Number, "toExponential"));
        assert!(prototype_has(PrototypeId::Number, "toFixed"));
        assert!(prototype_has(PrototypeId::Number, "toPrecision"));
        assert!(prototype_has(PrototypeId::Number, "toString"));
        assert!(prototype_has(PrototypeId::Number, "valueOf"));
        assert!(prototype_has(PrototypeId::Number, "toLocaleString"));
        assert!(!prototype_has(PrototypeId::Number, "nonexistent"));
    }

    #[test]
    fn boolean_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Boolean).collect();
        assert_eq!(members.len(), 3);
        assert!(prototype_has(PrototypeId::Boolean, "constructor"));
        assert!(prototype_has(PrototypeId::Boolean, "toString"));
        assert!(prototype_has(PrototypeId::Boolean, "valueOf"));
        assert!(!prototype_has(PrototypeId::Boolean, "nonexistent"));
    }

    #[test]
    fn bigint_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::BigInt).collect();
        assert_eq!(members.len(), 5);
        assert!(prototype_has(PrototypeId::BigInt, "constructor"));
        assert!(prototype_has(PrototypeId::BigInt, "toLocaleString"));
        assert!(prototype_has(PrototypeId::BigInt, "toString"));
        assert!(prototype_has(PrototypeId::BigInt, "valueOf"));
        assert!(prototype_has(PrototypeId::BigInt, "@@toStringTag"));
        assert!(!prototype_has(PrototypeId::BigInt, "nonexistent"));
    }

    #[test]
    fn promise_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Promise).collect();
        assert_eq!(members.len(), 5);
        assert!(prototype_has(PrototypeId::Promise, "constructor"));
        assert!(prototype_has(PrototypeId::Promise, "then"));
        assert!(prototype_has(PrototypeId::Promise, "catch"));
        assert!(prototype_has(PrototypeId::Promise, "finally"));
        assert!(prototype_has(PrototypeId::Promise, "@@toStringTag"));
        assert!(!prototype_has(PrototypeId::Promise, "nonexistent"));
    }

    #[test]
    fn object_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Object).collect();
        assert_eq!(members.len(), 12);
        assert!(prototype_has(PrototypeId::Object, "constructor"));
        assert!(prototype_has(PrototypeId::Object, "hasOwnProperty"));
        assert!(prototype_has(PrototypeId::Object, "isPrototypeOf"));
        assert!(prototype_has(PrototypeId::Object, "propertyIsEnumerable"));
        assert!(prototype_has(PrototypeId::Object, "toLocaleString"));
        assert!(prototype_has(PrototypeId::Object, "toString"));
        assert!(prototype_has(PrototypeId::Object, "valueOf"));
        assert!(prototype_has(PrototypeId::Object, "__defineGetter__"));
        assert!(prototype_has(PrototypeId::Object, "__defineSetter__"));
        assert!(prototype_has(PrototypeId::Object, "__lookupGetter__"));
        assert!(prototype_has(PrototypeId::Object, "__lookupSetter__"));
        assert!(prototype_has(PrototypeId::Object, "__proto__"));
        assert!(!prototype_has(PrototypeId::Object, "nonexistent"));
    }

    #[test]
    fn function_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Function).collect();
        assert_eq!(members.len(), 8);
        assert!(prototype_has(PrototypeId::Function, "constructor"));
        assert!(prototype_has(PrototypeId::Function, "apply"));
        assert!(prototype_has(PrototypeId::Function, "bind"));
        assert!(prototype_has(PrototypeId::Function, "call"));
        assert!(prototype_has(PrototypeId::Function, "toString"));
        assert!(prototype_has(PrototypeId::Function, "@@hasInstance"));
        assert!(prototype_has(PrototypeId::Function, "caller"));
        assert!(prototype_has(PrototypeId::Function, "arguments"));
        assert!(!prototype_has(PrototypeId::Function, "nonexistent"));
    }

    #[test]
    fn map_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Map).collect();
        assert_eq!(members.len(), 15);
        assert!(prototype_has(PrototypeId::Map, "constructor"));
        assert!(prototype_has(PrototypeId::Map, "clear"));
        assert!(prototype_has(PrototypeId::Map, "delete"));
        assert!(prototype_has(PrototypeId::Map, "entries"));
        assert!(prototype_has(PrototypeId::Map, "forEach"));
        assert!(prototype_has(PrototypeId::Map, "get"));
        assert!(prototype_has(PrototypeId::Map, "getOrInsert"));
        assert!(prototype_has(PrototypeId::Map, "getOrInsertComputed"));
        assert!(prototype_has(PrototypeId::Map, "has"));
        assert!(prototype_has(PrototypeId::Map, "keys"));
        assert!(prototype_has(PrototypeId::Map, "set"));
        assert!(prototype_has(PrototypeId::Map, "size"));
        assert!(prototype_has(PrototypeId::Map, "values"));
        assert!(prototype_has(PrototypeId::Map, "@@iterator"));
        assert!(prototype_has(PrototypeId::Map, "@@toStringTag"));
        assert!(!prototype_has(PrototypeId::Map, "nonexistent"));
    }

    #[test]
    fn set_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::Set).collect();
        assert_eq!(members.len(), 19);
        assert!(prototype_has(PrototypeId::Set, "constructor"));
        assert!(prototype_has(PrototypeId::Set, "add"));
        assert!(prototype_has(PrototypeId::Set, "clear"));
        assert!(prototype_has(PrototypeId::Set, "delete"));
        assert!(prototype_has(PrototypeId::Set, "difference"));
        assert!(prototype_has(PrototypeId::Set, "entries"));
        assert!(prototype_has(PrototypeId::Set, "forEach"));
        assert!(prototype_has(PrototypeId::Set, "has"));
        assert!(prototype_has(PrototypeId::Set, "intersection"));
        assert!(prototype_has(PrototypeId::Set, "isDisjointFrom"));
        assert!(prototype_has(PrototypeId::Set, "isSubsetOf"));
        assert!(prototype_has(PrototypeId::Set, "isSupersetOf"));
        assert!(prototype_has(PrototypeId::Set, "keys"));
        assert!(prototype_has(PrototypeId::Set, "size"));
        assert!(prototype_has(PrototypeId::Set, "symmetricDifference"));
        assert!(prototype_has(PrototypeId::Set, "union"));
        assert!(prototype_has(PrototypeId::Set, "values"));
        assert!(prototype_has(PrototypeId::Set, "@@iterator"));
        assert!(prototype_has(PrototypeId::Set, "@@toStringTag"));
        assert!(!prototype_has(PrototypeId::Set, "nonexistent"));
    }

    #[test]
    fn weak_map_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::WeakMap).collect();
        assert_eq!(members.len(), 8);
        assert!(prototype_has(PrototypeId::WeakMap, "constructor"));
        assert!(prototype_has(PrototypeId::WeakMap, "delete"));
        assert!(prototype_has(PrototypeId::WeakMap, "get"));
        assert!(prototype_has(PrototypeId::WeakMap, "getOrInsert"));
        assert!(prototype_has(PrototypeId::WeakMap, "getOrInsertComputed"));
        assert!(prototype_has(PrototypeId::WeakMap, "has"));
        assert!(prototype_has(PrototypeId::WeakMap, "set"));
        assert!(prototype_has(PrototypeId::WeakMap, "@@toStringTag"));
        assert!(!prototype_has(PrototypeId::WeakMap, "nonexistent"));
    }

    #[test]
    fn weak_set_prototype_members_are_complete() {
        let members: Vec<_> = prototype_members(PrototypeId::WeakSet).collect();
        assert_eq!(members.len(), 5);
        assert!(prototype_has(PrototypeId::WeakSet, "constructor"));
        assert!(prototype_has(PrototypeId::WeakSet, "add"));
        assert!(prototype_has(PrototypeId::WeakSet, "delete"));
        assert!(prototype_has(PrototypeId::WeakSet, "has"));
        assert!(prototype_has(PrototypeId::WeakSet, "@@toStringTag"));
        assert!(!prototype_has(PrototypeId::WeakSet, "nonexistent"));
    }
}
