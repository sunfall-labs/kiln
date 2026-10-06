use std::rc::Rc;

use kiln_string::StringUnits;

/// The well-known symbols, defined authoritatively in `kiln-builtin-surface` (ADR 0054, ADR 0059).
pub use kiln_builtin_surface::WellKnownSymbol as WellKnown;

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
    pub(crate) description: Option<StringUnits>,
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

    pub(crate) fn description_units(&self) -> Option<StringUnits> {
        match self {
            Self::WellKnown(symbol) => Some(StringUnits::from(symbol.description())),
            Self::Unique(symbol) => symbol.description.clone(),
        }
    }

    /// Lossy spelling used only in diagnostics and host display paths.
    pub(crate) fn description(&self) -> String {
        self.description_units()
            .map_or_else(String::new, |description| description.to_lossy_string())
    }

    /// SymbolDescriptiveString for language conversions, preserving UTF-16 code units.
    pub(crate) fn descriptive_string_units(&self) -> StringUnits {
        let description = self.description_units().unwrap_or_default();
        let mut units = Vec::with_capacity(description.unit_len() + 8);
        units.extend("Symbol(".encode_utf16());
        units.extend(description.units());
        units.push(u16::from(b')'));
        StringUnits::from_units(&units)
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
const INTERNAL_SLOTS: [&str; 40] = [
    "@@abort-listeners",
    "@@abort-dispatch",
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
    WellKnown::from_property_key(key).is_some() || is_internal_key(key)
}

/// Whether this is a VM-owned reserved spelling rather than a well-known symbol key.
///
/// Kept separate from `is_reserved` so a property lookup that has already classified whether
/// the key is a symbol does not scan `WellKnown::ALL` a second time.
pub(crate) fn is_internal_key(key: &str) -> bool {
    // Every exact slot and generated internal family is under `@@`. Most property names have no
    // reserved prefix, so skip both tables instead of comparing an ordinary key against all 40
    // exact slots on every property lookup.
    if !key.starts_with(RESERVED_KEY_PREFIX) {
        return false;
    }
    INTERNAL_PREFIXES
        .iter()
        .any(|prefix| key.starts_with(prefix))
        || INTERNAL_SLOTS.contains(&key)
}

#[cfg(test)]
mod tests {
    use super::{INTERNAL_SLOTS, is_internal_key};

    #[test]
    fn internal_key_classification_skips_ordinary_names_without_widening_reserved_keys() {
        for key in [
            "map",
            "length",
            "value",
            "@@observable",
            "@@symbol-observable",
        ] {
            assert!(!is_internal_key(key), "{key} is a program-visible key");
        }

        for key in INTERNAL_SLOTS {
            assert!(is_internal_key(key), "{key} is an internal slot");
        }

        for key in [
            "@@symbol:1",
            "@@private:scope:1",
            "@@kiln:host-slot",
            "@@node:timer-handle",
        ] {
            assert!(is_internal_key(key), "{key} belongs to an internal family");
        }

        for key in ["@symbol:1", "@@symbol", "@@private", "@@kiln", "@@node"] {
            assert!(!is_internal_key(key), "{key} is not a complete family key");
        }
    }
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
