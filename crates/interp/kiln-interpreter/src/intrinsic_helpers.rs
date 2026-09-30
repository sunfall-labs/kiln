use super::*;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

// A short, fixed admission limit keeps the process-wide cache small even when
// an application constructs a million unique patterns. Unicode property sets
// and Unicode-set expressions can expand well beyond their source spelling,
// and counted repetitions can compile a short pattern into disproportionately
// large code, so neither is admitted. This bounds retained source/key volume;
// regress does not expose compiled allocation size. `RegExpData` still owns
// all observable state; this stores compiled code only.
const REGEXP_CACHE_MAX_ENTRIES: usize = 64;
const REGEXP_CACHE_MAX_KEY_BYTES: usize = 8 * 1024;
const REGEXP_CACHE_MAX_PATTERN_UNITS: usize = 128;

#[derive(Debug, PartialEq, Eq)]
struct RegExpCacheKey {
    pattern: Box<[u16]>,
    flags: Box<str>,
}

impl RegExpCacheKey {
    fn retained_bytes(&self) -> usize {
        self.pattern.len() * std::mem::size_of::<u16>() + self.flags.len()
    }
}

struct CompiledRegExpEntry {
    key: RegExpCacheKey,
    compiled: Arc<regress::Regex>,
}

#[derive(Default)]
struct CompiledRegExpCache {
    entries: VecDeque<CompiledRegExpEntry>,
    retained_key_bytes: usize,
}

impl CompiledRegExpCache {
    fn get(&mut self, pattern: &[u16], flags: &str) -> Option<Arc<regress::Regex>> {
        let index = self.entries.iter().position(|entry| {
            entry.key.pattern.as_ref() == pattern && entry.key.flags.as_ref() == flags
        })?;
        let entry = self.entries.remove(index)?;
        let compiled = Arc::clone(&entry.compiled);
        self.entries.push_back(entry);
        Some(compiled)
    }

    fn insert(
        &mut self,
        pattern: &[u16],
        flags: &str,
        compiled: Arc<regress::Regex>,
    ) -> Arc<regress::Regex> {
        if let Some(existing) = self.get(pattern, flags) {
            return existing;
        }
        let key = RegExpCacheKey {
            pattern: pattern.into(),
            flags: flags.into(),
        };
        let key_bytes = key.retained_bytes();
        if key_bytes > REGEXP_CACHE_MAX_KEY_BYTES {
            return compiled;
        }
        while self.entries.len() >= REGEXP_CACHE_MAX_ENTRIES
            || self.retained_key_bytes + key_bytes > REGEXP_CACHE_MAX_KEY_BYTES
        {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.retained_key_bytes -= oldest.key.retained_bytes();
            drop(oldest);
        }
        self.retained_key_bytes += key_bytes;
        self.entries.push_back(CompiledRegExpEntry {
            key,
            compiled: Arc::clone(&compiled),
        });
        compiled
    }
}

fn compiled_regexp_cache() -> &'static Mutex<CompiledRegExpCache> {
    static CACHE: OnceLock<Mutex<CompiledRegExpCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(CompiledRegExpCache::default()))
}

fn regexp_cache_admits(units: &[u16], flags: &str) -> bool {
    units.len() <= REGEXP_CACHE_MAX_PATTERN_UNITS
        && flags.len() <= 7
        && !flags.contains('u')
        && !flags.contains('v')
        // Conservatively exclude braces even when escaped or literal. This
        // avoids retaining code expanded from counted repetition syntax.
        && !units.contains(&(b'{' as u16))
}

#[cfg(test)]
mod regexp_cache_tests {
    use super::*;

    #[test]
    fn admission_is_limited_to_short_non_unicode_patterns() {
        assert!(regexp_cache_admits(
            &[b'\\' as u16, b'S' as u16, b'+' as u16],
            "g"
        ));
        assert!(!regexp_cache_admits(
            &[b'x' as u16; REGEXP_CACHE_MAX_PATTERN_UNITS + 1],
            ""
        ));
        assert!(!regexp_cache_admits(&[b'x' as u16], "u"));
        assert!(!regexp_cache_admits(&[b'x' as u16], "v"));
        assert!(!regexp_cache_admits(
            &[
                b'x' as u16,
                b'{' as u16,
                b'1' as u16,
                b'0' as u16,
                b'0' as u16,
                b'}' as u16
            ],
            ""
        ));
    }

    #[test]
    fn cache_bounds_entry_count_and_retained_key_bytes() {
        let compiled = Arc::new(regress::Regex::new("x").expect("valid pattern"));
        let mut count_bounded = CompiledRegExpCache::default();
        for index in 0..REGEXP_CACHE_MAX_ENTRIES + 1 {
            count_bounded.insert(&[index as u16], "", Arc::clone(&compiled));
        }
        assert_eq!(count_bounded.entries.len(), REGEXP_CACHE_MAX_ENTRIES);
        assert!(count_bounded.retained_key_bytes <= REGEXP_CACHE_MAX_KEY_BYTES);
        assert!(count_bounded.get(&[0], "").is_none());
        assert!(
            count_bounded
                .get(&[REGEXP_CACHE_MAX_ENTRIES as u16], "")
                .is_some()
        );

        let pattern = vec![b'x' as u16; REGEXP_CACHE_MAX_PATTERN_UNITS];
        let mut byte_bounded = CompiledRegExpCache::default();
        let max_keys = REGEXP_CACHE_MAX_KEY_BYTES / (pattern.len() * 2);
        for index in 0..=max_keys {
            let mut unique = pattern.clone();
            unique[0] = index as u16;
            byte_bounded.insert(&unique, "", Arc::clone(&compiled));
        }
        assert!(byte_bounded.retained_key_bytes <= REGEXP_CACHE_MAX_KEY_BYTES);
        assert!(byte_bounded.entries.len() <= REGEXP_CACHE_MAX_ENTRIES);
    }
}

/// The methods a string exposes.
///
/// Strings are primitives here rather than objects with a prototype, so their methods are looked
/// up directly. When `String.prototype` becomes a real object -- which subclassing and
/// monkey-patching both need -- this table is what moves onto it.
pub(super) fn string_property(name: &str) -> Value {
    match name {
        // A string carries `@@iterator` and no `values` beside it, unlike an array: node answers
        // `undefined` for `"abc".values`, and `String.prototype` has no such own name. The
        // iterator itself is unchanged -- it is the arm below, reaching the same function.
        key if key == WellKnown::Iterator.property_key() => {
            Value::Native(Native::StringValues, RealmId::PRIMARY)
        }
        "toUpperCase" => Value::Native(Native::StringToUpperCase, RealmId::PRIMARY),
        "toLowerCase" => Value::Native(Native::StringToLowerCase, RealmId::PRIMARY),
        "includes" => Value::Native(Native::StringIncludes, RealmId::PRIMARY),
        "indexOf" => Value::Native(Native::StringIndexOf, RealmId::PRIMARY),
        "slice" => Value::Native(Native::StringSlice, RealmId::PRIMARY),
        "substring" => Value::Native(Native::StringSubstring, RealmId::PRIMARY),
        // Deprecated in the specification and still everywhere in the wild.
        "substr" => Value::Native(Native::StringSubstr, RealmId::PRIMARY),
        "split" => Value::Native(Native::StringSplit, RealmId::PRIMARY),
        "trim" => Value::Native(Native::StringTrim, RealmId::PRIMARY),
        // Annex B's aliases, and the *same* functions rather than copies:
        // `String.prototype.trimLeft === String.prototype.trimStart` is true.
        "trimLeft" => Value::Native(Native::StringTrimStart, RealmId::PRIMARY),
        "trimRight" => Value::Native(Native::StringTrimEnd, RealmId::PRIMARY),
        "startsWith" => Value::Native(Native::StringStartsWith, RealmId::PRIMARY),
        "endsWith" => Value::Native(Native::StringEndsWith, RealmId::PRIMARY),
        "repeat" => Value::Native(Native::StringRepeat, RealmId::PRIMARY),
        "at" => Value::Native(Native::StringAt, RealmId::PRIMARY),
        "codePointAt" => Value::Native(Native::StringCodePointAt, RealmId::PRIMARY),
        // No locale data, so this is the ordinary code-unit order -- which is what a program
        // sorting ASCII gets from either.
        // ECMA-402 replaces this one outright: it is `Intl.Collator` built from the second and
        // third arguments, which is why a locale it rejects is rejected here too.
        "localeCompare" => Value::Native(
            Native::Intl(intl::Op::Adapter(intl::Adapter::StringLocaleCompare)),
            RealmId::PRIMARY,
        ),
        "toLocaleUpperCase" => Value::Native(
            Native::Intl(intl::Op::Adapter(intl::Adapter::StringToLocaleUpperCase)),
            RealmId::PRIMARY,
        ),
        // ECMA-402 gives these two their own definitions -- they validate the locale list and
        // may take a locale-specific mapping -- so they are separate functions rather than
        // aliases of `toLowerCase` and `toUpperCase`.
        "toLocaleLowerCase" => Value::Native(
            Native::Intl(intl::Op::Adapter(intl::Adapter::StringToLocaleLowerCase)),
            RealmId::PRIMARY,
        ),
        "normalize" => Value::Native(Native::StringNormalize, RealmId::PRIMARY),
        "charAt" => Value::Native(Native::StringCharAt, RealmId::PRIMARY),
        "charCodeAt" => Value::Native(Native::StringCharCodeAt, RealmId::PRIMARY),
        "padStart" => Value::Native(Native::StringPadStart, RealmId::PRIMARY),
        "padEnd" => Value::Native(Native::StringPadEnd, RealmId::PRIMARY),
        "replace" => Value::Native(Native::StringReplace, RealmId::PRIMARY),
        "replaceAll" => Value::Native(Native::StringReplaceAll, RealmId::PRIMARY),
        "lastIndexOf" => Value::Native(Native::StringLastIndexOf, RealmId::PRIMARY),
        "trimStart" => Value::Native(Native::StringTrimStart, RealmId::PRIMARY),
        "trimEnd" => Value::Native(Native::StringTrimEnd, RealmId::PRIMARY),
        "concat" => Value::Native(Native::StringConcat, RealmId::PRIMARY),
        // A string's `toString` and `valueOf` both hand back the string itself. They are rarely
        // written by hand and constantly reached by library code that calls `toString` on
        // something it has already coerced -- but they are still two functions with two names,
        // and `String.prototype.toLocaleString` is neither: it is inherited from
        // `Object.prototype`, so publishing one here made it an own property that is not one.
        "toString" => Value::Native(Native::StringToStringMethod, RealmId::PRIMARY),
        "valueOf" => Value::Native(Native::StringValueOf, RealmId::PRIMARY),
        "match" => Value::Native(Native::StringMatch, RealmId::PRIMARY),
        "matchAll" => Value::Native(Native::StringMatchAll, RealmId::PRIMARY),
        "search" => Value::Native(Native::StringSearch, RealmId::PRIMARY),
        _ => Value::Undefined,
    }
}

/// `JSON.rawJSON(text)`: an object carrying JSON text the serialiser will emit unchanged.
///
/// Only a *primitive* text is accepted. An object or an array would have to be re-serialised
/// with the indentation and the replacer the enclosing `stringify` was given, and pasting it in
/// whole would ignore both -- so the specification refuses the case rather than producing a
/// document whose shape depends on where the raw value happened to sit.
pub(super) fn build_raw_json(text: &StringUnits) -> Result<Value, ExecutionError> {
    // Not `trim`: surrounding whitespace is refused rather than removed, because the text is
    // going into a document verbatim and a leading newline would be there too.
    if text.is_empty()
        || text.unit_at(0).is_some_and(is_json_whitespace)
        || text
            .unit_at(text.unit_len().saturating_sub(1))
            .is_some_and(is_json_whitespace)
    {
        return Err(ExecutionError::InvalidJson);
    }
    let parsed = parse_json_text(text)?;
    if !matches!(
        parsed,
        Value::Null | Value::Boolean(_) | Value::Number(_) | Value::String(_)
    ) {
        return Err(ExecutionError::InvalidJson);
    }
    let mut object = ObjectData::default();
    object.set(RAW_JSON_TEXT, Value::String(text.clone()));
    object.set_non_enumerable(RAW_JSON_MARK, Value::Boolean(true));
    // A null prototype, so the object cannot answer to `toJSON`, `toString` or anything else a
    // program might have put on `Object.prototype` -- it is a carrier, not a participant.
    object.set_prototype(Value::Null);
    Ok(Value::Object(Rc::new(RefCell::new(object))))
}

/// What a `WeakRef` inherits from `WeakRef.prototype`.
/// What a `DisposableStack` inherits from `DisposableStack.prototype`.
///
/// `dispose` and `[Symbol.dispose]` answer with the *same* value, which is what the
/// specification says they are -- a program that stores one and compares it with the other has
/// to find them equal, and that is the whole of how `using` reaches a stack.
/// Where an error records the stack it was made on.
///
/// Reserved, so it never enumerates -- and it is also the only mark that separates a real error
/// from an object shaped like one, which is what `Error.isError` answers from.
pub(super) const ERROR_TRACE: &str = "@@trace";

/// The reserved slots a `DisposableStack` carries.
pub(super) const DISPOSE_ENTRIES: &str = "@@disposeEntries";
pub(super) const DISPOSE_DONE: &str = "@@disposeDone";

/// The same pair for an `AsyncDisposableStack`, under names of its own.
///
/// Separate names rather than a shared pair with a flag, because the brand check *is* the
/// distinction: `DisposableStack.prototype.use.call(asyncStack)` is a `TypeError`, and looking
/// for a slot the other kind does not have is the whole of how that is decided.
pub(super) const ASYNC_DISPOSE_ENTRIES: &str = "@@asyncDisposeEntries";
pub(super) const ASYNC_DISPOSE_DONE: &str = "@@asyncDisposeDone";

/// Which of the two stacks an operation belongs to.
///
/// The pair is one object driven two ways. The slots, the brand check and the recorded
/// `[method, receiver, arguments]` triple are the same shape for both; what differs is the name
/// the disposer is looked up under, and whether what it returns is awaited before the next one
/// runs. Everything the two share is written once and told which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Disposal {
    Sync,
    Async,
}

impl Disposal {
    pub(super) const fn entries_key(self) -> &'static str {
        match self {
            Self::Sync => DISPOSE_ENTRIES,
            Self::Async => ASYNC_DISPOSE_ENTRIES,
        }
    }

    pub(super) const fn done_key(self) -> &'static str {
        match self {
            Self::Sync => DISPOSE_DONE,
            Self::Async => ASYNC_DISPOSE_DONE,
        }
    }

    pub(super) const fn type_name(self) -> &'static str {
        match self {
            Self::Sync => "DisposableStack",
            Self::Async => "AsyncDisposableStack",
        }
    }

    /// The name a value has to carry a disposer under, as a diagnostic spells it.
    pub(super) const fn hint_name(self) -> &'static str {
        match self {
            Self::Sync => "[Symbol.dispose]",
            Self::Async => "[Symbol.asyncDispose] or [Symbol.dispose]",
        }
    }
}

/// One recorded call: what to invoke, against what, with what.
pub(super) fn disposal(method: Value, this_value: Value, arguments: Vec<Value>) -> Value {
    Value::Array(ArrayData::new(vec![
        method,
        this_value,
        Value::Array(ArrayData::new(arguments)),
    ]))
}

/// `new DisposableStack()` or `new AsyncDisposableStack()`: empty, and not yet disposed.
pub(super) fn new_disposable_stack(flavour: Disposal) -> Value {
    let mut data = ObjectData::default();
    data.set_non_enumerable(
        flavour.entries_key(),
        Value::Array(ArrayData::new(Vec::new())),
    );
    data.set_non_enumerable(flavour.done_key(), Value::Boolean(false));
    data.set_prototype(Value::Native(
        Native::PrototypeOf(match flavour {
            Disposal::Sync => PrototypeTable::DisposableStack,
            Disposal::Async => PrototypeTable::AsyncDisposableStack,
        }),
        RealmId::PRIMARY,
    ));
    Value::Object(Rc::new(RefCell::new(data)))
}

pub(super) fn disposable_stack_method(name: &str) -> Value {
    match name {
        "use" => Value::Native(Native::DisposableStackUse, RealmId::PRIMARY),
        "adopt" => Value::Native(Native::DisposableStackAdopt, RealmId::PRIMARY),
        "defer" => Value::Native(Native::DisposableStackDefer, RealmId::PRIMARY),
        "move" => Value::Native(Native::DisposableStackMove, RealmId::PRIMARY),
        "dispose" => Value::Native(Native::DisposableStackDispose, RealmId::PRIMARY),
        key if key == WellKnown::Dispose.property_key() => {
            Value::Native(Native::DisposableStackDispose, RealmId::PRIMARY)
        }
        key if key == WellKnown::ToStringTag.property_key() => {
            Value::String("DisposableStack".to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// What an `AsyncDisposableStack` inherits from `AsyncDisposableStack.prototype`.
///
/// `disposeAsync` and `[Symbol.asyncDispose]` answer with the same value, for the same reason
/// the synchronous pair do: the specification says the two properties hold one function, and
/// `await using` reaches a stack through the symbol.
pub(super) fn async_disposable_stack_method(name: &str) -> Value {
    match name {
        "use" => Value::Native(Native::AsyncDisposableStackUse, RealmId::PRIMARY),
        "adopt" => Value::Native(Native::AsyncDisposableStackAdopt, RealmId::PRIMARY),
        "defer" => Value::Native(Native::AsyncDisposableStackDefer, RealmId::PRIMARY),
        "move" => Value::Native(Native::AsyncDisposableStackMove, RealmId::PRIMARY),
        "disposeAsync" => Value::Native(Native::AsyncDisposableStackDisposeAsync, RealmId::PRIMARY),
        key if key == WellKnown::AsyncDispose.property_key() => {
            Value::Native(Native::AsyncDisposableStackDisposeAsync, RealmId::PRIMARY)
        }
        key if key == WellKnown::ToStringTag.property_key() => {
            Value::String("AsyncDisposableStack".to_owned().into())
        }
        _ => Value::Undefined,
    }
}

pub(super) fn weak_ref_method(name: &str) -> Value {
    match name {
        "deref" => Value::Native(Native::WeakRefDeref, RealmId::PRIMARY),
        key if key == WellKnown::ToStringTag.property_key() => {
            Value::String("WeakRef".to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// `ShadowRealm.prototype`'s two methods, which are the whole of what a shadow realm can do.
///
/// Deliberately two and no more: there is no way to reach the realm's global, no way to hand it
/// an object, and no way to get one back. Every addition here would be a new hole in the
/// membrane, so the shortness of this list is a property worth keeping.
pub(super) fn shadow_realm_method(name: &str) -> Value {
    match name {
        "evaluate" => Value::Native(Native::ShadowRealmEvaluate, RealmId::PRIMARY),
        "importValue" => Value::Native(Native::ShadowRealmImportValue, RealmId::PRIMARY),
        key if key == WellKnown::ToStringTag.property_key() => {
            Value::String("ShadowRealm".to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// What a `FinalizationRegistry` inherits from `FinalizationRegistry.prototype`.
///
/// `cleanupSome` is absent rather than a no-op: it is a separate proposal, and a method that
/// answered "the cleanup has run" without ever being able to run one would be a claim a program
/// could build on and be wrong about.
pub(super) fn finalization_registry_method(name: &str) -> Value {
    match name {
        "register" => Value::Native(Native::FinalizationRegistryRegister, RealmId::PRIMARY),
        "unregister" => Value::Native(Native::FinalizationRegistryUnregister, RealmId::PRIMARY),
        key if key == WellKnown::ToStringTag.property_key() => {
            Value::String("FinalizationRegistry".to_owned().into())
        }
        _ => Value::Undefined,
    }
}

/// The slot that marks a raw JSON object, and the property that carries its text.
///
/// Two, because the text is a property a program reads -- `JSON.rawJSON(1).rawJSON` is `"1"` --
/// while the mark is not: the object is required to have exactly one own property, so the thing
/// that says "this is raw JSON" has to be invisible to `Object.getOwnPropertyNames`. The
/// reserved prefix is what makes it so.
pub(super) const RAW_JSON_MARK: &str = "@@raw-json";
pub(super) const RAW_JSON_TEXT: &str = "rawJSON";

/// The slot a `WeakRef` keeps its target in, and the two a registry keeps its state in.
pub(super) const WEAK_REF_TARGET: &str = "@@weakref-target";
pub(super) const REGISTRY_CELLS: &str = "@@finreg-cells";
pub(super) const REGISTRY_CALLBACK: &str = "@@finreg-callback";

/// One of the five symbol-keyed methods on `RegExp.prototype`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegExpSymbol {
    Match,
    MatchAll,
    Replace,
    Search,
    Split,
}

impl RegExpSymbol {
    const ALL: [Self; 5] = [
        Self::Match,
        Self::MatchAll,
        Self::Replace,
        Self::Search,
        Self::Split,
    ];

    pub(super) const fn symbol(self) -> WellKnown {
        match self {
            Self::Match => WellKnown::Match,
            Self::MatchAll => WellKnown::MatchAll,
            Self::Replace => WellKnown::Replace,
            Self::Search => WellKnown::Search,
            Self::Split => WellKnown::Split,
        }
    }

    /// The name a stack trace prints, which for a symbol-keyed method is the key in brackets.
    pub(super) fn js_name(self) -> String {
        format!("[Symbol.{}]", self.symbol().js_name())
    }

    pub(super) fn named(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|found| found.symbol().property_key() == key)
    }
}

/// One of the four copying array methods.
///
/// Each is the counterpart of a method that mutates: `with` to an index assignment, `toReversed`
/// to `reverse`, `toSorted` to `sort`, `toSpliced` to `splice`. What they share is the shape --
/// read the receiver, build a new array, leave the receiver untouched -- which is why one marker
/// carries all four.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayCopy {
    With,
    ToReversed,
    ToSorted,
    ToSpliced,
}

impl ArrayCopy {
    pub(super) const fn property_key(self) -> &'static str {
        match self {
            Self::With => "with",
            Self::ToReversed => "toReversed",
            Self::ToSorted => "toSorted",
            Self::ToSpliced => "toSpliced",
        }
    }
}

/// One of the seven operations `Set.prototype` gained in ES2025.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetOperation {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
    IsSubsetOf,
    IsSupersetOf,
    IsDisjointFrom,
}

impl SetOperation {
    pub(super) const fn property_key(self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Intersection => "intersection",
            Self::Difference => "difference",
            Self::SymmetricDifference => "symmetricDifference",
            Self::IsSubsetOf => "isSubsetOf",
            Self::IsSupersetOf => "isSupersetOf",
            Self::IsDisjointFrom => "isDisjointFrom",
        }
    }
}

/// What one of the seven operations needs from its argument.
///
/// A *set-like*, not a Set: the specification asks only for a `size`, a `has` and a `keys`, and
/// reads all three before it does anything with them. That order is observable -- a program can
/// watch which getter runs first -- and it is what makes `new Set([1]).union(m)` work for any `m`
/// that answers those three.
pub(super) struct SetLike {
    pub(super) object: Value,
    pub(super) size: f64,
    pub(super) has: Value,
    pub(super) keys: Value,
}

/// The entries of a Set receiver, or the `TypeError` a method on the wrong receiver raises.
///
/// A Map is refused as firmly as a number is: it has entries, but no answer to what its union
/// with something else would contain.
pub(super) fn require_set(receiver: &Value) -> Result<Rc<ArrayData>, ExecutionError> {
    collection_state(receiver)
        .filter(|(_, is_map)| !is_map && !is_weak_collection(receiver))
        .map(|(entries, _)| entries)
        .ok_or_else(|| ExecutionError::TypeErrorMessage {
            message: "Method called on an incompatible receiver".to_owned(),
        })
}

/// `CanonicalizeKeyedCollectionKey`: `-0` is stored and reported as `+0`.
///
/// The one place a keyed collection departs from `SameValue`. Without it `new Set([-0]).has(0)`
/// and the value a program gets back out of the set disagree.
pub(super) fn canonical_key(value: Value) -> Value {
    match value {
        Value::Number(number) => {
            if number == 0.0 {
                Value::Number(0.0)
            } else {
                Value::Number(number)
            }
        }
        other => other,
    }
}

/// Whether a set's backing entries already hold this value, skipping the tombstones a delete
/// leaves behind.
pub(super) fn set_data_has(entries: &Rc<ArrayData>, value: &Value) -> bool {
    set_data_index(entries, value).is_some()
}

/// Where a set's backing entries hold this value, or `None`.
pub(super) fn set_data_index(entries: &Rc<ArrayData>, value: &Value) -> Option<usize> {
    find_collection_entry(entries, value, false)
}

/// The live values of a set's backing entries, in insertion order.
pub(super) fn set_data_values(entries: &Rc<ArrayData>) -> Vec<Value> {
    entries
        .borrow()
        .iter()
        .filter(|entry| !matches!(entry, Value::Native(Native::Tombstone, _)))
        .cloned()
        .collect()
}

/// A fresh `Set` holding exactly these values.
///
/// Built directly rather than through `add`, because the values are already deduplicated and
/// canonical by the time an operation has them -- and because `add` is a property a program can
/// replace, which must not change what `union` answers.
pub(super) fn new_set(values: Vec<Value>) -> Value {
    let size = values.len() as f64;
    let mut data = ObjectData::default();
    data.set("@@entries", Value::Array(ArrayData::new(values)));
    data.set_prototype(Value::Native(
        Native::PrototypeOf(PrototypeTable::Set),
        RealmId::PRIMARY,
    ));
    data.set(COLLECTION_SIZE, Value::Number(size));
    Value::Object(Rc::new(RefCell::new(data)))
}

/// Whether this value is one of the two weak collections.
pub(super) fn is_weak_collection(receiver: &Value) -> bool {
    matches!(receiver, Value::Object(object)
        if matches!(object.borrow().get_own("@@weak"), Some(&Value::Boolean(true))))
}

/// The backing entries of the collection `native` is a method of, or the `TypeError` the
/// specification raises when the receiver does not have that collection's internal slot.
///
/// Which brand a method demands is decided by the method, not by the receiver: `add`, `has` and
/// `delete` are one name apiece across four collections, and the one the property lookup handed
/// back is the one whose slot the receiver must have. Answering a benign default instead made
/// `Set.prototype.add.call(new Map(), 1)` -- and every other mistake of the same shape -- look
/// like it had worked.
pub(super) fn require_collection(
    receiver: &Value,
    native: Native,
) -> Result<(Rc<ArrayData>, bool), ExecutionError> {
    let wanted = match native {
        Native::SetAdd | Native::SetHas | Native::SetDelete => Some((false, false)),
        Native::MapSet | Native::MapGet | Native::MapHas | Native::MapDelete => Some((true, false)),
        Native::WeakSetAdd | Native::WeakSetHas | Native::WeakSetDelete => Some((false, true)),
        Native::WeakMapSet | Native::WeakMapGet | Native::WeakMapHas | Native::WeakMapDelete => {
            Some((true, true))
        }
        Native::CollectionGetOrInsert { weak, .. } => Some((true, weak)),
        // `clear`, `forEach` and the three iteration methods share an implementation between
        // Map and Set, and the payload is which of the two this one is: a weak collection
        // carries none of them, and a Set is not a Map.
        Native::CollectionClear(is_map)
        | Native::CollectionForEach(is_map)
        | Native::CollectionKeys(is_map)
        | Native::CollectionValues(is_map)
        | Native::CollectionEntries(is_map) => Some((is_map, false)),
        _ => None,
    };
    let weak = is_weak_collection(receiver);
    collection_state(receiver)
        .filter(|(_, is_map)| match wanted {
            Some(brand) => brand == (*is_map, weak),
            None => !weak,
        })
        .ok_or_else(|| ExecutionError::TypeErrorMessage {
            message: "Method called on an incompatible receiver".to_owned(),
        })
}

/// The `@@iterated` field of an iterator this realm made, or the `TypeError` the specification
/// raises for a receiver that is not one.
///
/// The marker is the brand: a plain object that merely carries a `next` is not an array
/// iterator, and reporting a walk that finished before it started hid the mistake.
/// The iterator `entries()`, `keys()` and `values()` answer with.
///
/// It keeps the receiver rather than a copy of it, which is the whole point: a write between the
/// call that made the iterator and the `next()` that reaches that position has to be visible, and
/// a snapshot is a promise that it will not be. The state lives in reserved properties on the
/// iterator itself, so `next` reads the receiver as it is each step.
pub(super) fn array_iterator_over(native: Native, iterated: &Value) -> Value {
    let mut iterator = ObjectData::default();
    // On `%ArrayIteratorPrototype%`, which owns `next` and the tag and itself sits on
    // `%IteratorPrototype%` -- so `map` and `take` are one link further up and
    // `[...].values().map(f)` still finds them. The iterator object now owns nothing, which is
    // what `Object.getOwnPropertyNames` on one reports.
    iterator.set_prototype(Value::Native(
        Native::PrototypeOf(PrototypeTable::IteratorFor(IteratorKind::Array)),
        RealmId::PRIMARY,
    ));
    iterator.set("@@iterated", iterated.clone());
    iterator.set("@@index", Value::Number(0.0));
    iterator.set(
        "@@part",
        Value::String(
            match native {
                Native::ArrayEntries => "entry",
                Native::ArrayKeys => "key",
                _ => "value",
            }
            .to_owned()
            .into(),
        ),
    );

    // An iterator is itself iterable, which is what lets one be passed straight to another
    // `for-of` or to `Array.from`.
    iterator.set(
        WellKnown::Iterator.property_key(),
        Value::Native(Native::IteratorSelf, RealmId::PRIMARY),
    );
    Value::Object(Rc::new(RefCell::new(iterator)))
}

pub(super) fn iterator_state(receiver: &Value) -> Result<Value, ExecutionError> {
    match receiver {
        Value::Object(object) => object
            .borrow()
            .get_own("@@iterated")
            .cloned()
            .ok_or(ExecutionError::NotAnIterator),
        _ => Err(ExecutionError::NotAnIterator),
    }
}

/// The backing entries of a Set or Map, and whether it is a Map.
pub(super) fn collection_state(receiver: &Value) -> Option<(Rc<ArrayData>, bool)> {
    let Value::Object(object) = receiver else {
        return None;
    };
    let object = object.borrow();
    let Some(Value::Array(entries)) = object.get_own("@@entries").cloned() else {
        return None;
    };
    let is_map = matches!(object.get_own("@@map"), Some(&Value::Boolean(true)));
    Some((entries, is_map))
}

/// SameValue, which is what `Object.is` reports and what a property definition compares a new
/// value against before deciding whether a non-configurable property is really changing.
///
/// It differs from `===` in two places: NaN equals itself, and `0` and `-0` are distinct.
/// The well-known symbol a `String.prototype` method hands its call to, if it is one of the
/// five that do.
///
/// `replace` and `replaceAll` share `@@replace`: the pair differ in how many matches they cover,
/// and that difference is settled by the regexp's own `g` flag rather than by two symbols.
pub(super) const fn string_delegating_symbol(native: Native) -> Option<WellKnown> {
    match native {
        Native::StringMatch => Some(WellKnown::Match),
        Native::StringMatchAll => Some(WellKnown::MatchAll),
        Native::StringReplace | Native::StringReplaceAll => Some(WellKnown::Replace),
        Native::StringSearch => Some(WellKnown::Search),
        Native::StringSplit => Some(WellKnown::Split),
        _ => None,
    }
}

/// `ToIntegerOrInfinity`, for the built-ins reached through `call_native`.
///
/// The same rule [`Vm::to_integer_or_infinity`] applies, spelled where a free function can use
/// it: NaN -- which is what `undefined` coerces to -- is zero, and everything else truncates
/// towards zero while an infinity stays one.
pub(super) fn to_integer_or_infinity(number: f64) -> f64 {
    if number.is_nan() { 0.0 } else { number.trunc() }
}

/// `ToLength`: a count, clamped into the range an index may occupy.
pub(super) fn to_length(number: f64) -> usize {
    if number.is_nan() || number <= 0.0 {
        return 0;
    }
    number.trunc().min(9_007_199_254_740_991.0) as usize
}

/// `AdvanceStringIndex`: the next position a match may start at.
///
/// A whole code point at a time when the pattern is Unicode-aware, and a single code unit
/// otherwise. The difference matters exactly once -- an empty match beside a surrogate pair --
/// and getting it wrong there splits an astral character down the middle.
pub(super) fn advance_string_index(units: &[u16], index: usize, unicode: bool) -> usize {
    if !unicode || index + 1 >= units.len() {
        return index + 1;
    }
    let lead = units[index];
    let trail = units[index + 1];
    if (0xD800..=0xDBFF).contains(&lead) && (0xDC00..=0xDFFF).contains(&trail) {
        index + 2
    } else {
        index + 1
    }
}

pub(super) fn same_value(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => {
            if a.is_nan() && b.is_nan() {
                true
            } else {
                a == b && a.is_sign_negative() == b.is_sign_negative()
            }
        }
        _ => strict_equal(left, right),
    }
}

/// SameValueZero, which is what Set and Map use for key identity.
///
/// It differs from `===` in exactly one place: NaN is its own key, so a Set can contain one and
/// find it again. `-0` and `0` remain the same key under both.
pub(super) fn same_value_zero(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) if a.is_nan() && b.is_nan() => true,
        _ => strict_equal(left, right),
    }
}

pub(super) fn array_method_value(
    method: kiln_builtin_surface::ArrayMethod,
    realm: RealmId,
) -> Value {
    use kiln_builtin_surface::ArrayMethod;
    match method {
        ArrayMethod::At => Value::Native(Native::ArrayAt, realm),
        ArrayMethod::Concat => Value::Native(Native::ArrayConcat, realm),
        ArrayMethod::CopyWithin => Value::Native(Native::ArrayCopyWithin, realm),
        ArrayMethod::Entries => Value::Native(Native::ArrayEntries, realm),
        ArrayMethod::Every => Value::Callback(CallbackMethod::Every, realm),
        ArrayMethod::Fill => Value::Native(Native::ArrayFill, realm),
        ArrayMethod::Filter => Value::Callback(CallbackMethod::Filter, realm),
        ArrayMethod::Find => Value::Callback(CallbackMethod::Find, realm),
        ArrayMethod::FindIndex => Value::Callback(CallbackMethod::FindIndex, realm),
        ArrayMethod::FindLast => Value::Callback(CallbackMethod::FindLast, realm),
        ArrayMethod::FindLastIndex => Value::Callback(CallbackMethod::FindLastIndex, realm),
        ArrayMethod::Flat => Value::Native(Native::ArrayFlat, realm),
        ArrayMethod::FlatMap => Value::Callback(CallbackMethod::FlatMap, realm),
        ArrayMethod::ForEach => Value::Callback(CallbackMethod::ForEach, realm),
        ArrayMethod::Includes => Value::Native(Native::ArrayIncludes, realm),
        ArrayMethod::IndexOf => Value::Native(Native::ArrayIndexOf, realm),
        ArrayMethod::Join => Value::Native(Native::ArrayJoin, realm),
        ArrayMethod::Keys => Value::Native(Native::ArrayKeys, realm),
        ArrayMethod::LastIndexOf => Value::Native(Native::ArrayLastIndexOf, realm),
        ArrayMethod::Map => Value::Callback(CallbackMethod::Map, realm),
        ArrayMethod::ParallelMap => Value::Callback(CallbackMethod::ParallelMap, realm),
        ArrayMethod::Pop => Value::Native(Native::ArrayPop, realm),
        ArrayMethod::Push => Value::Native(Native::ArrayPush, realm),
        ArrayMethod::Reduce => Value::Callback(CallbackMethod::Reduce, realm),
        ArrayMethod::ReduceRight => Value::Callback(CallbackMethod::ReduceRight, realm),
        ArrayMethod::Reverse => Value::Native(Native::ArrayReverse, realm),
        ArrayMethod::Shift => Value::Native(Native::ArrayShift, realm),
        ArrayMethod::Slice => Value::Native(Native::ArraySlice, realm),
        ArrayMethod::Some => Value::Callback(CallbackMethod::Some, realm),
        ArrayMethod::Sort => Value::Callback(CallbackMethod::Sort, realm),
        ArrayMethod::Splice => Value::Native(Native::ArraySplice, realm),
        ArrayMethod::ToLocaleString => Value::Native(Native::ArrayToLocaleString, realm),
        ArrayMethod::ToReversed => Value::Native(Native::ArrayCopy(ArrayCopy::ToReversed), realm),
        ArrayMethod::ToSorted => Value::Native(Native::ArrayCopy(ArrayCopy::ToSorted), realm),
        ArrayMethod::ToSpliced => Value::Native(Native::ArrayCopy(ArrayCopy::ToSpliced), realm),
        ArrayMethod::ToString => Value::Native(Native::ArrayJoinDefault, realm),
        ArrayMethod::Unshift => Value::Native(Native::ArrayUnshift, realm),
        ArrayMethod::Values => Value::Native(Native::ArrayValues, realm),
        ArrayMethod::With => Value::Native(Native::ArrayCopy(ArrayCopy::With), realm),
    }
}

/// A built-in that takes a function argument.
///
/// Separate from `Native` because these cannot be evaluated by `call_native`: they need to
/// re-enter the interpreter, which only the VM can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallbackMethod {
    /// `next` on a generator: resumes the body, which the VM must drive.
    GeneratorNext,
    /// `return` on a generator: finishes it without resuming.
    GeneratorReturn,
    /// `throw` on a generator: raises at the suspension point rather than resuming it.
    GeneratorThrow,
    /// `next` on an async generator: hands back a promise of the step.
    AsyncGeneratorNext,
    /// `return` on an async generator: unwinds it, and hands back a promise of the final step.
    AsyncGeneratorReturn,
    /// `throw` on an async generator: raises at the suspension point, promising the next step.
    AsyncGeneratorThrow,
    Map,
    /// `parallelMap`: `map` by another name on this engine. The compiled backend may run the
    /// walk on a pool (ADR 0058, application 2); the answer is the same array either way, and
    /// a name that reads `undefined` here while the other engine has a value is the failure
    /// CLAUDE.md calls the expensive one.
    ParallelMap,
    ForEach,
    Filter,
    /// `reduce`: the only one whose callback takes the accumulator, so it does not fit the
    /// `(item, index, array)` walk the rest share.
    Reduce,
    /// `sort`: its callback compares two elements rather than visiting one, so it does not fit
    /// the walk either.
    Sort,
    FindIndex,
    Find,
    Some,
    Every,
    /// `flatMap`: the forward walk, and then one level of flattening -- which is the whole of what
    /// it adds over `map`.
    FlatMap,
    /// The two that walk from the end. They visit a hole as undefined where `find` skips it: the
    /// language added them after it stopped skipping.
    FindLast,
    FindLastIndex,
    /// `reduceRight`: `reduce` from the other end, seeded with the last element that is there.
    ReduceRight,
}

pub(super) fn number_method_value(
    method: kiln_builtin_surface::NumberMethod,
    realm: RealmId,
) -> Value {
    match method {
        kiln_builtin_surface::NumberMethod::ToString => {
            Value::Native(Native::NumberToString, realm)
        }
        kiln_builtin_surface::NumberMethod::ToFixed => Value::Native(Native::NumberToFixed, realm),
        kiln_builtin_surface::NumberMethod::ToExponential => {
            Value::Native(Native::NumberToExponential, realm)
        }
        kiln_builtin_surface::NumberMethod::ToPrecision => {
            Value::Native(Native::NumberToPrecision, realm)
        }
        kiln_builtin_surface::NumberMethod::ValueOf => Value::Native(Native::NumberValueOf, realm),
        kiln_builtin_surface::NumberMethod::ToLocaleString => Value::Native(
            Native::Intl(intl::Op::Adapter(intl::Adapter::NumberToLocaleString)),
            realm,
        ),
    }
}

/// The methods a number primitive exposes.
pub(super) fn number_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Number,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::NumberConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::NumberMethod(method) => {
            number_method_value(method, RealmId::PRIMARY)
        }
        _ => Value::Undefined,
    }
}

pub(super) fn array_property(name: &str) -> Value {
    let Some(member) = kiln_builtin_surface::installed_prototype_member(
        kiln_builtin_surface::PrototypeId::Array,
        name,
    ) else {
        return Value::Undefined;
    };
    match member.binding {
        kiln_builtin_surface::SemanticBinding::ArrayLength => Value::Number(0.0),
        kiln_builtin_surface::SemanticBinding::Constructor => {
            Value::Native(Native::ArrayConstructor, RealmId::PRIMARY)
        }
        kiln_builtin_surface::SemanticBinding::ArrayUnscopables => Value::Null,
        kiln_builtin_surface::SemanticBinding::ArrayMethod(method) => {
            array_method_value(method, RealmId::PRIMARY)
        }
        _ => Value::Undefined,
    }
}

/// A key usable as a dense array index: a non-negative integer that fits in `usize`.
///
/// `a["0"]` and `a[0]` name the same element, so a string that round-trips through a number is
/// an index too. `a[-1]` and `a[1.5]` are ordinary string properties in JavaScript, and treating
/// them as indices would silently move data.
#[cfg(test)]
pub(super) fn array_index(key: &Value) -> Option<usize> {
    let number = match key {
        Value::Number(value) => *value,
        // A string reaches an element only in the one spelling that names it. `"01"`, `"1.0"`,
        // `" 1"`, `"1e2"` and `"-0"` are ordinary named properties in JavaScript, and parsing
        // them as numbers handed back an element the program never asked for.
        Value::String(text) => return index_key_units(text),
        _ => return None,
    };
    if number < 0.0 || number.fract() != 0.0 || !number.is_finite() {
        return None;
    }
    Some(number as usize)
}

/// `array_index`, with a property key already converted exactly once.
pub(super) fn array_index_key(key: PropertyKeyRef<'_>) -> Option<usize> {
    match key {
        PropertyKeyRef::Utf8(name) => canonical_index(name),
        PropertyKeyRef::Units(name) => index_key_units(name),
        PropertyKeyRef::SymbolUnits(_) => None,
    }
}

/// The element a property name reaches, if it is the canonical spelling of an index.
///
/// Canonical means the name is exactly what the index stringifies back to, which is the test the
/// language uses to separate an element from a property that merely looks like one.
pub(super) fn canonical_index(name: &str) -> Option<usize> {
    let index: usize = name.parse().ok()?;
    (index.to_string() == name).then_some(index)
}

/// The same array-index test over the language's UTF-16 key units. This avoids constructing a
/// lossy scalar spelling merely to order a property during insertion.
pub(super) fn index_key_units(name: &StringUnits) -> Option<usize> {
    let mut value = 0usize;
    let mut digits = 0usize;
    for unit in name.units() {
        if !(b'0' as u16..=b'9' as u16).contains(&unit) {
            return None;
        }
        if digits == 0 && unit == b'0' as u16 && name.unit_len() != 1 {
            return None;
        }

        value = value
            .checked_mul(10)?
            .checked_add(usize::from(unit - b'0' as u16))?;

        digits += 1;
    }
    (digits > 0 && value <= u32::MAX as usize - 1).then_some(value)
}

/// How long a wait with no limit is allowed to last before it reports a timeout.
///
/// See [`Vm::atomics_sleep`]: with one agent there is nothing that could ever end such a wait,
/// so the choice is between a bound and hanging the process.
pub(super) const ATOMICS_UNBOUNDED_WAIT_MS: f64 = 100.0;

/// `value` as the view's element type would hold it, wrapping the way a write does.
///
/// Atomic arithmetic is defined on the *stored* representation, so both operands are brought
/// into it before they meet: without this, `Atomics.and(u8, 0, 257)` would and against 257
/// rather than against the 1 the buffer would have held.
pub(super) fn narrow_element(kind: TypedKind, value: &Value) -> Value {
    let mut slot = vec![0_u8; kind.width()];
    kind.encode(value, &mut slot);
    kind.decode(&slot)
}

/// What a read-modify-write leaves in the element it touched.
///
/// Both operands are narrowed first, so the bitwise operations see the two's-complement pattern
/// the buffer holds rather than the number a program wrote; the result is narrowed again by the
/// write itself, which is where an addition wraps.
pub(super) fn atomics_combine(
    kind: TypedKind,
    op: AtomicsOp,
    old: &Value,
    operand: &Value,
) -> Value {
    let operand = narrow_element(kind, operand);
    if let (Value::BigInt(old), Value::BigInt(operand)) = (old, &operand) {
        let old = old.as_ref().clone();
        let operand = operand.as_ref().clone();
        return Value::BigInt(Rc::new(match op {
            AtomicsOp::Add => old + operand,
            AtomicsOp::Sub => old - operand,
            AtomicsOp::And => old & operand,
            AtomicsOp::Or => old | operand,
            AtomicsOp::Xor => old ^ operand,
            _ => operand,
        }));
    }
    #[expect(clippy::cast_possible_truncation)]
    let left = old.as_number() as i64;
    #[expect(clippy::cast_possible_truncation)]
    let right = operand.as_number() as i64;
    #[expect(clippy::cast_precision_loss)]
    let combined = match op {
        AtomicsOp::Add => left.wrapping_add(right),
        AtomicsOp::Sub => left.wrapping_sub(right),
        AtomicsOp::And => left & right,
        AtomicsOp::Or => left | right,
        AtomicsOp::Xor => left ^ right,
        _ => right,
    } as f64;
    Value::Number(combined)
}

/// `CanonicalNumericIndexString(P)`: the number a property name spells, when the name is exactly
/// what that number stringifies back to.
///
/// This is the line an integer-indexed exotic object draws between an *element* key and an
/// ordinary named one, and it is wider than "an index": `"1.5"`, `"-1"`, `"NaN"` and `"Infinity"`
/// are all element keys that reach no slot, so a typed array refuses them rather than storing
/// them, while `"01"` and `"1e2"` are ordinary names and are stored. `"-0"` is the one name the
/// specification admits by hand, because its round trip through `String` comes back as `"0"`.
pub(super) fn canonical_numeric_index(name: &str) -> Option<f64> {
    if name == "-0" {
        return Some(-0.0);
    }
    let number = parse_number(name);
    (number_string(number) == name).then_some(number)
}

/// `CanonicalNumericIndexString` over exact UTF-16 units.  Numeric spellings are ASCII by
/// definition, so matching the units directly is sufficient and keeps lone-surrogate keys out of
/// the scalar conversion path.
pub(super) fn canonical_numeric_index_key(key: PropertyKeyRef<'_>) -> Option<f64> {
    match key {
        PropertyKeyRef::Utf8(name) => canonical_numeric_index(name),
        PropertyKeyRef::SymbolUnits(_) => None,
        PropertyKeyRef::Units(name) => {
            let name = name;
            if name.is_empty() {
                return None;
            }
            if name == "-0" {
                return Some(-0.0);
            }
            let number = parse_number_units(name);
            let printed = number_string(number);
            units_match_str(name, &printed).then_some(number)
        }
    }
}

pub(super) fn units_match_str(units: &StringUnits, text: &str) -> bool {
    units.unit_len() == text.encode_utf16().count()
        && units
            .units()
            .zip(text.encode_utf16())
            .all(|(left, right)| left == right)
}

/// `IsValidIntegerIndex(O, index)`: the element this number reaches in a live view, if any.
///
/// Negative zero is refused along with the negatives it sorts with, and a view whose buffer was
/// detached or shrunk out from under it has no valid index at all.
pub(super) fn valid_integer_index(view: &TypedArrayData, index: f64) -> Option<usize> {
    if view.out_of_bounds() || index.is_sign_negative() || index.fract() != 0.0 {
        return None;
    }
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let at = index as usize;
    (at < view.len()).then_some(at)
}

/// `Array.prototype.join` semantics: null and undefined become empty, everything else stringifies.
/// A hole and `undefined` both render as nothing here, which is what `join` and `String()` do.
///
/// Bounded, and accumulated rather than collected, so a long array of long strings refuses at
/// the bound instead of materialising every piece and then the concatenation of all of them.
pub(super) fn join(items: &[Value], separator: &str) -> Result<String, ExecutionError> {
    let mut out = String::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            push_bounded(&mut out, separator)?;
        }
        match item {
            Value::Undefined | Value::Null | Value::Native(Native::Hole, _) => {}
            other => push_bounded(&mut out, &other.to_display_string())?,
        }
    }
    Ok(out)
}

pub(super) fn join_units(
    items: &[Value],
    separator: &StringUnits,
) -> Result<StringUnits, ExecutionError> {
    let mut out = StringUnitsBuilder::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push_units(separator)?;
        }
        match item {
            Value::Undefined | Value::Null | Value::Native(Native::Hole, _) => {}
            other => out.push_units(&other.to_string_units_checked()?)?,
        }
    }
    Ok(out.finish())
}

/// Remove an own property, reporting whether it is gone afterwards.
///
/// Which is why deleting something that was never there is still true, and why deleting a
/// string's `length` is false: the property is real and cannot be configured away. Every store
/// answers from the property's own `configurable` flag, so `delete` and the descriptor that
/// reports the flag cannot disagree.
/// `JSON.parse`'s grammar, on text that has already been converted to a string.
///
/// Separate from either caller because there are two: the pure built-in, and the machine's
/// route for a call that carries a reviver.
pub(super) fn parse_json_text(text: &StringUnits) -> Result<Value, ExecutionError> {
    kiln_json::parse_units(text, json_value_from_tape).map_err(|_| ExecutionError::InvalidJson)?
}

pub(super) type JsonSourceEntry = (usize, StringUnits, Value, StringUnits);

pub(super) fn parse_json_text_with_sources(
    text: &StringUnits,
) -> Result<(Value, Vec<JsonSourceEntry>, Option<StringUnits>), ExecutionError> {
    kiln_json::parse_units_with_source_spans(text, json_value_and_sources_from_tape)
        .map_err(|_| ExecutionError::InvalidJson)
}

pub(super) fn is_json_whitespace(unit: u16) -> bool {
    matches!(unit, 0x20 | 0x09 | 0x0a | 0x0d)
}

pub(super) fn delete_property_checked(
    intrinsics: &Intrinsics,
    target: &Value,
    key: &Value,
) -> Result<bool, ExecutionError> {
    let units = key.property_key_units_checked()?;
    Ok(delete_property_key(
        intrinsics,
        target,
        PropertyKeyRef::from_value(key, &units),
    ))
}

#[cfg(test)]
pub(super) fn delete_property(intrinsics: &Intrinsics, target: &Value, key: &Value) -> bool {
    delete_property_checked(intrinsics, target, key).expect("test property-key conversion")
}

pub(super) fn delete_property_key(
    intrinsics: &Intrinsics,
    target: &Value,
    key: PropertyKeyRef<'_>,
) -> bool {
    match target {
        Value::Shared(facade) => facade.own_property(key).is_none(),
        Value::Object(object) => {
            // `delete` reports whether the property is gone, and a non-configurable one never
            // is. `remove` already declines to do the work; the answer has to agree with it.
            let removable = object
                .borrow()
                .get_own_attributes_key(key)
                .is_none_or(|attributes| attributes.configurable);
            if !removable {
                return false;
            }
            object.borrow_mut().remove_key(key);
            true
        }
        // A function owns properties too -- a class's statics live in a store of its own -- and
        // `delete C.of` has to reach that store rather than silently succeeding against nothing.
        Value::Function(closure) => {
            let removable = closure
                .properties
                .borrow()
                .get_own_attributes_key(key)
                .is_none_or(|attributes| attributes.configurable);
            if !removable {
                return false;
            }
            closure.properties.borrow_mut().remove_key(key);
            true
        }
        // Removing an element leaves a *hole* rather than shortening the array: the slot reads
        // as undefined and `length` is unchanged, but `1 in a` is false and a console shows
        // `<1 empty item>`. The comment said hole and the code wrote undefined, which is the
        // one place the difference between the two is directly observable.
        Value::Array(items) => {
            // `length` is non-configurable, so `delete a.length` is false and the array keeps it.
            if key.matches_ascii("length") {
                return false;
            }
            if items.properties.borrow().is_frozen() {
                return false;
            }
            let Some(index) = array_index_key(key) else {
                // Not an index, so it is an ordinary named property of the array and it lives in
                // the store beside the elements. Reporting `true` without reaching that store
                // said the property was gone while it was still there -- which every
                // descriptor test in the corpus notices, because `verifyProperty` deletes a
                // property and restores it and had been restoring one that never left.
                return remove_exotic_property_key(&items.properties, key);
            };
            // An element a program locked down will not go, and `delete` reports that
            // rather than leaving `length` describing an array that no longer matches it.
            if let Some(entry) = array_exotic_element(items, index) {
                if !entry.attributes.configurable {
                    return false;
                }
                items
                    .elements
                    .borrow_mut()
                    .retain(|other| other.index != index);
            }
            if let Some(slot) = items.borrow_mut().get_mut(index) {
                items.sparse.set(true);
                *slot = Value::Native(Native::Hole, RealmId::PRIMARY);
            }
            true
        }
        Value::TypedArray(view) => match canonical_numeric_index_key(key) {
            // An element that is really there will not go -- the specification gives an
            // integer-indexed property no way to be configured away -- and an element key that
            // reaches no slot was never there to remove, so `delete` reports it gone.
            Some(index) => valid_integer_index(view, index).is_none(),
            None => remove_exotic_property_key(&view.properties, key),
        },
        other if exotic_properties(other).is_some() => match exotic_properties(target) {
            Some(store) if !exotic_reserved_name_key(target, key) => {
                remove_exotic_property_key(store, key)
            }
            _ => true,
        },
        Value::String(text) => {
            let indexed = array_index_key(key).is_some_and(|index| index < text.unit_len());
            !(key.matches_ascii("length") || indexed)
        }
        // A built-in's own properties are tabulated rather than stored, so removing one is
        // recording the removal in the realm's store -- which is what makes the specification's
        // `configurable: true` on almost all of them mean something.
        Value::Native(_, _) | Value::Callback(_, _) => {
            match own_attributes_key(intrinsics, target, key) {
                // Never there, so it is gone: `delete Math.nonesuch` is true.
                None => true,
                Some(attributes) if attributes.configurable => {
                    intrinsics.remove_key(target, key);
                    true
                }
                Some(_) => false,
            }
        }
        // Nothing else carries own properties, so there is nothing to fail to remove.
        _ => true,
    }
}

/// Every own property of a value, in enumeration order, symbol-keyed ones included.
///
/// This is what a spread copies, and it is deliberately not the same set `Object.keys` returns:
/// spread carries symbol keys across and `Object.keys` does not. An array or a string contributes
/// its indices, which is what makes `{ ...[1, 2] }` an object with keys "0" and "1"; a primitive
/// with nothing of its own contributes nothing, and so does `null` or `undefined`.

pub(super) fn own_properties_units(
    intrinsics: &Intrinsics,
    value: &Value,
) -> Vec<(StringUnits, Property)> {
    match value {
        // A built-in's tabulated properties are all non-enumerable, so what a spread or an
        // `Object.assign` copies off one is exactly what a program put there -- with one
        // exception, and it is the exception every bundler's output depends on. A Node builtin
        // module's exports *are* enumerable, which is what makes `{ ...require("path") }` and
        // `export * from "path"` copy anything at all.
        Value::Native(_, _) | Value::Callback(_, _) => intrinsics.enumerable_entries_units(value),
        // A function is an object, and a static a program hung on one is an own enumerable
        // property like any other -- which is the whole reason `{ ...postcss }` copies anything
        // at all when `postcss` is a function carrying its plugins. `length` and `name` are
        // stored non-enumerable and `prototype` is not in this store, so the pair that decides
        // what a spread sees is already the pair the specification marks: `{ ...f }` is what the
        // program put on `f` and nothing of the function itself.
        Value::Function(closure) => closure
            .properties
            .borrow()
            .entries()
            .map(|(key, property)| (key.clone(), property.clone()))
            .collect(),
        // A bound function keeps its own store, and `length` and `name` are not in it -- the
        // own-key walk supplies those two literally -- so what is here is exactly what a holder
        // added.
        Value::Bound(bound) => bound
            .properties
            .borrow()
            .entries()
            .map(|(key, property)| (key.clone(), property.clone()))
            .collect(),
        Value::Object(object) => string_wrapper_entries_units(value)
            .into_iter()
            .chain(
                object
                    .borrow()
                    .entries()
                    .map(|(key, property)| (key.clone(), property.clone())),
            )
            .collect(),
        // The indices first and then whatever the program hung on the array by name, which is
        // the order `OrdinaryOwnPropertyKeys` gives and the order `Object.keys` already used
        // here. Stopping at the indices made this function disagree with the own-key walk beside
        // it about the same array: `Object.keys(a)` listed `a.total` and `{ ...a }` dropped it.
        // `length` is absent from both, because it is not enumerable.
        Value::Array(items) => items
            .borrow()
            .iter()
            .enumerate()
            // A hole has no key, so it is absent from `Object.keys`, `for-in` and a spread.
            .filter(|(_, item)| !matches!(item, Value::Native(Native::Hole, _)))
            .map(|(index, item)| {
                (
                    StringUnits::from(index.to_string()),
                    Property::Value(item.clone()),
                )
            })
            .chain(
                items
                    .properties
                    .borrow()
                    .entries()
                    .map(|(key, property)| (key.clone(), property.clone()))
                    .collect::<Vec<_>>(),
            )
            .collect(),
        Value::Shared(facade) => facade
            .own_keys()
            .into_iter()
            .filter_map(|key| {
                let (property, attributes) = facade.own_property(PropertyKeyRef::Units(&key))?;
                attributes.enumerable.then_some((key, property))
            })
            .collect(),
        // The elements, and then whatever the program hung on the view: a spread copies both,
        // in the order the own-key walk lists them.
        Value::TypedArray(view) => {
            let mut entries: Vec<(StringUnits, Property)> = (0..view.len())
                .filter_map(|index| {
                    view.get(index)
                        .map(|value| (StringUnits::from(index.to_string()), Property::Value(value)))
                })
                .collect();
            entries.extend(
                view.properties
                    .borrow()
                    .entries()
                    .map(|(key, property)| (key.clone(), property.clone())),
            );
            entries
        }
        other if exotic_properties(other).is_some() => exotic_properties(value)
            .map(|store| {
                store
                    .borrow()
                    .entries()
                    .map(|(key, property)| (key.clone(), property.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        Value::String(text) => text
            .units()
            .enumerate()
            .map(|(index, unit)| {
                (
                    StringUnits::from(index.to_string()),
                    Property::Value(Value::String(StringUnits::from_units(&[unit]))),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Enumerable string properties for `Object.values`, `Object.entries`, and JSON. The key's
/// spelling cannot distinguish a user string from a Symbol with identical reserved units.
pub(super) fn own_string_properties_units(
    intrinsics: &Intrinsics,
    value: &Value,
) -> Vec<(StringUnits, Property)> {
    let stored = |data: &ObjectData| {
        data.string_entries()
            .map(|(key, property)| (key.clone(), property.clone()))
            .collect::<Vec<_>>()
    };
    match value {
        Value::Object(object) => string_wrapper_entries_units(value)
            .into_iter()
            .chain(stored(&object.borrow()))
            .collect(),
        Value::Function(function) => stored(&function.properties.borrow()),
        Value::Bound(bound) => stored(&bound.properties.borrow()),
        Value::Array(items) => items
            .borrow()
            .iter()
            .enumerate()
            .filter(|(_, item)| !matches!(item, Value::Native(Native::Hole, _)))
            .map(|(index, item)| {
                (
                    StringUnits::from(index.to_string()),
                    Property::Value(item.clone()),
                )
            })
            .chain(stored(&items.properties.borrow()))
            .collect(),
        Value::TypedArray(view) => {
            let mut entries = (0..view.len())
                .filter_map(|index| {
                    view.get(index)
                        .map(|item| (StringUnits::from(index.to_string()), Property::Value(item)))
                })
                .collect::<Vec<_>>();
            entries.extend(stored(&view.properties.borrow()));
            entries
        }
        other if exotic_properties(other).is_some() => exotic_properties(value)
            .map(|store| stored(&store.borrow()))
            .unwrap_or_default(),
        _ => own_properties_units(intrinsics, value),
    }
}

/// A number written in a base other than ten.
///
/// The fraction is expanded by repeated multiplication rather than converted through a decimal
/// string, because there is no decimal spelling of `0.1` in base 3 to convert. It stops at 52
/// digits, which is where the mantissa runs out and every further digit would be invented.
pub(super) fn number_in_radix(value: f64, radix: u32) -> String {
    if !value.is_finite() {
        return number_string(value);
    }
    let negative = value < 0.0;
    let magnitude = value.abs();

    let mut whole = magnitude.trunc();
    let mut digits = Vec::new();
    if whole == 0.0 {
        digits.push('0');
    }
    while whole >= 1.0 {
        let digit = (whole % f64::from(radix)) as u32;
        digits.push(char::from_digit(digit, radix).expect("a digit below the radix"));
        whole = (whole / f64::from(radix)).trunc();
    }
    digits.reverse();
    let mut text: String = digits.into_iter().collect();

    let mut fraction = magnitude.fract();
    if fraction > 0.0 {
        text.push('.');
        for _ in 0..52 {
            if fraction == 0.0 {
                break;
            }
            fraction *= f64::from(radix);
            let digit = fraction.trunc();
            text.push(char::from_digit(digit as u32, radix).expect("a digit below the radix"));
            fraction -= digit;
        }
    }

    if negative { format!("-{text}") } else { text }
}

/// Positional argument at `index` as a number, or NaN when there is none.
///
/// ToNumber refuses BigInt and Symbol with a TypeError.
pub(super) fn argument_number_at(arguments: &[Value], index: usize) -> Result<f64, ExecutionError> {
    match arguments.get(index) {
        None | Some(Value::Undefined) => Ok(f64::NAN),
        Some(Value::BigInt(_)) => Err(ExecutionError::BigIntIsNotANumber),
        Some(Value::Symbol(_)) => Err(ExecutionError::SymbolIsNotANumber),
        Some(other) => Ok(other.as_number()),
    }
}

/// The first argument as a number, or NaN when there is none.
///
/// `Math.abs()` is NaN rather than 0: a missing argument is `undefined`, and `Number(undefined)`
/// is NaN.
pub(super) fn argument_number(arguments: &[Value]) -> Result<f64, ExecutionError> {
    argument_number_at(arguments, 0)
}

pub(super) fn is_integer(value: f64) -> bool {
    value.is_finite() && value.fract() == 0.0
}

/// `Number.prototype.toFixed`, as the specification spells it: the integer `n` for which
/// `n / 10^f - x` is closest to zero, taking the *larger* `n` on a tie.
///
/// Not `format!("{x:.*}")`, which rounds ties to even -- so it answered `"2"` for
/// `(2.5).toFixed(0)` where the language says `"3"`. Written as an integer rounding rather than as
/// a formatting flag so the C backend can be the same handful of lines: the two engines agreeing
/// about rounding matters more than either being clever about it.
///
/// The scaling is done in binary64, so a value whose scaled form needs more than 53 bits gets the
/// nearest double rather than the exact integer the specification names. Everything a program
/// actually formats -- a price, a percentage, a duration -- is far inside that.
pub(super) fn number_to_fixed(value: f64, digits: u32) -> String {
    if !value.is_finite() || value.abs() >= 1e21 {
        // The specification hands both of these back to `ToString`.
        return number_string(value);
    }
    let places = digits as usize;
    // The *exact* decimal expansion, not a rounding of it. A binary64 has at most 1074 fractional
    // digits, so this is the value itself -- and it has to be, because the digit that decides the
    // rounding is one `{:.places$}` would already have consumed. That matters beyond ties:
    // `(1.45).toFixed(1)` is `"1.4"`, because 1.45 is really 1.44999..., and any scaling by a
    // power of ten rounds that up to 14.5 before the language ever gets to look at it.
    let exact = format!("{:.*}", 1074, value.abs());
    let point = exact.find('.').unwrap_or(exact.len());
    let mut kept: Vec<u8> = exact.bytes().filter(|byte| *byte != b'.').collect();
    let decided = point + places;
    // Round half *up*, which is what "if there are two such n, pick the larger n" says. Rust's own
    // formatting rounds ties to even and would answer `"2"` for `(2.5).toFixed(0)`.
    let round_up = kept.get(decided).is_some_and(|digit| *digit >= b'5');
    kept.truncate(decided);
    if round_up {
        let mut at = kept.len();
        loop {
            if at == 0 {
                kept.insert(0, b'1');
                break;
            }
            at -= 1;
            if kept[at] == b'9' {
                kept[at] = b'0';
            } else {
                kept[at] += 1;
                break;
            }
        }
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let text = String::from_utf8(kept).unwrap_or_default();
    // A carry can have lengthened the integer part, so it is measured rather than assumed.
    let split = text.len() - places;
    if places == 0 {
        return format!("{sign}{text}");
    }
    format!("{sign}{}.{}", &text[..split], &text[split..])
}

/// The `n` and `e` the specification's exponential formats name, for a non-negative finite value.
///
/// `count` significant digits of the *exact* decimal expansion, rounded half up -- the
/// specification's "if there are two such sets, pick the larger n" -- and the decimal exponent of
/// the first of them. Taken from the exact expansion for the reason [`number_to_fixed`] gives:
/// scaling by a power of ten first would round the deciding digit away before the language ever
/// looked at it.
pub(super) fn significant_digits(value: f64, count: usize) -> (String, i32) {
    let exact = format!("{value:.1074}");
    let point = exact.find('.').unwrap_or(exact.len());
    let digits: Vec<u8> = exact.bytes().filter(|byte| *byte != b'.').collect();
    let Some(first) = digits.iter().position(|digit| *digit != b'0') else {
        // Zero has no significant digit; the specification names its mantissa as zeros and its
        // exponent as zero.
        return ("0".repeat(count), 0);
    };
    // The digit at `first` sits in the place worth 10^(point - 1 - first).
    let exponent = point as i32 - 1 - first as i32;
    let mut kept: Vec<u8> = digits.iter().skip(first).take(count).copied().collect();
    kept.resize(count, b'0');
    if digits
        .get(first + count)
        .is_some_and(|digit| *digit >= b'5')
    {
        let mut at = kept.len();
        loop {
            if at == 0 {
                // The carry ran off the front: every digit was a nine, so the mantissa is a one
                // followed by zeros and the exponent moved up one.
                kept.insert(0, b'1');
                kept.pop();
                return (String::from_utf8(kept).unwrap_or_default(), exponent + 1);
            }
            at -= 1;
            if kept[at] == b'9' {
                kept[at] = b'0';
            } else {
                kept[at] += 1;
                break;
            }
        }
    }
    (String::from_utf8(kept).unwrap_or_default(), exponent)
}

/// One mantissa and exponent as both exponential formats write them: `1.25e+2`.
pub(super) fn exponential_string(sign: &str, mantissa: &str, exponent: i32) -> String {
    let body = if mantissa.len() > 1 {
        format!("{}.{}", &mantissa[..1], &mantissa[1..])
    } else {
        mantissa.to_owned()
    };
    let marker = if exponent < 0 { '-' } else { '+' };
    format!("{sign}{body}e{marker}{}", exponent.abs())
}

/// `Number.prototype.toExponential`.
///
/// With no digit count the specification asks for the fewest digits that name the value exactly,
/// which is what Rust's own exponential formatting already produces -- the same shortest
/// round-tripping form `number_string` relies on.
pub(super) fn number_to_exponential(value: f64, digits: Option<usize>) -> String {
    if !value.is_finite() {
        return number_string(value);
    }
    let sign = if value < 0.0 { "-" } else { "" };
    let magnitude = value.abs();
    let (mantissa, exponent) = match digits {
        Some(count) => significant_digits(magnitude, count + 1),
        None => {
            let rendered = format!("{magnitude:e}");
            let (mantissa, exponent) = rendered.split_once('e').unwrap_or((rendered.as_str(), "0"));
            (
                mantissa.replace('.', ""),
                exponent.parse::<i32>().unwrap_or(0),
            )
        }
    };
    exponential_string(sign, &mantissa, exponent)
}

/// `Number.prototype.toPrecision`.
///
/// The switch to the exponential form is the specification's own window, and it is what keeps
/// `(0.0000001).toPrecision(2)` from being a string of zeros with no digits in it.
pub(super) fn number_to_precision(value: f64, digits: usize) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let (mantissa, exponent) = significant_digits(value.abs(), digits);
    if exponent < -6 || exponent >= digits as i32 {
        return exponential_string(sign, &mantissa, exponent);
    }
    if exponent == digits as i32 - 1 {
        return format!("{sign}{mantissa}");
    }
    if exponent >= 0 {
        let split = exponent as usize + 1;
        return format!("{sign}{}.{}", &mantissa[..split], &mantissa[split..]);
    }
    format!("{sign}0.{}{mantissa}", "0".repeat((-exponent - 1) as usize))
}

/// `BigInt.asIntN` and `asUintN`: the low `bits` of a value, read as signed or unsigned.
///
/// `None` for a width whose result no engine would build -- the specification allows up to 2^53,
/// and materialising a modulus that wide is a petabyte rather than an answer.
pub(super) fn truncate_bigint(
    value: &num_bigint::BigInt,
    bits: u64,
    signed: bool,
) -> Option<num_bigint::BigInt> {
    if bits == 0 {
        return Some(num_bigint::BigInt::from(0));
    }
    // A window wider than the value already is cannot change it. The one exception is a negative
    // value read as unsigned, which wraps however wide the window is.
    if value.bits() < bits && (signed || !matches!(value.sign(), num_bigint::Sign::Minus)) {
        return Some(value.clone());
    }
    let width = usize::try_from(bits).ok().filter(|bits| *bits <= 1 << 24)?;
    let modulus = num_bigint::BigInt::from(1) << width;
    let mut remainder = value % &modulus;
    if matches!(remainder.sign(), num_bigint::Sign::Minus) {
        remainder += &modulus;
    }
    if signed && remainder >= (&modulus >> 1_usize) {
        remainder -= &modulus;
    }
    Some(remainder)
}

/// `parseInt(text, radix?)`.
///
/// Reads as far as it can and stops, rather than rejecting: `parseInt("12px")` is 12. That
/// tolerance is the entire difference from `Number("12px")`, which is NaN.
pub(super) fn parse_int(arguments: &[Value]) -> Value {
    let text = arguments
        .first()
        .map_or_else(String::new, Value::to_display_string);
    // The language's white space, which is Unicode's plus U+FEFF (parallel issue 30).
    let text = text.trim_matches(super::builtin_members::is_js_whitespace);
    let radix = match arguments.get(1).map(Value::as_number) {
        Some(radix) if radix.is_finite() && radix != 0.0 => radix.trunc(),
        _ => 0.0,
    };
    // Ranged before the cast, not after: `as u32` saturates, so a negative radix became zero --
    // which reads as "no radix given" and quietly parsed `parseInt('12', -1)` as decimal where
    // the language says NaN.
    if !(radix == 0.0 || (2.0..=36.0).contains(&radix)) {
        return Value::Number(f64::NAN);
    }
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let radix = radix as u32;

    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    // `0x` selects hexadecimal when no radix was given, and is also allowed when 16 was.
    let (radix, digits) = match rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        Some(after) if radix == 0 || radix == 16 => (16, after),
        _ => (if radix == 0 { 10 } else { radix }, rest),
    };

    let end = digits
        .find(|character: char| !character.is_digit(radix))
        .unwrap_or(digits.len());
    if end == 0 {
        return Value::Number(f64::NAN);
    }
    // Folded as f64 rather than parsed into an integer: the result is a double either way, and a
    // long run of digits must saturate toward infinity rather than overflow.
    let magnitude = digits[..end].chars().fold(0.0f64, |total, digit| {
        total * f64::from(radix) + f64::from(digit.to_digit(radix).expect("checked by `find`"))
    });
    Value::Number(if negative { -magnitude } else { magnitude })
}

/// `parseFloat(text)`, which is `parseInt`'s tolerance applied to a decimal literal.
pub(super) fn parse_float(arguments: &[Value]) -> Value {
    let text = arguments
        .first()
        .map_or_else(String::new, Value::to_display_string);
    let text = text.trim_start_matches(super::builtin_members::is_js_whitespace);
    if text.starts_with("Infinity") || text.starts_with("+Infinity") {
        return Value::Number(f64::INFINITY);
    }
    if text.starts_with("-Infinity") {
        return Value::Number(f64::NEG_INFINITY);
    }
    // A decimal literal starts with a digit or a point. Checked before the prefix search below,
    // because Rust's float parser also accepts `inf`, `infinity` and `nan` -- spellings this
    // grammar does not have, and which would otherwise have made `parseFloat('inf')` infinite.
    if !text
        .strip_prefix(['+', '-'])
        .unwrap_or(text)
        .starts_with(|character: char| character.is_ascii_digit() || character == '.')
    {
        return Value::Number(f64::NAN);
    }
    // Take the longest prefix that parses, which is what "reads as far as it can" means when the
    // grammar allows an exponent: "1e" is 1, not an error.
    let mut best = None;
    for end in (1..=text.len()).rev() {
        if !text.is_char_boundary(end) {
            continue;
        }
        if let Ok(number) = text[..end].parse::<f64>() {
            best = Some(number);
            break;
        }
    }
    Value::Number(best.unwrap_or(f64::NAN))
}

/// Resolve one end of a `slice`-style range.
///
/// A negative argument counts from the end, which is the whole reason this is not a cast: `-2`
/// means "two from the end", not "clamp to zero". Both ends are clamped into the array.
/// Where a relative index lands, or `None` for one that names nothing.
///
/// A negative index counts back from the end, which is what `with(-1, x)` uses -- and a negative
/// one that reaches past the start names no element at all, where the clamping [`relative_index`]
/// used by `slice` would answer zero. The difference is a `RangeError` rather than a wrong
/// element, so the two cannot share.
pub(super) fn relative_position(relative: f64, length: usize) -> Option<usize> {
    if relative.is_nan() {
        return Some(0);
    }
    let absolute = if relative < 0.0 {
        length as f64 + relative.trunc()
    } else {
        relative.trunc()
    };
    if absolute < 0.0 || absolute > length as f64 {
        return None;
    }
    Some(absolute as usize)
}

pub(super) fn relative_index(argument: Option<&Value>, length: usize, default: usize) -> usize {
    let Some(value) = argument.filter(|value| !matches!(value, Value::Undefined)) else {
        return default;
    };
    let relative = value.as_number();
    if relative.is_nan() {
        return 0;
    }
    let length = length as f64;
    let resolved = if relative < 0.0 {
        (length + relative).max(0.0)
    } else {
        relative.min(length)
    };
    resolved as usize
}

/// A reader over JSON text.
///
/// Build the `Value` graph `JSON.parse` answers with from the tape the shared reader produced.
///
/// The reader is `kiln-json`, the same crate the native provider archive links, so the two engines
/// cannot disagree about the grammar (ADR 0025). What is left here is the part only this engine
/// can do: make its own strings, arrays and objects. Iterative over an explicit stack, as the
/// reader is, so a document's nesting is bounded by its length rather than by the machine's
/// stack. Duplicate keys take the last value in the first position and `__proto__` is an
/// ordinary own property, both because `ObjectData::set` does exactly that.
pub(super) fn json_value_from_tape(
    document: &kiln_json::Tape<'_>,
) -> Result<Value, ExecutionError> {
    json_value_and_sources_from_tape(document).map(|(value, _, _)| value)
}

pub(super) fn json_value_and_sources_from_tape(
    document: &kiln_json::Tape<'_>,
) -> Result<(Value, Vec<JsonSourceEntry>, Option<StringUnits>), ExecutionError> {
    enum Frame {
        Array {
            items: Vec<Value>,
            remaining: usize,
            sources: Vec<(StringUnits, Value, StringUnits)>,
        },
        Object {
            data: ObjectData,
            remaining: usize,
            key: Option<StringUnits>,
            sources: Vec<(StringUnits, Value, StringUnits)>,
        },
    }
    let mut stack: Vec<Frame> = Vec::new();
    let mut root = None;
    let mut entries = Vec::new();
    let mut root_source = None;
    for (node_index, node) in document.nodes().iter().enumerate() {
        if root.is_some() {
            return Err(ExecutionError::InvalidJson);
        }
        let count = node.len as usize;
        let mut value = match node.tag {
            kiln_json::TAG_NULL => Value::Null,
            kiln_json::TAG_FALSE => Value::Boolean(false),
            kiln_json::TAG_TRUE => Value::Boolean(true),
            kiln_json::TAG_NUMBER => Value::Number(node.as_number().unwrap_or(0.0)),
            kiln_json::TAG_STRING => Value::String(
                document
                    .string_units(node)
                    .ok_or(ExecutionError::InvalidJson)?,
            ),
            kiln_json::TAG_ARRAY => {
                if count != 0 {
                    stack.push(Frame::Array {
                        items: Vec::with_capacity(count),
                        remaining: count,
                        sources: Vec::new(),
                    });
                    continue;
                }
                Value::Array(ArrayData::new(Vec::new()))
            }
            kiln_json::TAG_OBJECT => {
                if count != 0 {
                    stack.push(Frame::Object {
                        data: ObjectData::default(),
                        remaining: count,
                        key: None,
                        sources: Vec::new(),
                    });
                    continue;
                }
                Value::Object(Rc::new(RefCell::new(ObjectData::default())))
            }
            _ => return Err(ExecutionError::InvalidJson),
        };
        let source = matches!(
            node.tag,
            kiln_json::TAG_NULL
                | kiln_json::TAG_FALSE
                | kiln_json::TAG_TRUE
                | kiln_json::TAG_NUMBER
                | kiln_json::TAG_STRING
        )
        .then(|| document.source_units(node_index))
        .flatten();
        if node_index == 0 && stack.is_empty() {
            root_source = source.clone();
        }
        // Deliver the value to the container it belongs to, and every container that completes
        // with it to the one above.
        loop {
            let Some(frame) = stack.last_mut() else {
                root = Some(value);
                break;
            };
            match frame {
                Frame::Array {
                    items,
                    remaining,
                    sources,
                } => {
                    if let Some(raw) = source.clone() {
                        sources.push((
                            StringUnits::from(items.len().to_string()),
                            value.clone(),
                            raw,
                        ));
                    }
                    items.push(value);
                    *remaining -= 1;
                    if *remaining != 0 {
                        break;
                    }
                }
                Frame::Object {
                    data,
                    remaining,
                    key,
                    sources,
                } => match key.take() {
                    None => {
                        let Value::String(name) = value else {
                            return Err(ExecutionError::InvalidJson);
                        };
                        *key = Some(name);
                        break;
                    }
                    Some(name) => {
                        sources.retain(|(existing, _, _)| existing != &name);
                        if let Some(raw) = source.clone() {
                            sources.push((name.clone(), value.clone(), raw));
                        }
                        data.set_units(&name, value);
                        *remaining -= 1;
                        if *remaining != 0 {
                            break;
                        }
                    }
                },
            }
            value = match stack.pop() {
                Some(Frame::Array { items, sources, .. }) => {
                    let array = ArrayData::new(items);
                    let identity = Rc::as_ptr(&array) as usize;
                    entries.extend(
                        sources
                            .into_iter()
                            .map(|(key, value, raw)| (identity, key, value, raw)),
                    );
                    Value::Array(array)
                }
                Some(Frame::Object { data, sources, .. }) => {
                    let object = Rc::new(RefCell::new(data));
                    let identity = Rc::as_ptr(&object) as usize;
                    entries.extend(
                        sources
                            .into_iter()
                            .map(|(key, value, raw)| (identity, key, value, raw)),
                    );
                    Value::Object(object)
                }
                None => return Err(ExecutionError::InvalidJson),
            };
        }
    }
    Ok((
        root.ok_or(ExecutionError::InvalidJson)?,
        entries,
        root_source,
    ))
}

/// Render a value the way a console does.
///
/// Deliberately not `to_display_string`, which is `String(value)` and answers a different
/// question: `String({ a: 1 })` is "[object Object]" and always will be, while a console showing
/// that instead of the contents is close to useless. Nested strings are quoted and top-level ones
/// are not, for the same reason -- `console.log("hi")` prints hi, but `{ a: "hi" }` has to show
/// that `a` holds a string rather than a bare word.
///
/// Follows node's `util.inspect` closely enough for `scripts/differential.sh` to diff against it,
/// with two documented limits: output is always one line, where node wraps past about 72
/// characters, and a cycle is marked `[Circular *1]` without node's matching `<ref *1>` prefix.
pub(crate) fn inspect(value: &Value) -> String {
    match value {
        // A top-level string prints as itself; only a nested one is quoted.
        Value::String(text) => text.to_lossy_string(),
        other => inspect_nested(other, 0, &mut Vec::new()),
    }
}

/// How deep node descends before printing a placeholder instead of the contents.
pub(super) const INSPECT_DEPTH: usize = 2;

pub(super) fn inspect_nested(value: &Value, depth: usize, seen: &mut Vec<usize>) -> String {
    match value {
        Value::String(text) => quote_for_console(&text.to_lossy_string()),
        Value::Undefined | Value::Null | Value::Boolean(_) | Value::Number(_) => {
            // `-0` prints as itself here, where `String(-0)` is "0": the console is showing the
            // value, not converting it.
            match value {
                Value::Number(number) if *number == 0.0 && number.is_sign_negative() => {
                    "-0".to_owned()
                }
                other => other.to_display_string(),
            }
        }
        Value::Symbol(symbol) => format!("Symbol({})", symbol.description()),
        Value::Function(closure) => match &closure.name {
            Some(name) => format!("[Function: {name}]"),
            None => "[Function (anonymous)]".to_owned(),
        },
        Value::Native(native, _) => match namespace_name(native) {
            Some(name) => format!("Object [{name}] {{}}"),
            None => format!("[Function: {}]", native_js_name(native)),
        },
        Value::Callback(method, _) => {
            format!("[Function: {}]", builtin_js_name(&format!("{method:?}")))
        }
        Value::Generator(_) => "Object [Generator] {}".to_owned(),
        Value::Intl(value) => format!("{} {{}}", value.service().tag()),
        Value::Settler(..) | Value::Capability(..) | Value::Element(_) => {
            "[Function (anonymous)]".to_owned()
        }
        // Node prints the target's name with a "bound " prefix; without a name to prefix, the
        // fact that it is bound is the whole of what there is to say.
        Value::Bound(_) => "[Function: bound]".to_owned(),
        Value::Wrapped(_) => "[Function: wrapped]".to_owned(),
        Value::CommonJsRequire(_) => "[Function: require]".to_owned(),
        Value::CommonJsResolve(_) => "[Function: resolve]".to_owned(),
        // Node shows what the traps produce; this shows the target, and says so, rather than
        // running a program's code because something was printed.
        Value::Proxy(proxy) => format!("Proxy [ {} ]", inspect_nested(&proxy.target, depth, seen)),
        Value::TypedArray(view) => {
            let items = view
                .items()
                .iter()
                .map(|item| inspect_nested(item, depth + 1, seen))
                .collect::<Vec<_>>();
            let name = view.kind.name();
            if items.is_empty() {
                format!("{name}(0) []")
            } else {
                format!("{name}({}) [ {} ]", items.len(), items.join(", "))
            }
        }
        Value::ArrayBuffer(buffer) => {
            let name = if buffer.shared {
                "SharedArrayBuffer"
            } else {
                "ArrayBuffer"
            };
            format!("{name} {{ byteLength: {} }}", buffer.byte_length())
        }
        Value::DataView(view) => format!(
            "DataView {{ byteLength: {}, byteOffset: {} }}",
            view.byte_length().unwrap_or(0),
            view.offset
        ),
        Value::RegExp(regexp) => format!(
            "/{}/{}",
            regexp.source.to_lossy_string(),
            canonical_regexp_flags(&regexp.flags)
        ),
        Value::Date(date) => {
            iso_string(date.time.get()).unwrap_or_else(|| "Invalid Date".to_owned())
        }
        Value::Temporal(built) => built
            .value()
            .to_temporal_string(&kiln_temporal::Options::default())
            .unwrap_or_else(|_| format!("[object {}]", built.kind().tag())),
        // A console shows the literal's spelling, which is how it is told from a number.
        Value::BigInt(value) => format!("{value}n"),
        Value::Promise(promise) => match &promise.state.borrow().settled {
            None => "Promise { <pending> }".to_owned(),
            Some(Settled::Fulfilled(value)) => {
                format!("Promise {{ {} }}", inspect_nested(value, depth + 1, seen))
            }
            Some(Settled::Rejected(reason)) => format!(
                "Promise {{ <rejected> {} }}",
                inspect_nested(reason, depth + 1, seen)
            ),
        },
        Value::Array(items) => {
            let Some(guard) = InspectGuard::enter(seen, Rc::as_ptr(items).cast::<()>()) else {
                return "[Circular *1]".to_owned();
            };
            if depth > INSPECT_DEPTH {
                return "[Array]".to_owned();
            }
            // An array's own named properties are shown after its elements: `index` and `input`
            // on a match result are the reason this matters, and hiding them made a printed
            // match look like a plain array of strings.
            let named: Vec<String> = items
                .properties
                .borrow()
                .entries()
                .filter(|(key, _)| !symbol::is_reserved(&key.to_lossy_string()))
                .map(|(key, property)| {
                    let shown = match property {
                        Property::Value(value) => inspect_nested(value, depth + 1, guard.seen),
                        Property::Accessor { .. } => "[Getter/Setter]".to_owned(),
                    };
                    format!("{}: {}", inspect_key(&key.to_lossy_string()), shown)
                })
                .collect();
            let items = items.borrow();
            if items.is_empty() && named.is_empty() {
                return "[]".to_owned();
            }
            // A run of holes is reported as a count rather than repeated, which is what node
            // does and what makes a sparse array readable at all.
            let mut parts: Vec<String> = Vec::new();
            let mut holes = 0usize;
            for item in items.iter() {
                if matches!(item, Value::Native(Native::Hole, _)) {
                    holes += 1;
                    continue;
                }
                if holes > 0 {
                    parts.push(empty_items(holes));
                    holes = 0;
                }
                parts.push(inspect_nested(item, depth + 1, guard.seen));
            }
            if holes > 0 {
                parts.push(empty_items(holes));
            }
            parts.extend(named);
            format!("[ {} ]", parts.join(", "))
        }
        Value::Object(object) => {
            let Some(guard) = InspectGuard::enter(seen, Rc::as_ptr(object).cast::<()>()) else {
                return "[Circular *1]".to_owned();
            };
            if depth > INSPECT_DEPTH {
                return "[Object]".to_owned();
            }
            inspect_object(&object.borrow(), depth, guard.seen)
        }
        Value::Shared(facade) => format!(
            "Shared {{ graph: {}, node: {} }}",
            facade.handle.graph().id(),
            facade.handle.index()
        ),
        Value::Cell(_) => unreachable!("a cell is never inspected"),
    }
}

pub(super) fn inspect_object(object: &ObjectData, depth: usize, seen: &mut Vec<usize>) -> String {
    // A Set or Map is an object carrying reserved state, and a console shows what it holds
    // rather than the bookkeeping that makes it work.
    if let Some(Value::Array(entries)) = object.get_own("@@entries") {
        let is_map = matches!(object.get_own("@@map"), Some(&Value::Boolean(true)));
        let live: Vec<_> = entries
            .borrow()
            .iter()
            .filter(|entry| !matches!(entry, Value::Native(Native::Tombstone, _)))
            .cloned()
            .collect();
        // A weak collection shows neither its size nor its contents. It cannot: an entry is
        // reachable only through a key the console does not hold, and printing a count would make
        // the collector's timing observable from the program. `Map(0) {}` was the wrong label
        // twice over -- it named a type the value is not, so a log line said `Map` about
        // something `x instanceof Map` is false of.
        if matches!(object.get_own("@@weak"), Some(&Value::Boolean(true))) {
            let label = if is_map { "WeakMap" } else { "WeakSet" };
            return format!("{label} {{ <items unknown> }}");
        }
        let label = if is_map { "Map" } else { "Set" };
        if live.is_empty() {
            return format!("{label}(0) {{}}");
        }
        let parts = live
            .iter()
            .map(|entry| match (is_map, entry) {
                (true, Value::Array(pair)) => {
                    let pair = pair.borrow();
                    let key = pair.first().cloned().unwrap_or(Value::Undefined);
                    let value = pair.get(1).cloned().unwrap_or(Value::Undefined);
                    format!(
                        "{} => {}",
                        inspect_nested(&key, depth + 1, seen),
                        inspect_nested(&value, depth + 1, seen)
                    )
                }
                (_, entry) => inspect_nested(entry, depth + 1, seen),
            })
            .collect::<Vec<_>>();
        return format!("{label}({}) {{ {} }}", live.len(), parts.join(", "));
    }

    let parts = object
        .entries()
        // Reserved keys are the VM's own and symbol-keyed properties; node shows the latter as
        // `[Symbol(x)]: v`, which needs a key-to-symbol map this VM does not keep.
        .filter(|(key, _)| !symbol::is_reserved(&key.to_lossy_string()))
        .map(|(key, property)| {
            let shown = match property {
                Property::Value(value) => inspect_nested(value, depth + 1, seen),
                // Node names the accessor rather than calling it: showing a value here would run
                // the program's code just because something was printed.
                Property::Accessor { get, set } => match (get.is_some(), set.is_some()) {
                    (true, true) => "[Getter/Setter]".to_owned(),
                    (true, false) => "[Getter]".to_owned(),
                    _ => "[Setter]".to_owned(),
                },
            };
            format!("{}: {}", inspect_key(&key.to_lossy_string()), shown)
        })
        .collect::<Vec<_>>();
    // Node names the class an instance came from, so a printed object says what it is. A plain
    // object's prototype is `Object.prototype`, which this realm does not have as an object, so
    // the absence of a prototype means plain here.
    let label = match constructor_label(object) {
        Some(name) => format!("{name} "),
        None => String::new(),
    };
    if parts.is_empty() {
        return format!("{label}{{}}");
    }
    format!("{label}{{ {} }}", parts.join(", "))
}

/// The name a console puts in front of an object's contents, if it has one.
///
/// Read off `constructor` on the prototype chain, which is where `x.constructor` reads it from
/// too, so the two can never disagree. `Object` is excluded: node prints a plain object bare.
pub(super) fn constructor_label(object: &ObjectData) -> Option<String> {
    let constructor = object.get("constructor")?;
    let name = match &constructor {
        Value::Function(closure) => closure.name.as_deref()?.to_owned(),
        Value::Native(native, _) => format!("{native:?}"),
        _ => return None,
    };
    (!name.is_empty() && name != "Object").then_some(name)
}

/// Compile a regular expression, or `None` when the engine rejects the pattern.
pub(super) fn build_regexp(pattern: &StringUnits, flags: &str) -> Option<Value> {
    let units = pattern.to_units();
    let cacheable = regexp_cache_admits(&units, flags);
    let compiled = if cacheable {
        compiled_regexp_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&units, flags)
    } else {
        None
    };
    let compiled = if let Some(compiled) = compiled {
        compiled
    } else {
        let mut codepoints = Vec::with_capacity(units.len());
        let mut index = 0;
        // Without `u` the pattern is code units like the subject: an astral literal is a pair of
        // surrogates that match a pair in the input one unit at a time. Only under `u` does a
        // surrogate pair in the pattern mean the one code point.
        let unicode = flags.contains('u') || flags.contains('v');
        while index < units.len() {
            let unit = units[index];
            if unicode
                && (0xd800..=0xdbff).contains(&unit)
                && units
                    .get(index + 1)
                    .is_some_and(|next| (0xdc00..=0xdfff).contains(next))
            {
                let low = units[index + 1];
                codepoints
                    .push(0x1_0000 + ((u32::from(unit) - 0xd800) << 10) + u32::from(low) - 0xdc00);
                index += 2;
            } else {
                codepoints.push(u32::from(unit));
                index += 1;
            }
        }
        let normalized = if flags.contains('v') {
            kiln_regexp::normalize_unicode_sets_pattern(&codepoints)
        } else {
            codepoints.clone()
        };
        let compiled = Arc::new(regress::Regex::from_unicode(normalized.into_iter(), flags).ok()?);
        if cacheable {
            compiled_regexp_cache()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(&units, flags, compiled)
        } else {
            compiled
        }
    };
    Some(Value::RegExp(Rc::new(RegExpData {
        source: pattern.clone(),
        flags: flags.to_owned(),
        compiled,
        global: flags.contains('g'),
        sticky: flags.contains('y'),
        has_indices: flags.contains('d'),
        properties: RefCell::new({
            // `lastIndex` is an ordinary own data property, which is what lets
            // `Object.defineProperty(r, "lastIndex", {writable: false})` be seen by the
            // matcher: writable, not enumerated, and not configurable.
            let mut data = ObjectData::default();
            data.define_value(
                "lastIndex",
                Value::Number(0.0),
                Attributes {
                    writable: true,
                    enumerable: false,
                    configurable: false,
                },
            );
            data
        }),
    })))
}

/// Run a regular expression against a subject from a given UTF-16 offset.
///
/// Returns the match's range and its capture groups, all in UTF-16 units -- which is what the
/// language counts in, and why the subject is converted rather than matched as UTF-8.
pub(super) fn regexp_search(
    regexp: &RegExpData,
    subject: &[u16],
    start: usize,
) -> Option<RegExpMatch> {
    if start > subject.len() {
        return None;
    }
    // Without `u` the language matches code units, so a surrogate pair is two characters
    // (`/^.$/.test("\u{1F600}")` is false); with it, one code point.
    let found = if regexp.flags.contains('u') || regexp.flags.contains('v') {
        regexp.compiled.find_from_utf16(subject, start).next()?
    } else {
        regexp.compiled.find_from_ucs2(subject, start).next()?
    };
    // A sticky pattern must match exactly at the cursor, not merely at or after it.
    if regexp.sticky && found.range().start != start {
        return None;
    }
    let groups = found.groups().collect();
    let names = found
        .named_groups()
        .map(|(name, range)| (name.to_owned(), range))
        .collect();
    Some((found.range(), groups, names))
}

/// `new Array(...)`: a length when given one number, the elements themselves otherwise.
///
/// The largest array this representation will materialise.
///
/// `new Array(n)` is specified to produce an array of length `n` that holds no elements at all,
/// and a sparse representation would honour that for every length the specification allows, up
/// to 2^32-1. This one is a dense `Vec`, so the only way to give an array that length is to
/// allocate every slot: `new Array(4294967295)` asks for tens of gigabytes and takes the host
/// down with it -- which is exactly how a conformance sweep of `built-ins/Array` reached a
/// hundred gigabytes and wedged the machine. Refusing above a bound trades a test that cannot
/// pass for one that cannot hang the host. Lifting the bound is what a sparse representation
/// would buy, and nothing else here needs to change for it.
pub(super) const MAX_DENSE_ARRAY: usize = 1 << 24;

/// The largest backing storage this implementation will allocate for one language string.
///
/// Every engine bounds a string's length -- V8 refuses above 2^29-24 units with `RangeError:
/// Invalid string length` -- and a program can observe only that some bound exists, not where it
/// is. So this one is set where the memory is, not where V8's is: 64 MiB, a quarter of
/// [`MAX_BUFFER_BYTES`], because a string that grows by concatenation holds the old bytes and
/// the new ones at once, and every intermediate on the way to the bound is a separate
/// allocation the host keeps -- a bound of V8's size peaks in the gigabytes before refusing.
/// The budget is backing storage: one byte per Latin1 unit and two bytes per UTF-16 unit.
///
/// Without a bound, `"1".repeat(1 << 20).replace(/(.+)/g, "$1".repeat(1 << 16))` -- which is a
/// real test, and one that accepts the throw as its answer -- expands toward 2^36 characters and
/// takes the process with it, reporting nothing for any other test in the same run.
pub(super) const MAX_STRING_STORAGE_BYTES: usize = 1 << 26;

/// Refuse a string that would exceed [`MAX_STRING_STORAGE_BYTES`].
///
/// Checked before the bytes are written rather than after, so the refusal costs one allocation
/// of the bound rather than the allocation the program asked for.
pub(super) fn bounded_string(bytes: usize) -> Result<(), ExecutionError> {
    if bytes > MAX_STRING_STORAGE_BYTES {
        return Err(ExecutionError::StringTooLong { bytes });
    }
    Ok(())
}

/// Append one character, refusing past [`MAX_STRING_STORAGE_BYTES`].
///
/// A replacement template is expanded character by character, and `$1` against a megabyte-long
/// match appends a megabyte every time it appears -- so the refusal has to be checked as the
/// string grows. Checked once at the end, the allocation has already happened: this is what took
/// `staging/sm/String/replace-math.js` to 3.9 GB.
pub(super) fn push_bounded_char(out: &mut String, character: char) -> Result<(), ExecutionError> {
    bounded_string(out.len().saturating_add(character.len_utf8()))?;
    out.push(character);
    Ok(())
}

/// Append text without growing past [`MAX_STRING_STORAGE_BYTES`].
pub(super) fn push_bounded(out: &mut String, text: &str) -> Result<(), ExecutionError> {
    bounded_string(out.len().saturating_add(text.len()))?;
    out.push_str(text);
    Ok(())
}

/// The largest byte buffer this implementation will allocate.
///
/// `new ArrayBuffer(n)` and `new Uint8Array(n)` take their size straight from the program, and a
/// Rust allocation failure is an `abort` rather than a panic: `catch_unwind` cannot contain it,
/// so a single test asking for 7.8 petabytes killed the process and reported nothing for the
/// whole of `built-ins/ArrayBuffer`. Refusing above a bound turns that into one failing test.
pub(super) const MAX_BUFFER_BYTES: f64 = (1usize << 28) as f64; // 256 MiB

/// `ToIndex`: the specification's coercion for a length or an offset.
///
/// Truncating rather than rounding, `undefined` and `NaN` reading as zero, and anything negative
/// refused outright. The refusal is the point: `new ArrayBuffer(-1)` and `rab.resize(-1)` are
/// `RangeError`s, and clamping them to zero would hand the program an empty buffer where it
/// asked for an impossible one and let the mistake travel.
pub(super) fn to_index(value: Option<&Value>) -> Result<f64, ExecutionError> {
    to_index_number(value.map_or(0.0, Value::as_number))
}

/// `ToIndex` once the coercion to a number has already happened.
///
/// Split out because an argument that is an object has to be reduced by *calling* one of its
/// methods, which only the machine can do -- and the rules below have to apply to what that
/// call produced rather than to the object.
pub(super) fn to_index_number(number: f64) -> Result<f64, ExecutionError> {
    let integer = if number.is_nan() { 0.0 } else { number.trunc() };
    if !(0.0..=9_007_199_254_740_991.0).contains(&integer) {
        return Err(ExecutionError::BufferLengthOutOfRange { length: number });
    }
    Ok(integer)
}

/// The element count for a buffer of `length` elements of `width` bytes, bounded.
pub(crate) fn bounded_buffer(length: f64, width: usize) -> Result<usize, ExecutionError> {
    if !length.is_finite() || length <= 0.0 {
        return Ok(0);
    }
    let bytes = length * width as f64;
    if bytes > MAX_BUFFER_BYTES {
        return Err(ExecutionError::BufferTooLarge { bytes });
    }
    Ok(length as usize)
}

/// The largest value the specification accepts as an array length.
pub(super) const MAX_ARRAY_LENGTH: f64 = 4_294_967_295.0;

/// `Number.MAX_SAFE_INTEGER`: the largest index the language will name.
///
/// `ToLength` clamps to it, and the methods that grow an array-like refuse to push past it --
/// which is not the same bound as an array's own `length`, and the two are checked separately.
pub(super) const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// The same bound as an integer, for the walks that count in one.
pub(super) const MAX_SAFE_INTEGER_AS_U64: u64 = 9_007_199_254_740_991;

/// What `sort` and `toSorted` refuse a comparator that is neither a function nor `undefined`
/// with. Written once because both check it, and before either touches its receiver.
pub(super) fn comparator_refused() -> ExecutionError {
    ExecutionError::TypeErrorMessage {
        message: "The comparison function must be either a function or undefined".to_owned(),
    }
}

/// The length an array-like claims, bounded to what this representation can materialise.
///
/// These built-ins read `length` off an arbitrary object and then produce that many elements, so
/// the *object* chooses the allocation. An array-like claiming 2^32-1 reserves tens of gigabytes
/// and then loops four billion times -- a hang wearing an allocation's clothes -- and Test262
/// passes exactly such objects on purpose. A length at or below zero, or not a number at all, is
/// an empty result rather than an error: that is what the specification's ToLength gives.
pub(super) fn bounded_length(length: f64) -> Result<usize, ExecutionError> {
    if !length.is_finite() || length <= 0.0 {
        return Ok(0);
    }
    if length > MAX_DENSE_ARRAY as f64 {
        return Err(ExecutionError::ArrayTooLarge {
            length: length as usize,
        });
    }
    Ok(length as usize)
}

pub(super) fn construct_array(arguments: &[Value]) -> Result<Value, ExecutionError> {
    if let [Value::Number(length)] = arguments {
        let length = *length;
        // A non-integer or out-of-range length is a RangeError. Returning an empty array --
        // which this did -- is a wrong answer that a test asserting the throw reads as a pass.
        if !length.is_finite() || length < 0.0 || length.fract() != 0.0 || length > MAX_ARRAY_LENGTH
        {
            return Err(ExecutionError::InvalidArrayLength);
        }
        let length = length as usize;
        if length > MAX_DENSE_ARRAY {
            return Err(ExecutionError::ArrayTooLarge { length });
        }
        return Ok(Value::Array(ArrayData::new(vec![
            Value::Native(
                Native::Hole,
                RealmId::PRIMARY
            );
            length
        ])));
    }
    Ok(Value::Array(ArrayData::new(arguments.to_vec())))
}
