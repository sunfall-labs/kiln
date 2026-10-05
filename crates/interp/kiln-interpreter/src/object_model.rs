use super::*;
use std::hash::Hasher;

/// An ordinary object: string-keyed properties in insertion order.
/// A `Vec` rather than a map because objects here are small and property order is observable --
/// JavaScript enumerates string keys in insertion order, so the structure that preserves it for
/// free is the right one until a profile says otherwise. Lookup is linear, which is fine at this
/// size and is the kind of decision the timing harness exists to revisit.
/// Object identity is reference identity, which is what `===` asks of two objects. Structural
/// equality would call two distinct objects with equal contents the same object, and they are
/// not: assigning through one would then appear to change the other.
impl PartialEq for ObjectData {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// What a property slot holds.
///
/// An accessor is not a value that happens to be a function: reading one *calls* it, and writing
/// through one calls a different one. Collapsing the two would make `{ get x() { … } }` and
/// `{ x: function () { … } }` the same object, which they are not.
#[derive(Clone, Debug, PartialEq)]
pub enum Property {
    Value(Value),
    /// Either half may be absent. A getter-only property reads through the getter and discards
    /// writes; a setter-only one reads as `undefined`.
    Accessor {
        get: Option<Value>,
        set: Option<Value>,
    },
}

/// The three flags an own property carries, beside what it holds.
///
/// Kept on each slot rather than in a side-list of hidden names. The side-list could answer
/// "is this enumerated", which is all enumeration needs, and nothing else --
/// `Object.getOwnPropertyDescriptor` has to report what the *property* is, and a list of names
/// has no room for the other two flags or for the difference between a property that is not
/// writable and an object that is frozen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attributes {
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

impl Attributes {
    /// What an ordinary assignment creates. `o.a = 1` is writable, enumerated and deletable.
    pub const DEFAULT: Self = Self {
        writable: true,
        enumerable: true,
        configurable: true,
    };
    /// What the runtime's own properties and a class's methods get: reachable by name, and
    /// never enumerated. `Object.keys(new Error("x"))` is empty and `for (const k in new C())`
    /// skips the class body for the same reason -- neither is something the program put there.
    pub const BUILT_IN: Self = Self {
        writable: true,
        enumerable: false,
        configurable: true,
    };
    /// What a field the descriptor left out means when the property is being created.
    /// `Object.defineProperty(o, "a", { value: 1 })` is locked down on every axis it did not
    /// mention, which is the one place the language's default is `false` rather than `true`.
    pub const NONE: Self = Self {
        writable: false,
        enumerable: false,
        configurable: false,
    };
}

/// One own property: its key, what it holds, and how it behaves.
#[derive(Clone, Debug)]
pub(super) struct Slot {
    pub(super) key: StringUnits,
    /// Symbol identity is separate from a user string with the same reserved spelling.
    pub(super) symbol: bool,
    /// Engine bookkeeping is separate from a user string with the same spelling.
    pub(super) internal: bool,
    pub(super) property: Property,
    pub(super) attributes: Attributes,
}

/// Live bindings owned by a module namespace. Boxed so ordinary objects pay only the size of an
/// `Option` pointer for namespace-specific state.
#[derive(Clone, Debug, Default)]
pub(super) struct NamespaceBindings {
    pub(super) cells: Vec<Value>,
    pub(super) cell_slots: Vec<Option<usize>>,
}

#[derive(Clone, Copy)]
pub(super) enum PropertyKeyRef<'a> {
    Utf8(&'a str),
    Units(&'a StringUnits),
    SymbolUnits(&'a StringUnits),
}

impl<'a> PropertyKeyRef<'a> {
    /// Compare a language key with an ASCII spelling without materialising a scalar `String`.
    ///
    /// The units representation is authoritative.  In particular, a lone surrogate is never
    /// passed through Rust's replacement-character conversion merely to ask whether it names a
    /// built-in member.
    pub(super) fn matches_ascii(self, key: &str) -> bool {
        match self {
            Self::Utf8(stored) => stored == key,
            Self::Units(stored) | Self::SymbolUnits(stored) => stored == key,
        }
    }

    /// Borrow an ASCII spelling at the scalar intrinsic-table seam. Latin1 storage is the hot
    /// path and can be checked with `from_utf8` without allocation; UTF-16 storage deliberately
    /// yields no scalar spelling, so a lone surrogate can never enter a scalar table arm.
    pub(super) fn known_ascii(self) -> Option<Cow<'a, str>> {
        match self {
            Self::Utf8(key) => Some(Cow::Borrowed(key)),
            Self::Units(key) | Self::SymbolUnits(key) => match key.as_view() {
                StringView::Latin1(bytes) => {
                    let key = std::str::from_utf8(bytes).ok()?;
                    key.is_ascii().then_some(Cow::Borrowed(key))
                }
                StringView::Utf16(units) => {
                    let key = String::from_utf16(units).ok()?;
                    key.is_ascii().then_some(Cow::Owned(key))
                }
            },
        }
    }

    pub(super) fn matches(self, stored: &StringUnits) -> bool {
        match self {
            Self::Utf8(key) => stored == key,
            Self::Units(key) | Self::SymbolUnits(key) => stored == key,
        }
    }

    pub(super) fn owned(self) -> StringUnits {
        match self {
            Self::Utf8(key) => StringUnits::from(key),
            Self::Units(key) | Self::SymbolUnits(key) => key.clone(),
        }
    }

    pub(super) fn is_symbol(self) -> bool {
        match self {
            Self::Utf8(key) => is_symbol_key(key),
            Self::Units(_) => false,
            Self::SymbolUnits(_) => true,
        }
    }

    pub(super) fn is_internal(self) -> bool {
        matches!(self, Self::Utf8(key) if symbol::is_reserved(key) && !is_symbol_key(key))
    }

    pub(super) fn matches_slot(self, slot: &Slot) -> bool {
        self.is_symbol() == slot.symbol
            && self.is_internal() == slot.internal
            && self.matches(&slot.key)
    }

    pub(super) fn from_value(value: &Value, units: &'a StringUnits) -> Self {
        if matches!(value, Value::Symbol(_)) {
            Self::SymbolUnits(units)
        } else {
            Self::Units(units)
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct WeakCellValue(WeakCollectionKey);

impl WeakCellValue {
    pub(super) fn new(value: Value) -> Self {
        // FinalizationRegistry targets and unregister tokens are weak for every JavaScript
        // object identity. Keeping the fallback Value strongly here made a registered target
        // look like an unexplained external owner to the cycle collector, so it could never be
        // reclaimed. `register` validates CanBeHeldWeakly before constructing this handle.
        Self(
            WeakCollectionKey::new(&value)
                .expect("validated weakly-held values have a weak identity handle"),
        )
    }

    pub(super) fn upgrade(&self) -> Option<Value> {
        self.0.upgrade()
    }
}

#[derive(Clone, Debug)]
pub(super) struct FinalizationCell {
    pub(super) target: WeakCellValue,
    pub(super) held: Value,
    pub(super) token: Option<WeakCellValue>,
}

/// A weak collection entry has a genuinely weak key. Values are conditionally retained by the
/// collector only when the key is independently reachable (ECMAScript ephemeron semantics).
#[derive(Clone, Debug)]
pub(super) enum WeakCollectionKey {
    Object(Weak<RefCell<ObjectData>>),
    Closure(Weak<Closure>),
    Array(Weak<ArrayData>),
    Promise(Weak<PromiseData>),
    Generator(Weak<GeneratorData>),
    Shared(Weak<SharedFacade>),
    RegExp(Weak<RegExpData>),
    Date(Weak<DateData>),
    Temporal(Weak<TemporalData>),
    Intl(Weak<intl::IntlValue>),
    TypedArray(Weak<TypedArrayData>),
    ArrayBuffer(Weak<BufferData>),
    DataView(Weak<DataViewData>),
    Proxy(Weak<ProxyData>),
    Bound(Weak<BoundFunction>),
    Wrapped(Weak<WrappedFunction>),
    CommonJs(Weak<CommonJsRequire>),
    CommonJsResolve(Weak<CommonJsRequire>),
    Settler(Weak<PromiseData>, bool),
    Capability(Weak<CapabilityRecord>),
    Element(Weak<ElementFunction>),
    UniqueSymbol(Weak<UniqueSymbol>),
    WellKnownSymbol(WellKnown),
    Native(Native, RealmId),
    Callback(CallbackMethod, RealmId),
}

impl PartialEq for WeakCollectionKey {
    fn eq(&self, other: &Self) -> bool {
        use WeakCollectionKey as Key;
        match (self, other) {
            (Key::Object(a), Key::Object(b)) => Weak::ptr_eq(a, b),
            (Key::Closure(a), Key::Closure(b)) => Weak::ptr_eq(a, b),
            (Key::Array(a), Key::Array(b)) => Weak::ptr_eq(a, b),
            (Key::Promise(a), Key::Promise(b)) => Weak::ptr_eq(a, b),
            (Key::Generator(a), Key::Generator(b)) => Weak::ptr_eq(a, b),
            (Key::Shared(a), Key::Shared(b)) => Weak::ptr_eq(a, b),
            (Key::RegExp(a), Key::RegExp(b)) => Weak::ptr_eq(a, b),
            (Key::Date(a), Key::Date(b)) => Weak::ptr_eq(a, b),
            (Key::Temporal(a), Key::Temporal(b)) => Weak::ptr_eq(a, b),
            (Key::Intl(a), Key::Intl(b)) => Weak::ptr_eq(a, b),
            (Key::TypedArray(a), Key::TypedArray(b)) => Weak::ptr_eq(a, b),
            (Key::ArrayBuffer(a), Key::ArrayBuffer(b)) => Weak::ptr_eq(a, b),
            (Key::DataView(a), Key::DataView(b)) => Weak::ptr_eq(a, b),
            (Key::Proxy(a), Key::Proxy(b)) => Weak::ptr_eq(a, b),
            (Key::Bound(a), Key::Bound(b)) => Weak::ptr_eq(a, b),
            (Key::Wrapped(a), Key::Wrapped(b)) => Weak::ptr_eq(a, b),
            (Key::CommonJs(a), Key::CommonJs(b)) => Weak::ptr_eq(a, b),
            (Key::CommonJsResolve(a), Key::CommonJsResolve(b)) => Weak::ptr_eq(a, b),
            (Key::Settler(a, ar), Key::Settler(b, br)) => ar == br && Weak::ptr_eq(a, b),
            (Key::Capability(a), Key::Capability(b)) => Weak::ptr_eq(a, b),
            (Key::Element(a), Key::Element(b)) => Weak::ptr_eq(a, b),
            (Key::UniqueSymbol(a), Key::UniqueSymbol(b)) => Weak::ptr_eq(a, b),
            (Key::WellKnownSymbol(a), Key::WellKnownSymbol(b)) => a == b,
            (Key::Native(an, ar), Key::Native(bn, br)) => an == bn && ar == br,
            (Key::Callback(ac, ar), Key::Callback(bc, br)) => ac == bc && ar == br,
            _ => false,
        }
    }
}

impl Eq for WeakCollectionKey {}

impl std::hash::Hash for WeakCollectionKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        use WeakCollectionKey as Key;
        std::mem::discriminant(self).hash(state);
        match self {
            Key::Object(value) => value.as_ptr().hash(state),
            Key::Closure(value) => value.as_ptr().hash(state),
            Key::Array(value) => value.as_ptr().hash(state),
            Key::Promise(value) => value.as_ptr().hash(state),
            Key::Generator(value) => value.as_ptr().hash(state),
            Key::Shared(value) => value.as_ptr().hash(state),
            Key::RegExp(value) => value.as_ptr().hash(state),
            Key::Date(value) => value.as_ptr().hash(state),
            Key::Temporal(value) => value.as_ptr().hash(state),
            Key::Intl(value) => value.as_ptr().hash(state),
            Key::TypedArray(value) => value.as_ptr().hash(state),
            Key::ArrayBuffer(value) => value.as_ptr().hash(state),
            Key::DataView(value) => value.as_ptr().hash(state),
            Key::Proxy(value) => value.as_ptr().hash(state),
            Key::Bound(value) => value.as_ptr().hash(state),
            Key::Wrapped(value) => value.as_ptr().hash(state),
            Key::CommonJs(value) | Key::CommonJsResolve(value) => value.as_ptr().hash(state),
            Key::Settler(value, rejects) => {
                value.as_ptr().hash(state);
                rejects.hash(state);
            }
            Key::Capability(value) => value.as_ptr().hash(state),
            Key::Element(value) => value.as_ptr().hash(state),
            Key::UniqueSymbol(value) => value.as_ptr().hash(state),
            Key::WellKnownSymbol(value) => std::mem::discriminant(value).hash(state),
            // These engine callables are identity values too. Debug includes any operation
            // payload, unlike a discriminant-only hash, and equality remains the authoritative
            // check in the table.
            Key::Native(value, realm) => {
                format!("{value:?}").hash(state);
                realm.hash(state);
            }
            Key::Callback(value, realm) => {
                std::mem::discriminant(value).hash(state);
                realm.hash(state);
            }
        }
    }
}

impl WeakCollectionKey {
    pub(super) fn new(value: &Value) -> Option<Self> {
        Some(match value {
            Value::Object(value) => Self::Object(Rc::downgrade(value)),
            Value::Function(value) => Self::Closure(Rc::downgrade(value)),
            Value::Array(value) => Self::Array(Rc::downgrade(value)),
            Value::Promise(value) => Self::Promise(Rc::downgrade(value)),
            Value::Generator(value) => Self::Generator(Rc::downgrade(value)),
            Value::Shared(value) => Self::Shared(Rc::downgrade(value)),
            Value::RegExp(value) => Self::RegExp(Rc::downgrade(value)),
            Value::Date(value) => Self::Date(Rc::downgrade(value)),
            Value::Temporal(value) => Self::Temporal(Rc::downgrade(value)),
            Value::Intl(value) => Self::Intl(Rc::downgrade(value)),
            Value::TypedArray(value) => Self::TypedArray(Rc::downgrade(value)),
            Value::ArrayBuffer(value) => Self::ArrayBuffer(Rc::downgrade(value)),
            Value::DataView(value) => Self::DataView(Rc::downgrade(value)),
            Value::Proxy(value) => Self::Proxy(Rc::downgrade(value)),
            Value::Bound(value) => Self::Bound(Rc::downgrade(value)),
            Value::Wrapped(value) => Self::Wrapped(Rc::downgrade(value)),
            Value::CommonJsRequire(value) => Self::CommonJs(Rc::downgrade(value)),
            Value::CommonJsResolve(value) => Self::CommonJsResolve(Rc::downgrade(value)),
            Value::Settler(value, rejects) => Self::Settler(Rc::downgrade(value), *rejects),
            Value::Capability(value) => Self::Capability(Rc::downgrade(value)),
            Value::Element(value) => Self::Element(Rc::downgrade(value)),
            Value::Native(native, realm) => Self::Native(*native, *realm),
            Value::Callback(callback, realm) => Self::Callback(*callback, *realm),
            Value::Symbol(Symbol::Unique(value)) => Self::UniqueSymbol(Rc::downgrade(value)),
            Value::Symbol(Symbol::WellKnown(value)) => Self::WellKnownSymbol(*value),
            _ => return None,
        })
    }

    pub(super) fn upgrade(&self) -> Option<Value> {
        Some(match self {
            Self::Object(value) => Value::Object(value.upgrade()?),
            Self::Closure(value) => Value::Function(value.upgrade()?),
            Self::Array(value) => Value::Array(value.upgrade()?),
            Self::Promise(value) => Value::Promise(value.upgrade()?),
            Self::Generator(value) => Value::Generator(value.upgrade()?),
            Self::Shared(value) => Value::Shared(value.upgrade()?),
            Self::RegExp(value) => Value::RegExp(value.upgrade()?),
            Self::Date(value) => Value::Date(value.upgrade()?),
            Self::Temporal(value) => Value::Temporal(value.upgrade()?),
            Self::Intl(value) => Value::Intl(value.upgrade()?),
            Self::TypedArray(value) => Value::TypedArray(value.upgrade()?),
            Self::ArrayBuffer(value) => Value::ArrayBuffer(value.upgrade()?),
            Self::DataView(value) => Value::DataView(value.upgrade()?),
            Self::Proxy(value) => Value::Proxy(value.upgrade()?),
            Self::Bound(value) => Value::Bound(value.upgrade()?),
            Self::Wrapped(value) => Value::Wrapped(value.upgrade()?),
            Self::CommonJs(value) => Value::CommonJsRequire(value.upgrade()?),
            Self::CommonJsResolve(value) => Value::CommonJsResolve(value.upgrade()?),
            Self::Settler(value, rejects) => Value::Settler(value.upgrade()?, *rejects),
            Self::Capability(value) => Value::Capability(value.upgrade()?),
            Self::Element(value) => Value::Element(value.upgrade()?),
            Self::UniqueSymbol(value) => Value::Symbol(Symbol::Unique(value.upgrade()?)),
            Self::WellKnownSymbol(value) => Value::Symbol(Symbol::WellKnown(*value)),
            Self::Native(native, realm) => Value::Native(*native, *realm),
            Self::Callback(callback, realm) => Value::Callback(*callback, *realm),
        })
    }
}

#[derive(Clone, Debug)]
pub struct ObjectData {
    pub(super) properties: Vec<Slot>,
    /// Shared graph targets are weak even though this interpreter keeps ordinary WeakRef targets
    /// in a reserved property. The facade owns the graph lease, so storing it as a Value here
    /// would make WeakRef itself keep an otherwise unreachable shared graph alive.
    pub(super) weak_shared_target: Option<Weak<SharedFacade>>,
    /// Ordinary object targets are genuinely weak. Other opaque value kinds still use the
    /// reserved property representation until their Rc identities gain weak handle variants.
    pub(super) weak_ref_object_target: Option<Weak<RefCell<ObjectData>>>,
    pub(super) finalization_cells: Vec<FinalizationCell>,
    /// WeakMap and WeakSet entries are not ordinary child edges. The cycle collector treats
    /// WeakMap values as conditional ephemeron edges and never traces either weak key directly.
    pub(super) weak_collection_entries:
        Option<Box<std::collections::HashMap<WeakCollectionKey, Value>>>,
    /// The built-in Error brand. This is an internal slot, not a reserved property: program code
    /// may read, write and delete every string key, including ones beginning with `@@`.
    pub(super) error_brand: bool,
    /// The internal slot of a `%JSON.rawJSON%` carrier. The visible `rawJSON` data property is
    /// kept in the ordinary property list; this slot is the unforgeable brand and source text.
    pub(super) raw_json_text: Option<StringUnits>,
    /// Frames captured for the lazy Error stack accessor; not a property of the object.
    pub(super) error_stack_frames: Option<Value>,
    /// The `arguments` brand: `[[Class]]` is "Arguments", which only
    /// `Object.prototype.toString` can see.
    ///
    /// An internal slot rather than a `Symbol.toStringTag` property, because the tag is *not* a
    /// property: `Object.getOwnPropertyNames(arguments)` and `getOwnPropertySymbols(arguments)`
    /// both have to come back without it, and a program can delete a tag it can see.
    pub(super) arguments_brand: bool,
    /// `Object.preventExtensions` and `Object.seal`. Weaker than frozen: the properties that
    /// are already there stay writable, only the shape is closed.
    pub(super) extensible: bool,
    /// `Object.freeze`. A frozen object silently ignores every write, which is what sloppy mode
    /// does; strict mode throws, and reaching that needs the write path to know which it is in.
    pub(super) frozen: bool,
    /// A module namespace object, whose `[[Set]]` is defined to refuse.
    ///
    /// Not the same fact as `frozen`, which is why it is not spelled with it: a namespace
    /// reports each export as *writable* -- a program reading the descriptor is told so, and
    /// `Object.isFrozen` of one is `false` -- while the write itself never lands. That pair is
    /// the specification's own, and collapsing it either way is a wrong answer: freezing makes
    /// the descriptor lie, and leaving it ordinary lets a program overwrite an export.
    pub(super) module_namespace: bool,
    /// The exporting module's cells, for a namespace built over live bindings.
    ///
    /// The exporting module's cells, held separately from properties so they are never reflected
    /// as values. Ordinary objects pay only this optional pointer; namespace data is boxed.
    pub(super) namespace_bindings: Option<Box<NamespaceBindings>>,
    /// Strong generation owner for namespace objects created from an inactive generation. This
    /// edge keeps their source records available while the namespace is externally reachable.
    pub(super) module_generation: Option<crate::module_map::ModuleGenerationHandle>,
    /// The `[[Prototype]]` link. Reads fall through it; writes never do -- assigning a property
    /// that exists only on the prototype creates an own property rather than mutating the shared
    /// one, which is what keeps two instances of a class independent.
    pub(super) prototype: Option<Value>,
    /// The primitive this object boxes: `new String("x")`, `new Number(1)`, `Object(true)`.
    ///
    /// A slot on the ordinary object rather than a `Value` variant of its own, so that a wrapper
    /// *is* an object everywhere it has to be one -- it has identity, it takes properties, it
    /// answers `typeof` as "object" -- and only the handful of operations that can see through
    /// the box consult this: `ToPrimitive`, the wrapper prototypes' receiver, and
    /// `Object.prototype.toString`.
    pub(super) primitive: Option<Value>,
    /// Realm-local Node EventEmitter state, intentionally absent from property reflection.
    pub(super) node_events: Option<Vec<NodeEventBucket>>,
    /// Requested natural process status, hidden from ordinary property reflection.
    pub(super) process_exit_code: Option<i32>,
    /// Stateful codec storage attached to the TextDecoder receiver, not a reflectable property.
    pub(super) text_decoder: Option<Rc<RefCell<kiln_encoding::StreamingDecoder>>>,
    /// Incremental Web Compression Streams codec, kept off reflective JS properties.
    pub(super) compression: Option<Rc<RefCell<kiln_compression::Codec>>>,
    /// Interpreter-owned Node server/socket/request/response state. Runtime drivers remain
    /// executor- and JavaScript-independent; this slot only ties one JS object to its adapter.
    pub(super) node_object: Option<NodeObject>,
    /// The Buffer brand on a `Uint8Array`'s slot: what `Buffer.isBuffer` answers and what
    /// routes a member read through `Buffer.prototype` (the object the shared bootstrap assigns,
    /// see `buffer_prototype`) before `Uint8Array.prototype`. Native keeps the same bit as
    /// `is_buffer` on the view. Until 2026-09-19 every `Uint8Array` here was a Buffer.
    pub(super) buffer_brand: bool,
    /// `[[ShadowRealm]]`: the realm a `ShadowRealm` instance evaluates in.
    ///
    /// An internal slot rather than a property, because a program must not be able to read the
    /// realm off the instance, write a different one onto it, or see it in
    /// `Object.getOwnPropertyNames`. It is what `ValidateShadowRealmObject` checks, and its
    /// absence is what makes `evaluate.call({}, src)` a `TypeError` rather than an evaluation in
    /// somebody's realm.
    ///
    /// On the ordinary object rather than a `Value` variant of its own because a `ShadowRealm`
    /// instance *is* an ordinary object everywhere else: it takes properties, it is extensible,
    /// and its `[[Prototype]]` is `ShadowRealm.prototype` like any other instance's.
    pub(super) shadow_realm: Option<RealmId>,
}

#[derive(Clone, Debug)]
pub(super) enum NodeObject {
    NodeServer(Rc<RefCell<NodeServerState>>),
    NetSocket(Rc<RefCell<NetSocketState>>),
    HttpIncoming(Rc<RefCell<HttpIncomingState>>),
    HttpResponse(Rc<RefCell<HttpResponseState>>),
    HttpClientRequest(Rc<RefCell<HttpClientRequestState>>),
    Readable(Rc<RefCell<ReadableState>>),
    Writable(Rc<RefCell<WritableState>>),
    Url(Rc<UrlState>),
    UrlSearchParams(Rc<UrlState>),
    Hash(Rc<RefCell<crypto::HashState>>),
    CryptoKey(Rc<kiln_key_core::ImportedKey>),
    StringDecoder(Rc<RefCell<StringDecoderState>>),
    AsyncLocalStorage(Rc<AsyncStorage>),
    Zlib(Rc<zlib::ZlibState>),
    /// A `Timeout` or `Immediate` the engine allocated: the timer's whole record. A slot rather
    /// than properties, so a program cannot see it and arming a timer costs no key lookups.
    TimerHandle(Box<TimerRecord>),
}

/// What a timer handle is: the timer's whole record. The realm's queue entry refers to the
/// handle (`Timer::Handle`) instead of copying the callback, firing reads it from here, and a
/// timeout that already ran keeps it for as long as the program holds the handle, so a
/// `refresh()` after the fact -- or from inside its own callback -- arms it again as node's does.
#[derive(Clone, Debug)]
pub(super) struct TimerRecord {
    /// The realm's identity for the timer, the same for its whole life, a refresh included.
    pub(super) id: u64,
    /// The delay it was armed with, after node's clamp: what a refresh arms again.
    pub(super) delay: std::time::Duration,
    pub(super) interval: bool,
    pub(super) immediate: bool,
    pub(super) callback: Value,
    pub(super) arguments: Vec<Value>,
    /// The async context in force when the timer was *set*, restored while its callback runs.
    pub(super) context: AsyncContext,
    /// A timeout or immediate ran and has not been armed since.
    pub(super) ran: bool,
    /// `unref()`, which `hasRef` still reports after the timer is gone.
    pub(super) unreferenced: bool,
    /// Cleared: the callback and arguments are dropped and `refresh()` does nothing.
    pub(super) cleared: bool,
}

impl Default for ObjectData {
    fn default() -> Self {
        Self {
            properties: Vec::new(),
            weak_shared_target: None,
            weak_ref_object_target: None,
            finalization_cells: Vec::new(),
            weak_collection_entries: None,
            error_brand: false,
            raw_json_text: None,
            error_stack_frames: None,
            arguments_brand: false,
            buffer_brand: false,
            // Objects start open; closing one is always something a program asked for.
            extensible: true,
            frozen: false,
            module_namespace: false,
            namespace_bindings: None,
            module_generation: None,
            shadow_realm: None,
            prototype: None,
            primitive: None,
            node_events: None,
            process_exit_code: None,
            text_decoder: None,
            compression: None,
            node_object: None,
        }
    }
}
