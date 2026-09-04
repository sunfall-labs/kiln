use std::rc::Rc;

/// The well-known symbols, which are the only symbols that exist so far.
///
/// Symbol-keyed properties are stored under a reserved string key (`@@iterator`), the same
/// convention engines use internally. It is not airtight -- a program could write that string as
/// a literal key -- but a separate key space costs a second lookup path everywhere for a
/// collision nobody writes by accident, and the corpus is what would catch it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WellKnown {
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

impl WellKnown {
    /// Every well-known symbol, including the host one `Symbol` does not publish.
    ///
    /// Kept exhaustive by [`WellKnown::ordinal`] rather than by review: that function matches on
    /// `self`, so adding a variant stops the crate compiling until the variant is given an
    /// ordinal, and the `const` block at the end of this file refuses to build unless this list
    /// holds every ordinal exactly once and in order. There used to be a list of fifteen beside
    /// an enum of sixteen, and a `chain` at the one call site that noticed -- which is how a
    /// symbol goes missing from `getOwnPropertySymbols` while every other path still works.
    pub(crate) const ALL: [Self; 16] = [
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

    /// This symbol's position in [`WellKnown::ALL`].
    ///
    /// Exists only so that the compiler has something exhaustive to check. Adding a variant
    /// without adding it to `ALL` fails here.
    const fn ordinal(self) -> usize {
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

    pub fn property_key(self) -> &'static str {
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

    /// The name `Symbol` publishes this one under.
    ///
    /// The key with the reserved prefix taken off. Kept as a function rather than derived at the
    /// call site so that the two spellings cannot drift apart.
    pub(crate) fn js_name(self) -> &'static str {
        self.property_key()
            .strip_prefix(RESERVED_KEY_PREFIX)
            .unwrap_or_else(|| unreachable!("every well-known key carries the prefix"))
    }

    /// The well-known symbol a reserved key names, if it names one.
    ///
    /// The inverse of [`Self::property_key`], and exact: every other `@@` key in this
    /// interpreter is an internal slot -- `@@entries`, `@@disposeEntries`, `@@trace` -- and none
    /// of them collides with a name here. That exactness is what lets reflection tell a
    /// symbol-keyed property apart from the machine's own bookkeeping, which is the whole
    /// difficulty of storing symbols as strings.
    ///
    /// `@@nodeInspectCustom` included: `Symbol` does not publish it, but an object that carries
    /// it carries a symbol-keyed property and `Object.getOwnPropertySymbols` has to say so.
    pub(crate) fn from_property_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|symbol| symbol.property_key() == key)
    }

    /// Whether `Symbol` publishes this one as a static member.
    ///
    /// All but one: `@@nodeInspectCustom` is a host symbol, so `Symbol.nodeInspectCustom` must
    /// not exist -- but an object carrying it still carries a symbol-keyed property, and
    /// `Object.getOwnPropertySymbols` has to say so. That single difference used to be spelled
    /// as two lists of different lengths.
    pub(crate) const fn published_by_symbol(self) -> bool {
        !matches!(self, Self::NodeInspectCustom)
    }

    /// What `String(symbol)` shows, which spells the well-known ones out.
    pub(crate) fn description(self) -> &'static str {
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

/// A symbol: a property key with an identity of its own.
#[derive(Clone, Debug)]
pub enum Symbol {
    /// One of the specification's own symbols. The same one everywhere in the program, so it
    /// needs no allocation and equality is the variant's.
    WellKnown(WellKnown),
    /// `Symbol(description)`. Two calls make two different symbols even given the same
    /// description, so identity is the allocation's and the reserved key carries a serial number.
    /// The description is for display only and is never part of the key.
    Unique(Rc<UniqueSymbol>),
}

#[derive(Debug)]
pub struct UniqueSymbol {
    pub(crate) serial: u64,
    pub(crate) description: Option<String>,
}

impl Symbol {
    /// The reserved string key this symbol names a property under.
    ///
    /// A well-known symbol gets a fixed key so that two mentions of `Symbol.iterator` reach the
    /// same property; a unique one gets its serial, so that two symbols with the same description
    /// do not collide and a program cannot spell either by writing a string.
    pub fn property_key(&self) -> String {
        match self {
            Self::WellKnown(symbol) => symbol.property_key().to_owned(),
            Self::Unique(symbol) => format!("{UNIQUE_SYMBOL_PREFIX}{}", symbol.serial),
        }
    }

    pub(crate) fn description(&self) -> &str {
        match self {
            Self::WellKnown(symbol) => symbol.description(),
            Self::Unique(symbol) => symbol.description.as_deref().unwrap_or(""),
        }
    }

    /// The serial in a unique symbol's reserved key, or `None` for any other key.
    ///
    /// The inverse of the `@@symbol:{serial}` half of [`Self::property_key`]. The serial alone
    /// is not the symbol -- identity is the allocation's, not the number's -- which is why the
    /// realm keeps every one it has made and this only says which to look up.
    pub(crate) fn serial_in_key(key: &str) -> Option<u64> {
        key.strip_prefix(UNIQUE_SYMBOL_PREFIX)?.parse().ok()
    }
}

/// The reserved key prefix a `Symbol(description)` names its properties under.
pub(crate) const UNIQUE_SYMBOL_PREFIX: &str = "@@symbol:";

impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::WellKnown(left), Self::WellKnown(right)) => left == right,
            (Self::Unique(left), Self::Unique(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }
}

/// The prefix the VM writes its own keys under: a symbol-keyed property, or an internal slot.
/// `Object.keys` and `JSON.stringify` enumerate own *string* keys, so both skip them.
pub(crate) const RESERVED_KEY_PREFIX: &str = "@@";

/// The internal slots that are not symbol keys and do not carry one of the prefixes below.
///
/// Spelled out because the alternative -- "anything starting with `@@`" -- is a guess about what
/// a *program* would never write, and it is wrong. `symbol-observable`, which redux and rxjs both
/// depend on, stores its hook under the literal string `"@@observable"`, and that property then
/// vanished from `Object.keys` and `getOwnPropertyNames` while remaining readable by name. A
/// program's own data disappearing from enumeration is the kind of wrong answer that surfaces far
/// from its cause.
///
/// `every_internal_slot_is_reserved` in `tests.rs` scans this crate for `"@@..."` literals and
/// fails if one is missing from here, so a new slot cannot quietly start enumerating.
const INTERNAL_SLOTS: [&str; 39] = [
    "@@abort-listeners",
    "@@asyncDisposeDone",
    "@@asyncDisposeEntries",
    "@@call-site-name",
    "@@concat-index",
    "@@concat-sources",
    "@@disposeDone",
    "@@disposeEntries",
    "@@entries",
    "@@finreg-callback",
    "@@finreg-cells",
    "@@helper-arg",
    "@@helper-buffer",
    "@@helper-counter",
    "@@helper-done",
    "@@helper-inner",
    "@@helper-inner-next",
    "@@helper-next",
    "@@helper-partial",
    "@@helper-source",
    "@@index",
    "@@iterated",
    "@@map",
    "@@matchall-global",
    "@@matchall-regexp",
    "@@matchall-subject",
    "@@matchall-unicode",
    "@@part",
    "@@raw-json",
    "@@size",
    "@@trace",
    "@@weak",
    "@@weakref-target",
    "@@zip-iters",
    "@@zip-keyed",
    "@@zip-keys",
    "@@zip-mode",
    "@@zip-open",
    "@@zip-padding",
];

/// The families of internal key whose members are generated rather than listed: a unique symbol's
/// serial, a private name's scope, and the two host-slot namespaces.
const INTERNAL_PREFIXES: [&str; 4] = ["@@symbol:", "@@private:", "@@kiln:", "@@node:"];

/// Whether `key` is one the VM wrote, rather than one a program did.
///
/// Exact rather than a prefix test -- see [`INTERNAL_SLOTS`] for what a prefix test cost.
pub(crate) fn is_reserved(key: &str) -> bool {
    if !key.starts_with(RESERVED_KEY_PREFIX) {
        return false;
    }
    WellKnown::from_property_key(key).is_some()
        || INTERNAL_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
        || INTERNAL_SLOTS.contains(&key)
}

/// `WellKnown::ALL` holds every variant, once, in ordinal order -- checked while the compiler
/// builds it, because a list that is merely believed to be complete is not one.
const _: () = {
    let mut index = 0;
    while index < WellKnown::ALL.len() {
        assert!(
            WellKnown::ALL[index].ordinal() == index,
            "WellKnown::ALL must hold every well-known symbol exactly once, in ordinal order"
        );
        index += 1;
    }
};
