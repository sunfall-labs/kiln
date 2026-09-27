//! What each built-in prototype publishes, in one place.
//!
//! A built-in prototype here is a *marker* rather than an object: `Native::PrototypeOf(table)`
//! has no property store, so every question a program can ask about one of its members --
//! does it exist, what is its value, is it an accessor, what are its flags, does it enumerate --
//! has to be answered by a table. The mistake this module exists to prevent is having *several*
//! tables, because then the answers can disagree, and every one of those disagreements is a
//! silent wrong answer rather than a failure:
//!
//! - a name listed for enumeration with no value behind it appears in
//!   `Object.getOwnPropertyNames` and reads as `undefined`;
//! - an accessor whose name is listed but whose getter is not resolved is described as a data
//!   property holding `undefined`;
//! - a symbol-keyed member written into a *string* list leaks the internal `@@` spelling into
//!   every property listing in the realm. That one has been shipped once already.
//!
//! So the two halves that can drift are joined here, and the one that cannot be expressed is
//! made unspellable: [`MemberKey`] separates a string key from a symbol key by *type*, and
//! [`NamedMembers`] refuses a reserved spelling at compile time rather than at review time.

use crate::{BuiltinAccessor, PrototypeTable};
pub use kiln_builtin_surface::MemberKey;

/// Whether a string is the internal spelling of a symbol.
///
/// A `const fn` because that is the whole point: the check below runs while the compiler is
/// building the tables, so a symbol written into a named list is a build failure and not a
/// property that quietly appears in every listing in the realm.
const fn is_reserved_spelling(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() >= 2 && bytes[0] == b'@' && bytes[1] == b'@'
}

/// A table's string-keyed members, checked when the compiler builds it.
///
/// Constructed only through [`NamedMembers::new`], and only in `const` position -- which is what
/// makes the assertion inside it run at build time. A `&[&str]` would accept anything.
pub struct NamedMembers(&'static [&'static str]);

impl NamedMembers {
    /// # Panics
    ///
    /// At *compile time*, if any name is the internal spelling of a symbol. There is no runtime
    /// path into this: every caller is a `const` item.
    pub const fn new(names: &'static [&'static str]) -> Self {
        let mut index = 0;
        while index < names.len() {
            assert!(
                !is_reserved_spelling(names[index]),
                "a symbol-keyed member cannot be published in a named list: it would leak the \
                 internal `@@` spelling into every property listing in the realm. Publish it \
                 through the value path -- `tabulated_function_property` and friends -- as \
                 `Function.prototype[@@hasInstance]` and `Date.prototype[@@toPrimitive]` are."
            );
            index += 1;
        }
        Self(names)
    }

    /// The names, for listing and for membership tests.
    pub const fn names(&self) -> &'static [&'static str] {
        self.0
    }
}

/// One accessor a built-in prototype publishes about its instances.
///
/// The name and the getter live in the same row because they used to live in two tables --
/// `prototype_accessor_names` listed them and `prototype_accessor` resolved them -- and a name in
/// one but not the other is not a compile error, it is a property that lists and describes as a
/// data property holding `undefined`.
pub struct Accessor {
    pub table: PrototypeTable,
    pub key: MemberKey,
    pub getter: BuiltinAccessor,
}

/// Every accessor published on a built-in prototype.
///
/// Setter-less by construction: none of these has one, which is what makes `re.global = true`
/// and `view.byteLength = 0` do nothing rather than store a shadowing property.
pub static PROTOTYPE_ACCESSORS: &[Accessor] = &[
    Accessor {
        table: PrototypeTable::ArrayBuffer,
        key: MemberKey::Named("byteLength"),
        getter: BuiltinAccessor::BufferByteLength,
    },
    Accessor {
        table: PrototypeTable::ArrayBuffer,
        key: MemberKey::Named("maxByteLength"),
        getter: BuiltinAccessor::BufferMaxByteLength,
    },
    Accessor {
        table: PrototypeTable::ArrayBuffer,
        key: MemberKey::Named("resizable"),
        getter: BuiltinAccessor::BufferResizable,
    },
    Accessor {
        table: PrototypeTable::ArrayBuffer,
        key: MemberKey::Named("detached"),
        getter: BuiltinAccessor::BufferDetached,
    },
    Accessor {
        table: PrototypeTable::ArrayBuffer,
        key: MemberKey::Named("immutable"),
        getter: BuiltinAccessor::BufferImmutable,
    },
    Accessor {
        table: PrototypeTable::SharedArrayBuffer,
        key: MemberKey::Named("byteLength"),
        getter: BuiltinAccessor::SharedByteLength,
    },
    Accessor {
        table: PrototypeTable::SharedArrayBuffer,
        key: MemberKey::Named("maxByteLength"),
        getter: BuiltinAccessor::SharedMaxByteLength,
    },
    Accessor {
        table: PrototypeTable::SharedArrayBuffer,
        key: MemberKey::Named("growable"),
        getter: BuiltinAccessor::SharedGrowable,
    },
    Accessor {
        table: PrototypeTable::DataView,
        key: MemberKey::Named("buffer"),
        getter: BuiltinAccessor::DataViewBuffer,
    },
    Accessor {
        table: PrototypeTable::DataView,
        key: MemberKey::Named("byteLength"),
        getter: BuiltinAccessor::DataViewByteLength,
    },
    Accessor {
        table: PrototypeTable::DataView,
        key: MemberKey::Named("byteOffset"),
        getter: BuiltinAccessor::DataViewByteOffset,
    },
    Accessor {
        table: PrototypeTable::TypedArray,
        key: MemberKey::Named("buffer"),
        getter: BuiltinAccessor::TypedArrayBuffer,
    },
    Accessor {
        table: PrototypeTable::TypedArray,
        key: MemberKey::Named("byteLength"),
        getter: BuiltinAccessor::TypedArrayByteLength,
    },
    Accessor {
        table: PrototypeTable::TypedArray,
        key: MemberKey::Named("byteOffset"),
        getter: BuiltinAccessor::TypedArrayByteOffset,
    },
    Accessor {
        table: PrototypeTable::TypedArray,
        key: MemberKey::Named("length"),
        getter: BuiltinAccessor::TypedArrayLength,
    },
    Accessor {
        table: PrototypeTable::Map,
        key: MemberKey::Named("size"),
        getter: BuiltinAccessor::CollectionSize(true),
    },
    Accessor {
        table: PrototypeTable::Set,
        key: MemberKey::Named("size"),
        getter: BuiltinAccessor::CollectionSize(false),
    },
    Accessor {
        table: PrototypeTable::DisposableStack,
        key: MemberKey::Named("disposed"),
        getter: BuiltinAccessor::DisposableStackDisposed,
    },
    Accessor {
        table: PrototypeTable::AsyncDisposableStack,
        key: MemberKey::Named("disposed"),
        getter: BuiltinAccessor::AsyncDisposableStackDisposed,
    },
];

/// The getter published under `key` on `table`, or `None` for a name that is not an accessor.
pub fn accessor(table: PrototypeTable, key: &str) -> Option<BuiltinAccessor> {
    PROTOTYPE_ACCESSORS
        .iter()
        .find(|row| row.table == table && row.key.property_key() == key)
        .map(|row| row.getter)
}

/// The accessor names `table` publishes, in table order.
pub fn accessor_keys(table: PrototypeTable) -> impl Iterator<Item = &'static str> {
    PROTOTYPE_ACCESSORS
        .iter()
        .filter(move |row| row.table == table)
        .map(|row| row.key.property_key())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The named lists must not contain a symbol's internal spelling.
    ///
    /// `NamedMembers::new` already refuses one while the compiler builds the table, so this is
    /// belt and braces for any list that reaches the type by another route.
    #[test]
    fn named_members_reject_reserved_spellings() {
        assert!(is_reserved_spelling("@@iterator"));
        assert!(!is_reserved_spelling("iterator"));
        assert!(!is_reserved_spelling("@"));
    }

    /// Every accessor row resolves through the same lookup that lists it.
    #[test]
    fn every_listed_accessor_resolves() {
        for row in PROTOTYPE_ACCESSORS {
            let key = row.key.property_key();
            assert!(
                accessor(row.table, key).is_some(),
                "{key} is listed on {:?} but does not resolve",
                row.table
            );
            assert_eq!(accessor(row.table, key), Some(row.getter));
        }
    }

    /// No table publishes the same accessor name twice.
    #[test]
    fn accessor_rows_are_unique() {
        for (index, row) in PROTOTYPE_ACCESSORS.iter().enumerate() {
            let duplicate = PROTOTYPE_ACCESSORS
                .iter()
                .skip(index + 1)
                .any(|other| other.table == row.table && other.key == row.key);
            assert!(
                !duplicate,
                "{:?} publishes {} twice",
                row.table,
                row.key.property_key()
            );
        }
    }
}
