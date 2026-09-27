use super::*;

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
    pub(super) property: Property,
    pub(super) attributes: Attributes,
}

#[derive(Clone, Copy)]
pub(super) enum PropertyKeyRef<'a> {
    Utf8(&'a str),
    Units(&'a StringUnits),
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
            Self::Units(stored) => stored == key,
        }
    }

    /// Borrow an ASCII spelling at the scalar intrinsic-table seam. Latin1 storage is the hot
    /// path and can be checked with `from_utf8` without allocation; UTF-16 storage deliberately
    /// yields no scalar spelling, so a lone surrogate can never enter a scalar table arm.
    pub(super) fn known_ascii(self) -> Option<Cow<'a, str>> {
        match self {
            Self::Utf8(key) => Some(Cow::Borrowed(key)),
            Self::Units(key) => match key.as_view() {
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
            Self::Units(key) => stored == key,
        }
    }

    pub(super) fn owned(self) -> StringUnits {
        match self {
            Self::Utf8(key) => StringUnits::from(key),
            Self::Units(key) => key.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ObjectData {
    pub(super) properties: Vec<Slot>,
    /// The built-in Error brand. This is an internal slot, not a reserved property: program code
    /// may read, write and delete every string key, including ones beginning with `@@`.
    pub(super) error_brand: bool,
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
    /// Held beside the properties rather than *in* them: a `Value::Cell` stored as a property
    /// value would reach `Object.keys`, a descriptor read, a spread and `JSON.stringify` as
    /// itself, which is a wrong answer everywhere it appeared. The slots keep ordinary values
    /// and are caught up from these before a read.
    pub(super) namespace_cells: Vec<(String, Value)>,
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
    StringDecoder(Rc<RefCell<StringDecoderState>>),
    AsyncLocalStorage(Rc<AsyncStorage>),
    Zlib(Rc<zlib::ZlibState>),
}

impl Default for ObjectData {
    fn default() -> Self {
        Self {
            properties: Vec::new(),
            error_brand: false,
            arguments_brand: false,
            buffer_brand: false,
            // Objects start open; closing one is always something a program asked for.
            extensible: true,
            frozen: false,
            module_namespace: false,
            namespace_cells: Vec::new(),
            shadow_realm: None,
            prototype: None,
            primitive: None,
            node_events: None,
            node_object: None,
        }
    }
}
